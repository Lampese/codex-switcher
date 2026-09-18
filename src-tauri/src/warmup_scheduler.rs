//! Host-owned automatic and timed warm-up scheduling.
//!
//! The scheduler is deliberately hosted by the Rust process rather than a
//! React window. Every host process may run this loop, but the durable
//! mutation lock from the storage layer makes only one process the owner of a
//! cycle at a time. The ledger is written only after a warm-up succeeds.

use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, Local, Utc};

use futures::{stream, StreamExt};

use crate::api::usage::{get_account_usage, warmup_account as send_warmup};
use crate::auth::{acquire_mutation_lock, load_accounts, load_app_settings, mutate_app_settings};
use crate::types::{
    StoredAccount, UsageInfo, WarmupAccountLedger, WarmupLedger, WarmupPolicy, WarmupState,
    WarmupSummary,
};

const SCHEDULER_INTERVAL: Duration = Duration::from_secs(30);
const FULL_WINDOW_SLACK_MINUTES: i64 = 5;
const LIMIT_FULL_THRESHOLD: f64 = 99.5;
const MIN_SUCCESS_INTERVAL_SECONDS: i64 = 60 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WindowKind {
    Session,
    Weekly,
}

impl WindowKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Weekly => "weekly",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AutoWindow {
    kind: WindowKind,
    window_minutes: i64,
    resets_at: i64,
}

/// Start one long-lived scheduler loop on the current async host.
pub async fn run() {
    loop {
        if let Err(error) = run_cycle().await {
            eprintln!("[Warmup] scheduler cycle failed: {error:#}");
        }
        tokio::time::sleep(SCHEDULER_INTERVAL).await;
    }
}

/// Return the durable policy and completion ledger projected to the UI.
pub fn get_state() -> Result<WarmupState> {
    let settings = load_app_settings()?;
    Ok(WarmupState {
        policy: settings.warmup_policy,
        ledger: settings.warmup_ledger,
    })
}

/// Normalize policy values at the host boundary so every UI client shares the
/// same durable representation and eligibility semantics.
pub fn normalize_policy(mut policy: WarmupPolicy) -> WarmupPolicy {
    policy
        .auto_warmup_account_ids
        .retain(|id| !id.trim().is_empty());
    policy.auto_warmup_account_ids.sort();
    policy.auto_warmup_account_ids.dedup();
    policy.timed_warmup_times = normalize_timed_warmup_times(&policy.timed_warmup_times);
    policy
}

/// Persist a policy without overwriting a concurrently updated ledger.
pub fn set_policy(policy: WarmupPolicy) -> Result<()> {
    let policy = normalize_policy(policy);
    mutate_app_settings(|settings| {
        settings.warmup_policy = policy;
        Ok(())
    })
}

/// Record a manually-triggered successful warm-up in the host ledger. This
/// keeps manual actions from immediately re-triggering automatic warm-up.
fn record_manual_success(account_id: &str, timestamp_ms: i64) -> Result<()> {
    mutate_app_settings(|settings| {
        update_manual_ledger(&mut settings.warmup_ledger, account_id, timestamp_ms);
        Ok(())
    })
}

async fn run_cycle() -> Result<()> {
    // The lock is held through the complete decision/request/ledger sequence.
    // A second desktop or codex-web host can therefore never execute the same
    // scheduled slot concurrently.
    let _owner = acquire_scheduler_owner().await?;

    let settings = load_app_settings()?;
    let policy = normalize_policy(settings.warmup_policy);
    let mut ledger = settings.warmup_ledger;
    let accounts = load_accounts()?.accounts;
    if accounts.is_empty() {
        return Ok(());
    }

    let now = Utc::now();
    if policy.auto_warmup_all_enabled || !policy.auto_warmup_account_ids.is_empty() {
        for account in accounts.iter().filter(|account| {
            policy.auto_warmup_all_enabled
                || policy
                    .auto_warmup_account_ids
                    .iter()
                    .any(|id| id == &account.id)
        }) {
            run_auto_for_account(account, &mut ledger, now).await;
        }
    }

    if policy.timed_warmup_enabled && !policy.timed_warmup_times.is_empty() {
        run_timed_for_current_slot(&policy, &accounts, &mut ledger, Local::now()).await;
    }

    Ok(())
}

async fn run_auto_for_account(
    account: &StoredAccount,
    ledger: &mut WarmupLedger,
    now: DateTime<Utc>,
) {
    let usage = match get_account_usage(account).await {
        Ok(usage) => usage,
        Err(error) => {
            eprintln!(
                "[Warmup] automatic usage refresh failed for {}: {error:#}",
                account.id
            );
            return;
        }
    };
    let history = ledger.accounts.get(&account.id);
    let Some(window) = due_auto_window(&usage, history, now.timestamp()) else {
        return;
    };

    if let Err(error) = send_warmup(account).await {
        eprintln!(
            "[Warmup] automatic warm-up failed for {}: {error:#}",
            account.id
        );
        return;
    }

    let timestamp_ms = now.timestamp_millis();
    if let Err(error) = record_auto_success(&account.id, timestamp_ms, window) {
        eprintln!(
            "[Warmup] failed to persist automatic success for {}: {error:#}",
            account.id
        );
        return;
    }
    update_auto_ledger(ledger, &account.id, timestamp_ms, window);
}

