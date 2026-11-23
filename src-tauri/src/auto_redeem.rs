//! Auto-redeem of rate-limit reset credits before they expire (#133).
//!
//! When enabled, a background scheduler polls each ChatGPT account's reset
//! credits and consumes any credit whose expiry falls inside the lead window,
//! so it is used rather than lost. Opt-in via `set_auto_redeem_enabled`.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::auth::load_accounts;

/// Redeem a credit once its expiry is within this window (milliseconds).
pub const REDEEM_LEAD_MS: i64 = 75 * 1000;
/// How often the scheduler wakes to check for expiring credits.
const CHECK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);
/// Drop redeemed-credit records older than this to bound the state file.
const REDEEMED_RETENTION_MS: i64 = 30 * 24 * 60 * 60 * 1000;
/// Per-credit retry delay after a failed consume attempt.
const RETRY_AFTER_MS: i64 = 60 * 1000;

static STATE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AutoRedeemState {
    pub enabled: bool,
    /// credit_id -> redeemed_at (ms), so restarts never double-consume.
    pub redeemed: HashMap<String, i64>,
    /// credit_id -> next retry timestamp (ms) after a failed consume.
    pub retry_after: HashMap<String, i64>,
}

fn state_path() -> Result<PathBuf> {
    Ok(crate::auth::get_config_dir()?.join("auto-redeem.json"))
}

fn read_state() -> Result<AutoRedeemState> {
    let path = state_path()?;
    if !path.exists() {
        return Ok(AutoRedeemState::default());
    }
    let content = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    Ok(serde_json::from_str(&content).unwrap_or_default())
}

