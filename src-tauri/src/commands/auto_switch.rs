//! Auto-switch Tauri commands

use crate::auto_switch::{get_recent_events, is_monitor_running, start_auto_switch_monitor, stop_auto_switch_monitor};
use crate::auth::storage::{load_auto_switch_config, save_auto_switch_config};
use crate::types::AutoSwitchConfig;

/// Get the current auto-switch configuration
#[tauri::command]
pub async fn get_auto_switch_config() -> Result<AutoSwitchConfig, String> {
    load_auto_switch_config().map_err(|e| e.to_string())
}

/// Update the auto-switch configuration
#[tauri::command]
pub async fn set_auto_switch_config(config: AutoSwitchConfig) -> Result<(), String> {
    // Save the config
    save_auto_switch_config(&config).map_err(|e| e.to_string())?;

    // Reload monitor with new config
    if config.enabled {
        // Stop existing monitor first
        stop_auto_switch_monitor();
        // Start with new config (small delay to ensure clean stop)
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        start_auto_switch_monitor().await?;
    } else {
        stop_auto_switch_monitor();
    }

    Ok(())
}

/// Start the auto-switch monitor manually
#[tauri::command]
pub async fn start_auto_switch() -> Result<(), String> {
    // Update config to enable
    let mut config = load_auto_switch_config().map_err(|e| e.to_string())?;
    config.enabled = true;
    save_auto_switch_config(&config).map_err(|e| e.to_string())?;

    start_auto_switch_monitor().await
}

/// Stop the auto-switch monitor manually
#[tauri::command]
pub async fn stop_auto_switch() -> Result<(), String> {
    // Update config to disable
    let mut config = load_auto_switch_config().map_err(|e| e.to_string())?;
    config.enabled = false;
    save_auto_switch_config(&config).map_err(|e| e.to_string())?;

    stop_auto_switch_monitor();
    Ok(())
}

/// Check if the auto-switch monitor is running
#[tauri::command]
pub fn auto_switch_status() -> Result<bool, String> {
    Ok(is_monitor_running())
}

/// Get recent auto-switch events
#[tauri::command]
pub async fn get_auto_switch_events() -> Result<Vec<crate::types::AutoSwitchEvent>, String> {
    Ok(get_recent_events())
}

/// Clear auto-switch event history
#[tauri::command]
pub fn clear_auto_switch_events() -> Result<(), String> {
    crate::auto_switch::clear_recent_events();
    Ok(())
}
