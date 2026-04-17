//! Auto-switch background monitor for automatic account rotation
//!
//! This module provides background monitoring that automatically switches
//! to a different account when the current account's usage exceeds a threshold.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::RwLock;
use std::time::Duration;

use tokio::time::sleep;

use crate::api::usage::get_account_usage;
use crate::auth::{get_active_account, load_accounts, set_active_account, switch_to_account};
use crate::auth::storage::load_auto_switch_config;
use crate::types::{AutoSwitchConfig, AutoSwitchEvent, AutoSwitchReason, UsageInfo};

/// Global state for the auto-switch monitor
static MONITOR_RUNNING: AtomicBool = AtomicBool::new(false);
static MONITOR_STOP: AtomicBool = AtomicBool::new(false);

/// Store recent switch events for UI display
static RECENT_EVENTS: RwLock<Vec<AutoSwitchEvent>> = RwLock::new(Vec::new());

/// Maximum number of recent events to keep
const MAX_RECENT_EVENTS: usize = 10;

/// Get the recent auto-switch events
pub fn get_recent_events() -> Vec<AutoSwitchEvent> {
    match RECENT_EVENTS.read() {
        Ok(events) => events.clone(),
        Err(_) => Vec::new(),
    }
}

/// Clear recent events
pub fn clear_recent_events() {
    if let Ok(mut events) = RECENT_EVENTS.write() {
        events.clear();
    }
}

/// Check if the monitor is currently running
pub fn is_monitor_running() -> bool {
    MONITOR_RUNNING.load(Ordering::SeqCst)
}

/// Start the auto-switch monitor (idempotent)
pub async fn start_auto_switch_monitor() -> Result<(), String> {
    if MONITOR_RUNNING.load(Ordering::SeqCst) {
        println!("[AutoSwitch] Monitor already running");
        return Ok(());
    }

    let config = load_auto_switch_config().map_err(|e| e.to_string())?;

    if !config.enabled {
        println!("[AutoSwitch] Monitor not started: disabled in config");
        return Ok(());
    }

    MONITOR_STOP.store(false, Ordering::SeqCst);
    MONITOR_RUNNING.store(true, Ordering::SeqCst);

    println!("[AutoSwitch] Starting monitor with interval {}s, threshold {}%",
        config.check_interval_seconds,
        config.threshold_percent
    );

    tokio::spawn(async move {
        run_monitor_loop().await;
    });

    Ok(())
}

/// Stop the auto-switch monitor
pub fn stop_auto_switch_monitor() {
    MONITOR_STOP.store(true, Ordering::SeqCst);
    MONITOR_RUNNING.store(false, Ordering::SeqCst);
}

/// Reload config and restart monitor if needed
pub async fn reload_monitor_config() -> Result<(), String> {
    let config = load_auto_switch_config().map_err(|e| e.to_string())?;

    if config.enabled && !MONITOR_RUNNING.load(Ordering::SeqCst) {
        // Config says enabled but monitor not running - start it
        start_auto_switch_monitor().await?;
    } else if !config.enabled && MONITOR_RUNNING.load(Ordering::SeqCst) {
        // Config says disabled but monitor running - stop it
        stop_auto_switch_monitor();
    }

    Ok(())
}

/// Main monitor loop
async fn run_monitor_loop() {
    println!("[AutoSwitch] Monitor loop started");

    loop {
        // Check if we should stop
        if MONITOR_STOP.load(Ordering::SeqCst) {
            println!("[AutoSwitch] Monitor stopping");
            break;
        }

        // Get current config
        let config = match load_auto_switch_config() {
            Ok(c) => c,
            Err(e) => {
                println!("[AutoSwitch] Failed to load config: {}", e);
                sleep(Duration::from_secs(10)).await;
                continue;
            }
        };

        // Skip this iteration if disabled
        if !config.enabled {
            println!("[AutoSwitch] Monitor paused (disabled in config)");
            MONITOR_RUNNING.store(false, Ordering::SeqCst);
            break;
        }

        // Check if a Codex process is running (don't switch during active use)
        if is_codex_running() {
            println!("[AutoSwitch] Codex process detected, skipping check");
            sleep(Duration::from_secs(config.check_interval_seconds)).await;
            continue;
        }

        // Check usage and potentially switch
        if let Err(e) = check_and_switch(&config).await {
            println!("[AutoSwitch] Error in check_and_switch: {}", e);
        }

        // Wait for next interval
        sleep(Duration::from_secs(config.check_interval_seconds)).await;
    }

    MONITOR_RUNNING.store(false, Ordering::SeqCst);
    println!("[AutoSwitch] Monitor stopped");
}

