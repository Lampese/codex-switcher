//! Native auto warm-up scheduler.
//!
//! On macOS the main webview is suspended a few minutes after the window is
//! closed to the menu bar, so a JavaScript timer cannot be trusted to notice a
//! fresh 5h window. Warm-up decisions therefore live here, in a plain thread
//! that keeps running while the UI sleeps. The policy mirrors
//! `src/lib/autoWarmupPolicy.ts`, which the browser runtime still uses.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::auth::get_config_dir;
use crate::types::UsageInfo;

const DEFAULT_SESSION_WINDOW_MINUTES: i64 = 5 * 60;
const DEFAULT_WEEKLY_WINDOW_MINUTES: i64 = 7 * 24 * 60;
const FULL_WINDOW_SLACK_MINUTES: i64 = 5;
const LIMIT_FULL_THRESHOLD: f64 = 99.5;
const MIN_SUCCESS_INTERVAL_MS: i64 = 60 * 60 * 1000;
const RETRY_BASE_DELAY: Duration = Duration::from_secs(60);
const RETRY_MAX_DELAY: Duration = Duration::from_secs(30 * 60);

/// Serializes read-modify-write cycles on the state file.
static STATE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WindowKind {
    Session,
    Weekly,
}

impl WindowKind {
    fn as_str(self) -> &'static str {
        match self {
            WindowKind::Session => "session",
            WindowKind::Weekly => "weekly",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WarmupWindow {
    pub kind: WindowKind,
    pub window_minutes: i64,
    pub resets_at: i64,
}

impl WarmupWindow {
    pub fn key(&self) -> String {
        format!(
            "{}:{}:{}",
            self.kind.as_str(),
            self.window_minutes,
            self.resets_at
        )
    }
}

/// Same shape as the frontend ledger entry so it can be forwarded as-is.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LedgerEntry {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_successful_warmup_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_auto_window_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_auto_window_kind: Option<WindowKind>,
}

impl LedgerEntry {
    /// A manual warm-up clears the auto window, exactly like the frontend ledger.
    pub fn after_warmup(at_ms: i64, window: Option<&WarmupWindow>) -> Self {
        Self {
            last_successful_warmup_at: Some(at_ms),
            last_auto_window_key: window.map(WarmupWindow::key),
            last_auto_window_kind: window.map(|window| window.kind),
        }
    }

    fn warmed_at(&self) -> i64 {
        self.last_successful_warmup_at.unwrap_or(0)
    }
}

/// Merge history kept by an older build in the webview's localStorage.
/// For each account the entry with the later successful warm-up wins.
fn merge_ledger(ledger: &mut HashMap<String, LedgerEntry>, incoming: HashMap<String, LedgerEntry>) {
    for (account_id, entry) in incoming {
        match ledger.get(&account_id) {
            Some(existing) if existing.warmed_at() >= entry.warmed_at() => {}
            _ => {
                ledger.insert(account_id, entry);
            }
        }
    }
}

/// Per-account retry delay after a failed usage check or warm-up, doubling
/// from one minute up to half an hour. A success clears it.
#[derive(Debug, Default)]
pub struct RetryBackoff {
    accounts: HashMap<String, (u32, Instant)>,
}

impl RetryBackoff {
    pub fn delay_for(failures: u32) -> Duration {
        let doublings = failures.saturating_sub(1).min(16);
        RETRY_BASE_DELAY
            .saturating_mul(1 << doublings)
            .min(RETRY_MAX_DELAY)
    }

    pub fn is_waiting(&self, account_id: &str, now: Instant) -> bool {
        self.accounts
            .get(account_id)
            .is_some_and(|(_, retry_at)| now < *retry_at)
    }

    pub fn record_failure(&mut self, account_id: &str, now: Instant) -> Duration {
        let failures = self
            .accounts
            .get(account_id)
            .map_or(1, |(failures, _)| failures.saturating_add(1));
        let delay = Self::delay_for(failures);
        self.accounts
            .insert(account_id.to_string(), (failures, now + delay));
        delay
    }

