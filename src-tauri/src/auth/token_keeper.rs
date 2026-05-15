//! Background Token Keeper — proactively refreshes ChatGPT tokens before expiry.
//!
//! Inspired by cockpit-tools `provider_token_keeper.rs`.
//! Runs a background loop every `TICK_SECONDS` that checks all ChatGPT accounts
//! and refreshes tokens that are expired or near-expiry.

use std::sync::atomic::{AtomicBool, Ordering};

use tokio::time::{sleep, Duration};

use super::storage::load_accounts;
use super::token_refresh::ensure_chatgpt_tokens_fresh;
use crate::types::AuthData;

/// How often the keeper checks tokens (seconds).
const TICK_SECONDS: u64 = 60;

/// After a failed refresh, back off for this many seconds before retrying the same account.
const BACKOFF_SECONDS: u64 = 15 * 60; // 15 minutes

/// Global flag to ensure only one keeper loop runs.
static STARTED: AtomicBool = AtomicBool::new(false);

/// Start the background token keeper loop.
///
/// This should be called once during app setup. If called multiple times,
/// subsequent calls are no-ops.
///
/// The optional `app_handle` can be used in the future to emit Tauri events
/// when tokens are refreshed.
pub fn start(app_handle: tauri::AppHandle) {
    if STARTED.swap(true, Ordering::SeqCst) {
        println!("[TokenKeeper] Already running, skipping duplicate start");
        return;
    }

    println!("[TokenKeeper] Starting background token refresh loop (tick={TICK_SECONDS}s)");

    tauri::async_runtime::spawn(async move {
        run_loop(app_handle).await;
    });
}

async fn run_loop(_app_handle: tauri::AppHandle) {
    // Track per-account backoff timestamps
    let mut backoff_until: std::collections::HashMap<String, std::time::Instant> =
        std::collections::HashMap::new();

    loop {
        sleep(Duration::from_secs(TICK_SECONDS)).await;

        let accounts = match load_accounts() {
            Ok(store) => store.accounts,
            Err(err) => {
                println!("[TokenKeeper] Failed to load accounts: {err}");
                continue;
            }
        };

        let chatgpt_accounts: Vec<_> = accounts
            .iter()
            .filter(|a| matches!(a.auth_data, AuthData::ChatGPT { .. }))
            .collect();

        if chatgpt_accounts.is_empty() {
            continue;
        }

        for account in chatgpt_accounts {
            // Skip accounts in backoff period
            if let Some(&until) = backoff_until.get(&account.id) {
                if std::time::Instant::now() < until {
                    continue;
                }
            }

            match ensure_chatgpt_tokens_fresh(account).await {
                Ok(_updated) => {
                    // Clear any previous backoff
                    backoff_until.remove(&account.id);
                }
                Err(err) => {
                    println!(
                        "[TokenKeeper] Failed to refresh tokens for '{}': {err}",
                        account.name
                    );
                    // Set backoff
                    backoff_until.insert(
                        account.id.clone(),
                        std::time::Instant::now() + std::time::Duration::from_secs(BACKOFF_SECONDS),
                    );
                }
            }
        }
    }
}