fn write_state(state: &AutoRedeemState) -> Result<()> {
    let path = state_path()?;
    let tmp = path.with_extension("json.tmp");
    let result = (|| {
        let mut file = fs::File::create(&tmp)
            .with_context(|| format!("Failed to create {}", tmp.display()))?;
        serde_json::to_writer_pretty(&mut file, state)?;
        use std::io::Write;
        file.flush()
            .and_then(|()| file.sync_all())
            .with_context(|| format!("Failed to write {}", tmp.display()))?;
        fs::rename(&tmp, &path).with_context(|| format!("Failed to replace {}", path.display()))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn update_state<T>(change: impl FnOnce(&mut AutoRedeemState) -> T) -> Result<T> {
    let _guard = STATE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut state = read_state()?;
    let result = change(&mut state);
    write_state(&state)?;
    Ok(result)
}

pub fn load_state() -> Result<AutoRedeemState> {
    let _guard = STATE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    read_state()
}

pub fn set_enabled(enabled: bool) -> Result<()> {
    update_state(|state| {
        state.enabled = enabled;
    })
}

pub fn is_enabled() -> bool {
    load_state().map(|state| state.enabled).unwrap_or(false)
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// A credit is due for auto-redeem when it is still reported `available` and
/// its expiry falls inside the lead window — close enough that waiting another
/// poll risks losing it, but not yet expired.
pub fn credit_is_due(
    status: &str,
    expires_at: Option<&str>,
    now_ms: i64,
    lead_ms: i64,
) -> bool {
    if !status.eq_ignore_ascii_case("available") {
        return false;
    }
    let Some(expires_at) = expires_at else {
        return false;
    };
    let Ok(expiry) = chrono::DateTime::parse_from_rfc3339(expires_at) else {
        return false;
    };
    let expiry_ms = expiry.timestamp_millis();
    expiry_ms > now_ms && expiry_ms <= now_ms + lead_ms
}

#[cfg(desktop)]
pub mod scheduler {
    use super::*;
    use crate::commands::account_stats::{fetch_reset_credits, redeem_reset_credit};
    use crate::types::AuthData;
    use tauri::{AppHandle, Emitter, Runtime};

    pub const AUTO_REDEEMED_EVENT: &str = "reset-credit-auto-redeemed";

    #[derive(Debug, Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    pub struct AutoRedeemedPayload {
        pub account_id: String,
        pub credit_id: String,
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
                eprintln!("[AutoRedeem] Failed to load state: {error}");
                return;
            }
        };
        if !state.enabled {
            return;
        }
        let Ok(store) = load_accounts() else {
            return;
        };
        let now = now_ms();

        for account in store.accounts {
            if matches!(account.auth_data, AuthData::ApiKey { .. }) {
                continue;
            }

            let credits = match fetch_reset_credits(&account).await {
                Ok(credits) => credits,
                Err(error) => {
                    eprintln!(
                        "[AutoRedeem] Credit check failed for {}: {error}",
                        account.name
                    );
                    continue;
                }
            };

            for credit in credits.credits {
                if state.redeemed.contains_key(&credit.id) {
                    continue;
                }
                if state
                    .retry_after
                    .get(&credit.id)
                    .is_some_and(|retry_at| *retry_at > now)
                {
                    continue;
                }
                if !credit_is_due(
                    &credit.status,
                    credit.expires_at.as_deref(),
                    now,
                    REDEEM_LEAD_MS,
                ) {
                    continue;
                }

                match redeem_reset_credit(&account, &credit.id).await {
                    Ok(()) => {
                        println!(
                            "[AutoRedeem] Redeemed credit {} for {}",
                            credit.id, account.name
                        );
                        let redeemed_at = now_ms();
                        let _ = update_state(|state| {
                            state.redeemed.insert(credit.id.clone(), redeemed_at);
                            state.retry_after.remove(&credit.id);
                            state
                                .redeemed
                                .retain(|_, at| redeemed_at - *at < REDEEMED_RETENTION_MS);
                        });
                        let _ = app.emit(
                            AUTO_REDEEMED_EVENT,
                            AutoRedeemedPayload {
                                account_id: account.id.clone(),
                                credit_id: credit.id.clone(),
                            },
                        );
                    }
                    Err(error) => {
                        eprintln!(
                            "[AutoRedeem] Redeem failed for {} ({}): {error}",
                            account.name, credit.id
                        );
                        let _ = update_state(|state| {
                            state
                                .retry_after
                                .insert(credit.id.clone(), now_ms() + RETRY_AFTER_MS);
                        });
                    }
                }
            }
        }
    }
}

#[cfg(desktop)]
pub use scheduler::spawn_scheduler;

#[cfg(test)]
mod tests {
    use super::credit_is_due;

    #[test]
    fn credit_due_inside_lead_window() {
        let now = 1_800_000_000_000_i64;
        let expiry = chrono::DateTime::from_timestamp_millis(now + 60_000)
            .unwrap()
            .to_rfc3339();
        assert!(credit_is_due("available", Some(&expiry), now, 75_000));
    }

    #[test]
    fn credit_not_due_outside_lead_window() {
        let now = 1_800_000_000_000_i64;
        let expiry = chrono::DateTime::from_timestamp_millis(now + 600_000)
            .unwrap()
            .to_rfc3339();
        assert!(!credit_is_due("available", Some(&expiry), now, 75_000));
    }

    #[test]
    fn credit_not_due_when_already_expired() {
        let now = 1_800_000_000_000_i64;
        let expiry = chrono::DateTime::from_timestamp_millis(now - 1_000)
            .unwrap()
            .to_rfc3339();
        assert!(!credit_is_due("available", Some(&expiry), now, 75_000));
    }

    #[test]
    fn credit_not_due_when_not_available() {
        let now = 1_800_000_000_000_i64;
        let expiry = chrono::DateTime::from_timestamp_millis(now + 60_000)
            .unwrap()
            .to_rfc3339();
        assert!(!credit_is_due("redeemed", Some(&expiry), now, 75_000));
        assert!(!credit_is_due("expired", Some(&expiry), now, 75_000));
    }

    #[test]
    fn credit_not_due_without_expiry() {
        assert!(!credit_is_due("available", None, 1_800_000_000_000, 75_000));
    }
}