async fn run_timed_for_current_slot(
    policy: &WarmupPolicy,
    accounts: &[StoredAccount],
    ledger: &mut WarmupLedger,
    now: DateTime<Local>,
) {
    let current_time = now.format("%H:%M").to_string();
    if !policy
        .timed_warmup_times
        .iter()
        .any(|time| time == &current_time)
    {
        return;
    }
    let today = now.format("%Y-%m-%d").to_string();

    for account in accounts {
        let timed_key = format!("{}|{}", account.id, current_time);
        if ledger.timed_successes.get(&timed_key) == Some(&today) {
            continue;
        }

        let usage = match get_account_usage(account).await {
            Ok(usage) => usage,
            Err(error) => {
                eprintln!(
                    "[Warmup] timed usage refresh failed for {}: {error:#}",
                    account.id
                );
                continue;
            }
        };
        if usage.error.is_some()
            || usage
                .secondary_used_percent
                .is_some_and(|percent| percent >= LIMIT_FULL_THRESHOLD)
        {
            continue;
        }

        if let Err(error) = send_warmup(account).await {
            eprintln!(
                "[Warmup] timed warm-up failed for {}: {error:#}",
                account.id
            );
            continue;
        }

        if let Err(error) = record_timed_success(&account.id, &current_time, &today) {
            eprintln!(
                "[Warmup] failed to persist timed success for {}: {error:#}",
                account.id
            );
            continue;
        }
        ledger.timed_successes.insert(timed_key, today.clone());
        if let Some(entry) = ledger.accounts.get_mut(&account.id) {
            entry.last_successful_warmup_at = Some(now.timestamp_millis());
        } else {
            ledger.accounts.insert(
                account.id.clone(),
                WarmupAccountLedger {
                    last_successful_warmup_at: Some(now.timestamp_millis()),
                    ..WarmupAccountLedger::default()
                },
            );
        }
    }
}

async fn acquire_scheduler_owner() -> Result<crate::auth::MutationLock> {
    tokio::task::spawn_blocking(|| acquire_mutation_lock("warmup-scheduler.lock"))
        .await
        .context("scheduler owner task failed")?
}

/// Run one manual warm-up under the same host-owned scheduler boundary used
/// by automatic and timed warm-up, then persist completion from the host.
pub async fn run_manual_account(account: &StoredAccount) -> Result<()> {
    let _owner = acquire_scheduler_owner().await?;
    send_warmup(account).await?;
    record_manual_success(&account.id, Utc::now().timestamp_millis())
}

/// Run manual warm-up for every account under one scheduler owner and persist
/// successful completions without asking the UI to report them.
pub async fn run_manual_all(accounts: Vec<StoredAccount>) -> Result<WarmupSummary> {
    let _owner = acquire_scheduler_owner().await?;
    let total_accounts = accounts.len();
    let concurrency = total_accounts.min(10).max(1);
    let results: Vec<(String, bool)> = stream::iter(accounts.into_iter())
        .map(|account| async move {
            let account_id = account.id.clone();
            let succeeded = send_warmup(&account).await.is_ok();
            (account_id, succeeded)
        })
        .buffer_unordered(concurrency)
        .collect()
        .await;

    let timestamp_ms = Utc::now().timestamp_millis();
    for (account_id, succeeded) in &results {
        if *succeeded {
            record_manual_success(account_id, timestamp_ms)?;
        }
    }

    let failed_account_ids = results
        .into_iter()
        .filter_map(|(account_id, succeeded)| (!succeeded).then_some(account_id))
        .collect::<Vec<_>>();
    let warmed_accounts = total_accounts.saturating_sub(failed_account_ids.len());
    Ok(WarmupSummary {
        total_accounts,
        warmed_accounts,
        failed_account_ids,
    })
}

fn record_auto_success(account_id: &str, timestamp_ms: i64, window: AutoWindow) -> Result<()> {
    mutate_app_settings(|settings| {
        update_auto_ledger(
            &mut settings.warmup_ledger,
            account_id,
            timestamp_ms,
            window,
        );
        Ok(())
    })
}

fn record_timed_success(account_id: &str, time: &str, date: &str) -> Result<()> {
    mutate_app_settings(|settings| {
        settings
            .warmup_ledger
            .timed_successes
            .insert(format!("{account_id}|{time}"), date.to_string());
        let entry = settings
            .warmup_ledger
            .accounts
            .entry(account_id.to_string())
            .or_default();
        entry.last_successful_warmup_at = Some(Utc::now().timestamp_millis());
        Ok(())
    })
}

fn update_auto_ledger(
    ledger: &mut WarmupLedger,
    account_id: &str,
    timestamp_ms: i64,
    window: AutoWindow,
) {
    let entry = ledger.accounts.entry(account_id.to_string()).or_default();
    entry.last_successful_warmup_at = Some(timestamp_ms);
    entry.last_auto_window_key = Some(window_key(window));
    entry.last_auto_window_kind = Some(window.kind.as_str().to_string());
}