    pub fn record_success(&mut self, account_id: &str) {
        self.accounts.remove(account_id);
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AutoWarmupState {
    pub enabled_all: bool,
    pub account_ids: Vec<String>,
    pub ledger: HashMap<String, LedgerEntry>,
}

impl AutoWarmupState {
    pub fn is_enabled_for(&self, account_id: &str) -> bool {
        self.enabled_all || self.account_ids.iter().any(|id| id == account_id)
    }
}

// ============================================================================
// Policy (mirror of src/lib/autoWarmupPolicy.ts)
// ============================================================================

fn window_kind(usage: &UsageInfo) -> Option<WindowKind> {
    if usage.error.is_some() {
        return None;
    }
    let has_primary = usage.primary_used_percent.is_some()
        || usage.primary_window_minutes.is_some()
        || usage.primary_resets_at.is_some();
    if has_primary {
        return Some(WindowKind::Session);
    }
    let has_secondary = usage.secondary_used_percent.is_some()
        || usage.secondary_window_minutes.is_some()
        || usage.secondary_resets_at.is_some();
    has_secondary.then_some(WindowKind::Weekly)
}

fn warmup_window(usage: &UsageInfo) -> Option<WarmupWindow> {
    match window_kind(usage)? {
        WindowKind::Session => Some(WarmupWindow {
            kind: WindowKind::Session,
            window_minutes: usage
                .primary_window_minutes
                .unwrap_or(DEFAULT_SESSION_WINDOW_MINUTES),
            resets_at: usage.primary_resets_at?,
        }),
        WindowKind::Weekly => Some(WarmupWindow {
            kind: WindowKind::Weekly,
            window_minutes: usage
                .secondary_window_minutes
                .unwrap_or(DEFAULT_WEEKLY_WINDOW_MINUTES),
            resets_at: usage.secondary_resets_at?,
        }),
    }
}

fn is_window_fresh(window: &WarmupWindow, now_ms: i64) -> bool {
    if window.window_minutes <= 0 || window.resets_at <= 0 {
        return false;
    }
    let remaining_ms = window.resets_at * 1000 - now_ms;
    let threshold_minutes = (window.window_minutes - FULL_WINDOW_SLACK_MINUTES).max(0);
    remaining_ms >= threshold_minutes * 60 * 1000
}

/// Return the window that should be warmed now, if any.
pub fn due_window(
    usage: &UsageInfo,
    history: Option<&LedgerEntry>,
    now_ms: i64,
) -> Option<WarmupWindow> {
    let window = warmup_window(usage)?;
    if window.kind == WindowKind::Session
        && usage
            .secondary_used_percent
            .is_some_and(|used| used >= LIMIT_FULL_THRESHOLD)
    {
        return None;
    }
    if !is_window_fresh(&window, now_ms) {
        return None;
    }

    let Some(history) = history else {
        return Some(window);
    };
    if history.last_auto_window_key.as_deref() == Some(window.key().as_str()) {
        return None;
    }
    if let Some(last_success) = history.last_successful_warmup_at.filter(|at| *at > 0) {
        let same_kind = history
            .last_auto_window_kind
            .is_none_or(|kind| kind == window.kind);
        if now_ms - last_success < MIN_SUCCESS_INTERVAL_MS && same_kind {
            return None;
        }
    }
    Some(window)
}

// ============================================================================
// Persistent state (~/.codex-switcher/auto-warmup.json)
// ============================================================================

fn state_file() -> Result<PathBuf> {
    Ok(get_config_dir()?.join("auto-warmup.json"))
}

fn read_state() -> Result<AutoWarmupState> {
    let path = state_file()?;
    if !path.exists() {
        return Ok(AutoWarmupState::default());
    }
    let content = fs::read(&path).with_context(|| format!("Failed to read {}", path.display()))?;
    match serde_json::from_slice(&content) {
        Ok(state) => Ok(state),
        Err(error) => {
            // A damaged file must not stop auto warm-up for good: keep it for
            // diagnosis and start over. The UI re-sends its toggles and ledger
            // on its next launch.
            let backup = path.with_extension("json.corrupt");
            eprintln!(
                "[AutoWarmup] {} is unreadable ({error}); moved to {} and reset",
                path.display(),
                backup.display()
            );
            if let Err(error) = fs::rename(&path, &backup) {
                eprintln!("[AutoWarmup] Failed to move the unreadable state aside: {error}");
            }
            Ok(AutoWarmupState::default())
        }
    }
}

/// Write to a sibling temp file, flush it, then rename it over the old file,
/// so an interrupted write leaves the previous state rather than a torn one.
fn write_state(state: &AutoWarmupState) -> Result<()> {
    let path = state_file()?;
    let parent = path
        .parent()
        .context("State file path has no parent directory")?;
    fs::create_dir_all(parent).with_context(|| format!("Failed to create {}", parent.display()))?;
    let content = serde_json::to_vec_pretty(state).context("Failed to serialize state")?;

    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    let result = (|| -> Result<()> {
        let mut file = fs::File::create(&tmp)
            .with_context(|| format!("Failed to create {}", tmp.display()))?;
        file.write_all(&content)
            .and_then(|()| file.sync_all())
            .with_context(|| format!("Failed to write {}", tmp.display()))?;
        fs::rename(&tmp, &path).with_context(|| format!("Failed to replace {}", path.display()))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn update_state<T>(change: impl FnOnce(&mut AutoWarmupState) -> T) -> Result<T> {
    let _guard = STATE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut state = read_state()?;
    let result = change(&mut state);
    write_state(&state)?;
    Ok(result)
}

pub fn load_state() -> Result<AutoWarmupState> {
    let _guard = STATE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    read_state()
}

/// Store the toggles chosen in the UI. `None` leaves a field unchanged.
/// `ledger` carries the UI's warm-up history; it is merged in the same write
/// that enables the scheduler, so an upgrade never starts without it.
pub fn set_config(
    enabled_all: Option<bool>,
    account_ids: Option<Vec<String>>,
    ledger: Option<HashMap<String, LedgerEntry>>,
) -> Result<()> {
    update_state(|state| {
        if let Some(enabled_all) = enabled_all {
            state.enabled_all = enabled_all;
        }
        if let Some(account_ids) = account_ids {
            state.account_ids = account_ids;
        }
        if let Some(ledger) = ledger {
            merge_ledger(&mut state.ledger, ledger);
        }
    })
}

/// Record a successful warm-up (manual, timed or automatic).
pub fn record_warmup(
    account_id: &str,
    at_ms: i64,
    window: Option<&WarmupWindow>,
) -> Result<LedgerEntry> {
    let entry = LedgerEntry::after_warmup(at_ms, window);
    update_state(|state| {
        state.ledger.insert(account_id.to_string(), entry.clone());
    })?;
    Ok(entry)
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

// ============================================================================
// Scheduler (desktop only)
// ============================================================================

#[cfg(desktop)]
pub use scheduler::{observe_usage, spawn_scheduler};

#[cfg(desktop)]
mod scheduler {
    use std::collections::HashMap;
    use std::sync::{LazyLock, Mutex};
    use std::time::{Duration, Instant};

    use tauri::{AppHandle, Emitter, Runtime};

    use super::{
        due_window, load_state, now_ms, record_warmup, LedgerEntry, RetryBackoff, WindowKind,
    };
    use crate::api::usage::{get_account_usage, warmup_account};
    use crate::auth::load_accounts;
    use crate::types::{AuthData, UsageInfo};

    const CHECK_INTERVAL: Duration = Duration::from_secs(30);
    /// Usage reported by the UI or the tray poller is reused while this fresh.
    const USAGE_MAX_AGE: Duration = Duration::from_secs(55);
    const STARTED_EVENT: &str = "auto-warmup-started";
    const FINISHED_EVENT: &str = "auto-warmup-finished";

    static RECENT_USAGE: LazyLock<Mutex<HashMap<String, (Instant, UsageInfo)>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));

    static BACKOFF: LazyLock<Mutex<RetryBackoff>> =
        LazyLock::new(|| Mutex::new(RetryBackoff::default()));

    fn backoff() -> std::sync::MutexGuard<'static, RetryBackoff> {
        BACKOFF
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn back_off(account_id: &str, account_name: &str, reason: &str) {
        let delay = backoff().record_failure(account_id, Instant::now());
        eprintln!(
            "[AutoWarmup] {reason} for {account_name}; next try in {}s",
            delay.as_secs()
        );
    }

    #[derive(Clone, serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct StartedPayload {
        account_id: String,
    }

    #[derive(Clone, serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct FinishedPayload {
        account_id: String,
        account_name: String,
        kind: WindowKind,
        entry: Option<LedgerEntry>,
        error: Option<String>,
    }

    /// Remember usage fetched elsewhere so the scheduler does not repeat it.
    pub fn observe_usage(usage: &UsageInfo) {
        if usage.error.is_some() {
            return;
        }
        if let Ok(mut recent) = RECENT_USAGE.lock() {
            recent.insert(usage.account_id.clone(), (Instant::now(), usage.clone()));
        }
    }

    fn recent_usage(account_id: &str) -> Option<UsageInfo> {
        let recent = RECENT_USAGE.lock().ok()?;
        let (seen_at, usage) = recent.get(account_id)?;
        (seen_at.elapsed() <= USAGE_MAX_AGE).then(|| usage.clone())
    }

    pub fn spawn_scheduler<R: Runtime>(app: AppHandle<R>) {
        std::thread::spawn(move || loop {
            tauri::async_runtime::block_on(run_cycle(&app));
            std::thread::sleep(CHECK_INTERVAL);
        });
    }

    async fn run_cycle<R: Runtime>(app: &AppHandle<R>) {
        let state = match load_state() {
            Ok(state) => state,
            Err(error) => {
                eprintln!("[AutoWarmup] Failed to load state: {error}");
                return;
            }
        };
        if !state.enabled_all && state.account_ids.is_empty() {
            return;
        }
        let Ok(store) = load_accounts() else {
            return;
        };

        for account in store.accounts {
            if matches!(account.auth_data, AuthData::ApiKey { .. })
                || !state.is_enabled_for(&account.id)
                || backoff().is_waiting(&account.id, Instant::now())
            {
                continue;
            }

            let usage = match recent_usage(&account.id) {
                Some(usage) => usage,
                None => match get_account_usage(&account).await {
                    Ok(usage) => {
                        // Feeds the tray labels and RECENT_USAGE.
                        crate::tray::ingest_usage(app, vec![usage.clone()]);
                        usage
                    }
                    Err(error) => {
                        back_off(
                            &account.id,
                            &account.name,
                            &format!("Usage check failed: {error}"),
                        );
                        continue;
                    }
                },
            };
            if let Some(error) = &usage.error {
                back_off(
                    &account.id,
                    &account.name,
                    &format!("Usage check failed: {error}"),
                );
                continue;
            }

            // Re-read toggles and history right before sending: the user may
            // have turned auto warm-up off, or warmed manually, mid-cycle.
            let current = match load_state() {
                Ok(current) => current,
                Err(error) => {
                    eprintln!("[AutoWarmup] Failed to load state: {error}");
                    return;
                }
            };
            if !current.is_enabled_for(&account.id) {
                continue;
            }
            let Some(window) = due_window(&usage, current.ledger.get(&account.id), now_ms()) else {
                // Nothing left to retry for this account.
                backoff().record_success(&account.id);
                continue;
            };

            let _ = app.emit(
                STARTED_EVENT,
                StartedPayload {
                    account_id: account.id.clone(),
                },
            );
            let (entry, error) = match warmup_account(&account).await {
                Ok(()) => match record_warmup(&account.id, now_ms(), Some(&window)) {
                    Ok(entry) => (Some(entry), None),
                    Err(error) => (None, Some(error.to_string())),
                },
                Err(error) => (None, Some(error.to_string())),
            };
            match &error {
                None => {
                    backoff().record_success(&account.id);
                    println!("[AutoWarmup] Warm-up sent for {}", account.name);
                }
                Some(error) => back_off(
                    &account.id,
                    &account.name,
                    &format!("Warm-up failed: {error}"),
                ),
            }
            let _ = app.emit(
                FINISHED_EVENT,
                FinishedPayload {
                    account_id: account.id.clone(),
                    account_name: account.name.clone(),
                    kind: window.kind,
                    entry,
                    error,
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW_MS: i64 = 1_800_000_000_000;
    const NOW_S: i64 = NOW_MS / 1000;

    fn usage(primary_resets_in_minutes: Option<i64>, weekly_used: f64) -> UsageInfo {
        UsageInfo {
            account_id: "a".into(),
            plan_type: Some("plus".into()),
            primary_used_percent: primary_resets_in_minutes.map(|_| 0.0),
            primary_window_minutes: primary_resets_in_minutes.map(|_| 300),
            primary_resets_at: primary_resets_in_minutes.map(|m| NOW_S + m * 60),
            secondary_used_percent: Some(weekly_used),
            secondary_window_minutes: Some(10_080),
            secondary_resets_at: Some(NOW_S + 3 * 24 * 3600),
            has_credits: None,
            unlimited_credits: None,
            credits_balance: None,
            error: None,
        }
    }

    #[test]
    fn fresh_session_window_is_due() {
        let window = due_window(&usage(Some(300), 40.0), None, NOW_MS).unwrap();
        assert_eq!(window.kind, WindowKind::Session);
        assert_eq!(window.key(), format!("session:300:{}", NOW_S + 300 * 60));
    }

    #[test]
    fn window_inside_slack_is_still_due() {
        assert!(due_window(&usage(Some(296), 40.0), None, NOW_MS).is_some());
    }

    #[test]
    fn running_window_is_not_due() {
        assert!(due_window(&usage(Some(294), 40.0), None, NOW_MS).is_none());
    }

    #[test]
    fn full_weekly_limit_blocks_session_warmup() {
        assert!(due_window(&usage(Some(300), 99.5), None, NOW_MS).is_none());
    }

    #[test]
    fn already_warmed_window_is_not_due() {
        let fresh = usage(Some(300), 40.0);
        let window = due_window(&fresh, None, NOW_MS).unwrap();
        let entry = LedgerEntry::after_warmup(NOW_MS - 2 * MIN_SUCCESS_INTERVAL_MS, Some(&window));
        assert!(due_window(&fresh, Some(&entry), NOW_MS).is_none());
    }

    #[test]
    fn recent_manual_warmup_blocks_auto_warmup_for_an_hour() {
        let fresh = usage(Some(300), 40.0);
        let recent = LedgerEntry::after_warmup(NOW_MS - 10 * 60 * 1000, None);
        assert!(due_window(&fresh, Some(&recent), NOW_MS).is_none());

        let old = LedgerEntry::after_warmup(NOW_MS - MIN_SUCCESS_INTERVAL_MS, None);
        assert!(due_window(&fresh, Some(&old), NOW_MS).is_some());
    }

    #[test]
    fn recent_warmup_of_other_kind_does_not_block() {
        let fresh = usage(Some(300), 40.0);
        let weekly = WarmupWindow {
            kind: WindowKind::Weekly,
            window_minutes: 10_080,
            resets_at: NOW_S + 10_080 * 60,
        };
        let entry = LedgerEntry::after_warmup(NOW_MS - 60_000, Some(&weekly));
        assert!(due_window(&fresh, Some(&entry), NOW_MS).is_some());
    }

    #[test]
    fn weekly_window_is_used_when_session_window_is_missing() {
        let mut weekly_only = usage(None, 0.0);
        weekly_only.secondary_resets_at = Some(NOW_S + 10_080 * 60);
        let window = due_window(&weekly_only, None, NOW_MS).unwrap();
        assert_eq!(window.kind, WindowKind::Weekly);
    }

    #[test]
    fn errored_usage_is_never_due() {
        let mut errored = usage(Some(300), 0.0);
        errored.error = Some("API error: 500".into());
        assert!(due_window(&errored, None, NOW_MS).is_none());
    }

    #[test]
    fn ledger_entry_matches_frontend_shape() {
        let window = due_window(&usage(Some(300), 0.0), None, NOW_MS).unwrap();
        let json = serde_json::to_value(LedgerEntry::after_warmup(NOW_MS, Some(&window))).unwrap();
        assert_eq!(json["lastSuccessfulWarmupAt"], NOW_MS);
        assert_eq!(json["lastAutoWindowKind"], "session");
        assert_eq!(json["lastAutoWindowKey"], window.key());

        let manual = serde_json::to_value(LedgerEntry::after_warmup(NOW_MS, None)).unwrap();
        assert!(manual.get("lastAutoWindowKey").is_none());
    }

    #[test]
    fn state_persists_config_and_ledger() {
        let _home = crate::auth::test_support::TestHome::new();
        set_config(Some(true), Some(vec!["b".into()]), None).unwrap();
        record_warmup("a", NOW_MS, None).unwrap();
        set_config(None, Some(vec![]), None).unwrap();

        let state = load_state().unwrap();
        assert!(state.enabled_all);
        assert!(state.account_ids.is_empty());
        assert_eq!(state.ledger["a"].last_successful_warmup_at, Some(NOW_MS));
        assert!(state.is_enabled_for("anything"));
    }

    #[test]
    fn state_write_replaces_file_without_leaving_temp_files() {
        let _home = crate::auth::test_support::TestHome::new();
        set_config(Some(true), None, None).unwrap();
        record_warmup("a", NOW_MS, None).unwrap();

        let path = state_file().unwrap();
        let leftovers: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with("auto-warmup") && name != "auto-warmup.json")
            .collect();
        assert!(leftovers.is_empty(), "unexpected files: {leftovers:?}");
        assert_eq!(
            load_state().unwrap().ledger["a"].last_successful_warmup_at,
            Some(NOW_MS)
        );
    }

    #[test]
    fn torn_state_file_is_moved_aside_and_scheduler_recovers() {
        let _home = crate::auth::test_support::TestHome::new();
        let path = state_file().unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, br#"{"enabledAll": true, "ledger": {"a": {"lastSucc"#).unwrap();

        let state = load_state().unwrap();
        assert!(!state.enabled_all);
        assert!(state.ledger.is_empty());
        assert!(!path.exists());
        assert!(path.with_extension("json.corrupt").exists());

        set_config(Some(true), None, None).unwrap();
        assert!(load_state().unwrap().enabled_all);
    }

    #[test]
    fn config_sync_migrates_ui_ledger_and_keeps_newer_native_entries() {
        let _home = crate::auth::test_support::TestHome::new();
        let window = due_window(&usage(Some(300), 0.0), None, NOW_MS).unwrap();
        record_warmup("native-newer", NOW_MS, None).unwrap();
        record_warmup("ui-newer", NOW_MS - 5_000, None).unwrap();

        let ui_ledger = HashMap::from([
            (
                "native-newer".to_string(),
                LedgerEntry::after_warmup(NOW_MS - 60_000, Some(&window)),
            ),
            (
                "ui-newer".to_string(),
                LedgerEntry::after_warmup(NOW_MS, Some(&window)),
            ),
            (
                "ui-only".to_string(),
                LedgerEntry::after_warmup(NOW_MS - 1_000, Some(&window)),
            ),
        ]);
        set_config(Some(true), None, Some(ui_ledger)).unwrap();

        let state = load_state().unwrap();
        assert!(state.enabled_all);
        assert_eq!(
            state.ledger["native-newer"],
            LedgerEntry::after_warmup(NOW_MS, None)
        );
        assert_eq!(
            state.ledger["ui-newer"],
            LedgerEntry::after_warmup(NOW_MS, Some(&window))
        );
        assert_eq!(
            state.ledger["ui-only"].last_auto_window_key.as_deref(),
            Some(window.key().as_str())
        );
        // The migrated key stops a second warm-up of the same window.
        assert!(due_window(&usage(Some(300), 0.0), state.ledger.get("ui-only"), NOW_MS).is_none());
    }

    #[test]
    fn retry_delay_doubles_and_caps() {
        assert_eq!(RetryBackoff::delay_for(1), Duration::from_secs(60));
        assert_eq!(RetryBackoff::delay_for(2), Duration::from_secs(120));
        assert_eq!(RetryBackoff::delay_for(5), Duration::from_secs(16 * 60));
        assert_eq!(RetryBackoff::delay_for(6), Duration::from_secs(30 * 60));
        assert_eq!(
            RetryBackoff::delay_for(u32::MAX),
            Duration::from_secs(30 * 60)
        );
    }

    #[test]
    fn failed_account_waits_until_retry_and_success_clears_it() {
        let start = Instant::now();
        let mut backoff = RetryBackoff::default();
        assert!(!backoff.is_waiting("a", start));

        assert_eq!(backoff.record_failure("a", start), Duration::from_secs(60));
        assert!(backoff.is_waiting("a", start + Duration::from_secs(30)));
        assert!(!backoff.is_waiting("a", start + Duration::from_secs(60)));
        assert!(!backoff.is_waiting("b", start));

        let later = start + Duration::from_secs(61);
        assert_eq!(backoff.record_failure("a", later), Duration::from_secs(120));
        assert!(backoff.is_waiting("a", later + Duration::from_secs(90)));

        backoff.record_success("a");
        assert!(!backoff.is_waiting("a", later));
        assert_eq!(backoff.record_failure("a", later), Duration::from_secs(60));
    }
}
