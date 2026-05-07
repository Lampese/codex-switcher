//! OAuth login Tauri commands

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

use crate::auth::device_code::{
    start_device_code_login, wait_for_device_code_login, DeviceLoginResult,
};
use crate::auth::oauth_server::{start_oauth_login, wait_for_oauth_login, OAuthLoginResult};
use crate::auth::{
    add_account, load_accounts, set_active_account, switch_to_account, touch_account,
};
use crate::types::{AccountInfo, DeviceLoginInfo, OAuthLoginInfo};

struct PendingOAuth {
    rx: Option<oneshot::Receiver<anyhow::Result<OAuthLoginResult>>>,
    cancelled: Arc<AtomicBool>,
}

struct PendingDevice {
    rx: Option<oneshot::Receiver<anyhow::Result<DeviceLoginResult>>>,
    cancelled: Arc<AtomicBool>,
}

// Global state for pending OAuth login
static PENDING_OAUTH: Mutex<Option<PendingOAuth>> = Mutex::new(None);
static PENDING_DEVICE: Mutex<Option<PendingDevice>> = Mutex::new(None);

fn cancel_pending_oauth() {
    if let Some(previous) = {
        let mut pending = PENDING_OAUTH.lock().unwrap();
        pending.take()
    } {
        previous.cancelled.store(true, Ordering::Relaxed);
    }
}

fn cancel_pending_device() {
    if let Some(previous) = {
        let mut pending = PENDING_DEVICE.lock().unwrap();
        pending.take()
    } {
        previous.cancelled.store(true, Ordering::Relaxed);
    }
}

fn store_completed_account(account: crate::types::StoredAccount) -> Result<AccountInfo, String> {
    let stored = add_account(account).map_err(|e| e.to_string())?;

    set_active_account(&stored.id).map_err(|e| e.to_string())?;
    switch_to_account(&stored).map_err(|e| e.to_string())?;
    touch_account(&stored.id).map_err(|e| e.to_string())?;

    let store = load_accounts().map_err(|e| e.to_string())?;
    let active_id = store.active_account_id.as_deref();

    Ok(AccountInfo::from_stored(&stored, active_id))
}

/// Start the OAuth login flow
#[tauri::command]
pub async fn start_login(account_name: String) -> Result<OAuthLoginInfo, String> {
    // Cancel any previous pending flow so it does not keep the callback port occupied.
    cancel_pending_oauth();
    cancel_pending_device();

    let (info, rx, cancelled) = start_oauth_login(account_name)
        .await
        .map_err(|e| e.to_string())?;

    // Store the receiver for later
    {
        let mut pending = PENDING_OAUTH.lock().unwrap();
        *pending = Some(PendingOAuth {
            rx: Some(rx),
            cancelled,
        });
    }

    Ok(info)
}

/// Wait for the OAuth login to complete and add the account
#[tauri::command]
pub async fn complete_login() -> Result<AccountInfo, String> {
    let (rx, cancelled) = {
        let mut pending = PENDING_OAUTH.lock().unwrap();
        let pending = pending
            .as_mut()
            .ok_or_else(|| "No pending OAuth login".to_string())?;
        let rx = pending
            .rx
            .take()
            .ok_or_else(|| "OAuth login is already being completed".to_string())?;
        (rx, pending.cancelled.clone())
    };

    let account_result = wait_for_oauth_login(rx).await.map_err(|e| e.to_string());
    {
        let mut pending = PENDING_OAUTH.lock().unwrap();
        if pending
            .as_ref()
            .map(|current| Arc::ptr_eq(&current.cancelled, &cancelled))
            .unwrap_or(false)
        {
            pending.take();
        }
    }
    let account = account_result?;

    store_completed_account(account)
}

/// Start the ChatGPT device-code login flow.
#[tauri::command]
pub async fn start_device_login(account_name: String) -> Result<DeviceLoginInfo, String> {
    cancel_pending_oauth();
    cancel_pending_device();

    let (info, rx, cancelled) = start_device_code_login(account_name)
        .await
        .map_err(|e| e.to_string())?;

    {
        let mut pending = PENDING_DEVICE.lock().unwrap();
        *pending = Some(PendingDevice {
            rx: Some(rx),
            cancelled,
        });
    }

    Ok(info)
}

/// Wait for the device-code login to complete and add the account.
#[tauri::command]
pub async fn complete_device_login() -> Result<AccountInfo, String> {
    let (rx, cancelled) = {
        let mut pending = PENDING_DEVICE.lock().unwrap();
        let pending = pending
            .as_mut()
            .ok_or_else(|| "No pending device login".to_string())?;
        let rx = pending
            .rx
            .take()
            .ok_or_else(|| "Device login is already being completed".to_string())?;
        (rx, pending.cancelled.clone())
    };

    let account_result = wait_for_device_code_login(rx)
        .await
        .map_err(|e| e.to_string());
    {
        let mut pending = PENDING_DEVICE.lock().unwrap();
        if pending
            .as_ref()
            .map(|current| Arc::ptr_eq(&current.cancelled, &cancelled))
            .unwrap_or(false)
        {
            pending.take();
        }
    }
    let account = account_result?;

    store_completed_account(account)
}

/// Cancel a pending OAuth login
#[tauri::command]
pub async fn cancel_login() -> Result<(), String> {
    cancel_pending_oauth();
    cancel_pending_device();
    Ok(())
}