fn update_manual_ledger(ledger: &mut WarmupLedger, account_id: &str, timestamp_ms: i64) {
    ledger
        .accounts
        .entry(account_id.to_string())
        .or_default()
        .last_successful_warmup_at = Some(timestamp_ms);
}

fn due_auto_window(
    usage: &UsageInfo,
    history: Option<&WarmupAccountLedger>,
    now_seconds: i64,
) -> Option<AutoWindow> {
    if usage.error.is_some() {
        return None;
    }

    let (kind, window_minutes, resets_at) = if usage.primary_used_percent.is_some()
        || usage.primary_window_minutes.is_some()
        || usage.primary_resets_at.is_some()
    {
        (
            WindowKind::Session,
            usage.primary_window_minutes.unwrap_or(5 * 60),
            usage.primary_resets_at?,
        )
    } else if usage.secondary_used_percent.is_some()
        || usage.secondary_window_minutes.is_some()
        || usage.secondary_resets_at.is_some()
    {
        (
            WindowKind::Weekly,
            usage.secondary_window_minutes.unwrap_or(7 * 24 * 60),
            usage.secondary_resets_at?,
        )
    } else {
        return None;
    };

    if kind == WindowKind::Session
        && usage
            .secondary_used_percent
            .is_some_and(|percent| percent >= LIMIT_FULL_THRESHOLD)
    {
        return None;
    }

    if window_minutes <= 0 || resets_at <= 0 {
        return None;
    }
    let threshold_seconds = (window_minutes - FULL_WINDOW_SLACK_MINUTES).max(0) * 60;
    if resets_at - now_seconds < threshold_seconds {
        return None;
    }

    let window = AutoWindow {
        kind,
        window_minutes,
        resets_at,
    };
    if history.and_then(|entry| entry.last_auto_window_key.as_deref())
        == Some(window_key(window).as_str())
    {
        return None;
    }
    if let Some(last_successful) = history.and_then(|entry| entry.last_successful_warmup_at) {
        let same_kind = history
            .and_then(|entry| entry.last_auto_window_kind.as_deref())
            .is_none_or(|kind| kind == window.kind.as_str());
        if same_kind
            && now_seconds.saturating_sub(last_successful / 1000) < MIN_SUCCESS_INTERVAL_SECONDS
        {
            return None;
        }
    }
    Some(window)
}

fn window_key(window: AutoWindow) -> String {
    format!(
        "{}:{}:{}",
        window.kind.as_str(),
        window.window_minutes,
        window.resets_at
    )
}

fn normalize_timed_warmup_times(times: &[String]) -> Vec<String> {
    let mut normalized = times
        .iter()
        .filter_map(|raw| {
            let mut parts = raw.trim().split(':');
            let hours = parts.next()?.parse::<u8>().ok()?;
            let minutes = parts.next()?.parse::<u8>().ok()?;
            if parts.next().is_some() || hours > 23 || minutes > 59 {
                return None;
            }
            Some(format!("{hours:02}:{minutes:02}"))
        })
        .collect::<Vec<_>>();
    normalized.sort();
    normalized.dedup();
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage() -> UsageInfo {
        UsageInfo {
            account_id: "account".to_string(),
            plan_type: None,
            primary_used_percent: Some(10.0),
            primary_window_minutes: Some(300),
            primary_resets_at: Some(20_000),
            secondary_used_percent: Some(10.0),
            secondary_window_minutes: Some(10_080),
            secondary_resets_at: Some(20_000),
            has_credits: None,
            unlimited_credits: None,
            credits_balance: None,
            error: None,
        }
    }

    #[test]
    fn normalizes_and_deduplicates_timed_slots() {
        let input = vec![
            "9:5".to_string(),
            "09:05".to_string(),
            "25:00".to_string(),
            "bad".to_string(),
        ];
        assert_eq!(normalize_timed_warmup_times(&input), vec!["09:05"]);
    }

    #[test]
    fn auto_window_requires_fresh_window_and_new_ledger_key() {
        let candidate = due_auto_window(&usage(), None, 1_000).expect("window should be due");
        assert_eq!(candidate.kind, WindowKind::Session);

        let mut history = WarmupAccountLedger::default();
        history.last_auto_window_key = Some(window_key(candidate));
        assert!(due_auto_window(&usage(), Some(&history), 1_000).is_none());

        assert!(due_auto_window(&usage(), None, 2_500).is_none());
    }

    #[test]
    fn full_weekly_window_blocks_session_warmup() {
        let mut full = usage();
        full.secondary_used_percent = Some(100.0);
        assert!(due_auto_window(&full, None, 1_000).is_none());
    }

    #[test]
    fn manual_completion_updates_host_ledger_projection() {
        let mut ledger = WarmupLedger::default();
        update_manual_ledger(&mut ledger, "account", 42);

        assert_eq!(
            ledger
                .accounts
                .get("account")
                .and_then(|entry| entry.last_successful_warmup_at),
            Some(42)
        );
    }
}