/// Check if a Codex process is running
fn is_codex_running() -> bool {
    // Use the same process detection as the UI
    #[cfg(unix)]
    {
        use std::process::Command;
        let output = Command::new("ps")
            .args(["-eo", "pid,command"])
            .output();

        if let Ok(output) = output {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines().skip(1) {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }

                if let Some((pid_str, command)) = line.split_once(' ') {
                    let command = command.trim();
                    let executable = command.split_whitespace().next().unwrap_or("");

                    let is_codex = executable == "codex" || executable.ends_with("/codex");
                    let is_switcher = command.contains("codex-switcher") ||
                                     command.contains("Codex Switcher");
                    let is_ide_plugin = command.contains(".antigravity") ||
                                       command.contains("openai.chatgpt");

                    if is_codex && !is_switcher && !is_ide_plugin {
                        if let Ok(pid) = pid_str.trim().parse::<u32>() {
                            if pid != std::process::id() {
                                return true;
                            }
                        }
                    }
                }
            }
        }
        false
    }

    #[cfg(windows)]
    {
        // On Windows, use tasklist
        use std::process::Command;
        let output = Command::new("tasklist")
            .args(["/FI", "IMAGENAME eq codex.exe", "/FO", "CSV", "/NH"])
            .output();

        if let Ok(output) = output {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                if line.contains("codex.exe") {
                    return true;
                }
            }
        }
        false
    }

    #[cfg(not(any(unix, windows)))]
    false
}

/// Check usage and switch accounts if needed
async fn check_and_switch(config: &AutoSwitchConfig) -> Result<(), String> {
    // Get active account
    let active_account = get_active_account()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "No active account".to_string())?;

    // Check if this account is excluded
    if config.excluded_account_ids.contains(&active_account.id) {
        println!("[AutoSwitch] Active account {} is excluded from auto-switch", active_account.name);
        return Ok(());
    }

    // Get usage for active account
    let usage = get_account_usage(&active_account)
        .await
        .map_err(|e| e.to_string())?;

    // Check if we need to switch
    let (should_switch, reason, trigger_percent) = should_switch_account(&usage, config);

    if !should_switch {
        println!("[AutoSwitch] Usage check: primary {:.1}%, weekly {:.1}% - no switch needed",
            usage.primary_used_percent.unwrap_or(-1.0),
            usage.secondary_used_percent.unwrap_or(-1.0)
        );
        return Ok(());
    }

    println!("[AutoSwitch] Switch trigger: {:?} at {:.1}%", reason, trigger_percent);

    // Find the next account to switch to
    let next_account = find_next_account(&active_account.id, config).await?;

    if next_account.id == active_account.id {
        println!("[AutoSwitch] No alternative account available");
        return Ok(());
    }

    println!("[AutoSwitch] Auto-switching from {} to {}",
        active_account.name,
        next_account.name
    );

    // Perform the switch
    switch_to_account(&next_account)
        .map_err(|e| e.to_string())?;
    set_active_account(&next_account.id)
        .map_err(|e| e.to_string())?;

    // Record the event
    let event = AutoSwitchEvent {
        timestamp: chrono::Utc::now().timestamp(),
        from_account_id: active_account.id.clone(),
        to_account_id: next_account.id.clone(),
        reason,
        triggered_at_percent: trigger_percent,
    };

    if let Ok(mut events) = RECENT_EVENTS.write() {
        events.insert(0, event);
        if events.len() > MAX_RECENT_EVENTS {
            events.truncate(MAX_RECENT_EVENTS);
        }
    }

    Ok(())
}

