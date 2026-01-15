//! OAuth login Tauri commands

use std::sync::Mutex;
use tokio::sync::oneshot;

use crate::auth::oauth_server::{start_oauth_login, wait_for_oauth_login, OAuthLoginResult};
use crate::auth::{
    add_account, load_accounts, set_active_account, switch_to_account, touch_account,
};
use crate::types::{AccountInfo, OAuthLoginInfo};

// Global state for pending OAuth login
static PENDING_OAUTH: Mutex<Option<oneshot::Receiver<anyhow::Result<OAuthLoginResult>>>> =
    Mutex::new(None);

/// Start the OAuth login flow
#[tauri::command]
pub async fn start_login(account_name: String) -> Result<OAuthLoginInfo, String> {
    let (info, rx) = start_oauth_login(account_name)
        .await
        .map_err(|e| e.to_string())?;

    // Store the receiver for later
    {
        let mut pending = PENDING_OAUTH.lock().unwrap();
        *pending = Some(rx);
    }

    Ok(info)
}

/// Wait for the OAuth login to complete and add the account
#[tauri::command]
pub async fn complete_login() -> Result<AccountInfo, String> {
    let rx = {
        let mut pending = PENDING_OAUTH.lock().unwrap();
        pending
            .take()
            .ok_or_else(|| "No pending OAuth login".to_string())?
    };

    let account = wait_for_oauth_login(rx).await.map_err(|e| e.to_string())?;

    // Add the account to storage
    let stored = add_account(account).map_err(|e| e.to_string())?;

    // Make it active and switch to it
    set_active_account(&stored.id).map_err(|e| e.to_string())?;
    switch_to_account(&stored).map_err(|e| e.to_string())?;
    touch_account(&stored.id).map_err(|e| e.to_string())?;

    let store = load_accounts().map_err(|e| e.to_string())?;
    let active_id = store.active_account_id.as_deref();

    Ok(AccountInfo::from_stored(&stored, active_id))
}

/// Cancel a pending OAuth login
#[tauri::command]
pub async fn cancel_login() -> Result<(), String> {
    let mut pending = PENDING_OAUTH.lock().unwrap();
    *pending = None;
    Ok(())
}
