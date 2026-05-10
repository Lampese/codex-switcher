//! Background Usage Poller — auto-refreshes quota data at configurable intervals.
//!
//! Inspired by cockpit-tools `codex_quota.rs`.
//! Periodically polls `wham/usage` for all ChatGPT accounts and emits
//! Tauri events so the frontend can update in realtime.

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;
use tokio::time::{sleep, Duration};

use super::usage::refresh_all_usage;
use crate::auth::load_accounts;
use crate::types::AuthData;

/// Default polling interval in minutes.
const DEFAULT_INTERVAL_MINUTES: u64 = 5;

/// Minimum allowed interval (prevent abuse).
const MIN_INTERVAL_MINUTES: u64 = 1;

/// Global state for the poller.
struct PollerState {
    running: AtomicBool,
    interval_minutes: Mutex<u64>,
    stop_signal: Mutex<Option<tokio::sync::watch::Sender<bool>>>,
}

static POLLER: std::sync::OnceLock<PollerState> = std::sync::OnceLock::new();

fn get_poller() -> &'static PollerState {
    POLLER.get_or_init(|| PollerState {
        running: AtomicBool::new(false),
        interval_minutes: Mutex::new(DEFAULT_INTERVAL_MINUTES),
        stop_signal: Mutex::new(None),
    })
}

/// Start the auto-usage-poll background loop.
/// Returns `true` if started, `false` if already running.
pub async fn start_polling(app_handle: AppHandle, interval_minutes: Option<u64>) -> bool {
    let poller = get_poller();

    if poller.running.load(Ordering::SeqCst) {
        println!("[UsagePoller] Already running, skipping duplicate start");
        return false;
    }

    let interval = interval_minutes
        .unwrap_or(DEFAULT_INTERVAL_MINUTES)
        .max(MIN_INTERVAL_MINUTES);
    *poller.interval_minutes.lock().await = interval;

    let (tx, rx) = tokio::sync::watch::channel(false);
    *poller.stop_signal.lock().await = Some(tx);
    poller.running.store(true, Ordering::SeqCst);

    println!("[UsagePoller] Starting auto-poll every {interval} minutes");

    tauri::async_runtime::spawn(async move {
        run_poll_loop(app_handle, rx).await;
    });

    true
}

/// Stop the auto-usage-poll background loop.
/// Returns `true` if stopped, `false` if wasn't running.
pub async fn stop_polling() -> bool {
    let poller = get_poller();

    if !poller.running.load(Ordering::SeqCst) {
        return false;
    }

    if let Some(tx) = poller.stop_signal.lock().await.take() {
        let _ = tx.send(true);
    }
    poller.running.store(false, Ordering::SeqCst);
    println!("[UsagePoller] Stopped");
    true
}

/// Check if the poller is currently active.
pub fn is_running() -> bool {
    get_poller().running.load(Ordering::SeqCst)
}

async fn run_poll_loop(app_handle: AppHandle, mut stop_rx: tokio::sync::watch::Receiver<bool>) {
    loop {
        let interval = {
            let minutes = get_poller().interval_minutes.lock().await;
            Duration::from_secs(*minutes * 60)
        };

        // Wait for the interval OR a stop signal
        tokio::select! {
            _ = sleep(interval) => {}
            _ = stop_rx.changed() => {
                println!("[UsagePoller] Received stop signal");
                break;
            }
        }

        // Check if we should stop
        if *stop_rx.borrow() {
            break;
        }

        println!("[UsagePoller] Auto-polling usage for all accounts...");

        let accounts = match load_accounts() {
            Ok(store) => store.accounts,
            Err(err) => {
                println!("[UsagePoller] Failed to load accounts: {err}");
                continue;
            }
        };

        // Only poll ChatGPT accounts (API key accounts don't have usage info)
        let chatgpt_accounts: Vec<_> = accounts
            .into_iter()
            .filter(|a| matches!(a.auth_data, AuthData::ChatGPT { .. }))
            .collect();

        if chatgpt_accounts.is_empty() {
            continue;
        }

        let results = refresh_all_usage(&chatgpt_accounts).await;

        // Emit event to frontend
        if let Err(err) = app_handle.emit("usage-auto-refreshed", &results) {
            println!("[UsagePoller] Failed to emit event: {err}");
        } else {
            println!(
                "[UsagePoller] Emitted usage-auto-refreshed ({} accounts)",
                results.len()
            );
        }
    }

    get_poller().running.store(false, Ordering::SeqCst);
    println!("[UsagePoller] Loop exited");
}