/// Determine if we should switch accounts based on usage thresholds
fn should_switch_account(usage: &UsageInfo, config: &AutoSwitchConfig) -> (bool, AutoSwitchReason, f64) {
    let threshold = config.threshold_percent;

    let primary_exceeded = usage.primary_used_percent
        .map(|p| p >= threshold)
        .unwrap_or(false);

    let secondary_exceeded = if config.respect_weekly_limit {
        usage.secondary_used_percent
            .map(|p| p >= threshold)
            .unwrap_or(false)
    } else {
        false
    };

    // Determine the reason and trigger percent
    match (primary_exceeded, secondary_exceeded) {
        (true, true) => {
            let trigger_percent = usage.primary_used_percent.unwrap_or(threshold)
                .max(usage.secondary_used_percent.unwrap_or(threshold));
            (true, AutoSwitchReason::BothLimitsReached, trigger_percent)
        }
        (true, false) => {
            let trigger_percent = usage.primary_used_percent.unwrap_or(threshold);
            (true, AutoSwitchReason::PrimaryLimitReached, trigger_percent)
        }
        (false, true) => {
            let trigger_percent = usage.secondary_used_percent.unwrap_or(threshold);
            (true, AutoSwitchReason::WeeklyLimitReached, trigger_percent)
        }
        (false, false) => (false, AutoSwitchReason::PrimaryLimitReached, 0.0),
    }
}

/// Find the next account to switch to
async fn find_next_account(
    exclude_id: &str,
    config: &AutoSwitchConfig,
) -> Result<crate::types::StoredAccount, String> {
    let store = load_accounts().map_err(|e| e.to_string())?;

    if store.accounts.len() <= 1 {
        return Ok(store.accounts.into_iter().next()
            .ok_or_else(|| "No accounts available".to_string())?);
    }

    // Filter out excluded accounts and the current one
    let candidates: Vec<_> = store.accounts.iter()
        .filter(|a| a.id != exclude_id)
        .filter(|a| !config.excluded_account_ids.contains(&a.id))
        .collect();

    if candidates.is_empty() {
        println!("[AutoSwitch] No candidate accounts after filtering");
        return Ok(store.accounts.into_iter().next()
            .ok_or_else(|| "No accounts available".to_string())?);
    }

    // If priority order is set, try those first
    if !config.priority_order.is_empty() {
        for priority_id in &config.priority_order {
            if let Some(account) = candidates.iter().find(|a| a.id == *priority_id) {
                // Check if this account has remaining quota
                if let Ok(usage) = get_account_usage(account).await {
                    if has_remaining_quota(&usage, config) {
                        println!("[AutoSwitch] Found priority account: {}", account.name);
                        return Ok((*account).clone());
                    }
                }
            }
        }
    }

    // Otherwise, find the account with most remaining quota
    let mut best_account: Option<crate::types::StoredAccount> = None;
    let mut best_remaining: f64 = -1.0;

    for account in &candidates {
        match get_account_usage(account).await {
            Ok(usage) => {
                let remaining = calculate_remaining_quota(&usage, config);
                println!("[AutoSwitch] Account {} has {:.1}% remaining", account.name, remaining);

                if remaining > best_remaining {
                    best_remaining = remaining;
                    best_account = Some((*account).clone());
                }
            }
            Err(e) => {
                println!("[AutoSwitch] Failed to get usage for {}: {}", account.name, e);
            }
        }
    }

    // Fallback to first candidate if no usage data available
    Ok(best_account.unwrap_or_else(|| candidates[0].clone()))
}

/// Check if an account has remaining quota
fn has_remaining_quota(usage: &UsageInfo, config: &AutoSwitchConfig) -> bool {
    let threshold = config.threshold_percent;

    let primary_ok = usage.primary_used_percent
        .map(|p| p < threshold)
        .unwrap_or(true);

    let secondary_ok = if config.respect_weekly_limit {
        usage.secondary_used_percent
            .map(|p| p < threshold)
            .unwrap_or(true)
    } else {
        true
    };

    primary_ok && secondary_ok
}

/// Calculate remaining quota percentage (higher is better)
fn calculate_remaining_quota(usage: &UsageInfo, config: &AutoSwitchConfig) -> f64 {
    let primary_remaining = usage.primary_used_percent
        .map(|p| 100.0 - p)
        .unwrap_or(100.0);

    let secondary_remaining = if config.respect_weekly_limit {
        usage.secondary_used_percent
            .map(|p| 100.0 - p)
            .unwrap_or(100.0)
    } else {
        100.0
    };

    // Return the minimum remaining, penalized if either is exhausted
    primary_remaining.min(secondary_remaining)
}