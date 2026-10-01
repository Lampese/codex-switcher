//! OAuth login Tauri commands

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

use crate::auth::oauth_server::{start_oauth_login, wait_for_oauth_login, OAuthLoginResult};
use crate::auth::{
    add_or_replace_account, duplicate_account_requires_confirmation, load_accounts,
    set_active_account, switch_to_account, touch_account, ACCOUNT_REPLACE_CONFIRMATION_PREFIX,
    AUTH_OPERATION_LOCK,
};
use crate::types::{AccountInfo, OAuthLoginInfo, StoredAccount};

struct PendingOAuth {
    rx: oneshot::Receiver<anyhow::Result<OAuthLoginResult>>,
    cancelled: Arc<AtomicBool>,
}

// Global state for pending OAuth login. If login succeeds but replacing a
// healthy duplicate needs confirmation, keep the completed credentials in
// memory so the user can confirm without authenticating in the browser again.
static PENDING_OAUTH: Mutex<Option<PendingOAuth>> = Mutex::new(None);
static PENDING_COMPLETED_ACCOUNT: Mutex<Option<StoredAccount>> = Mutex::new(None);

/// Start the OAuth login flow
#[tauri::command]
pub async fn start_login(account_name: String) -> Result<OAuthLoginInfo, String> {
    // A new login supersedes any previously staged duplicate replacement.
    PENDING_COMPLETED_ACCOUNT.lock().unwrap().take();

    // Cancel any previous pending flow so it does not keep the callback port occupied.
    if let Some(previous) = {
        let mut pending = PENDING_OAUTH.lock().unwrap();
        pending.take()
    } {
        previous.cancelled.store(true, Ordering::Relaxed);
    }

    let (info, rx, cancelled) = start_oauth_login(account_name.trim().to_string())
        .await
        .map_err(|e| e.to_string())?;

    // Store the receiver for later
    {
        let mut pending = PENDING_OAUTH.lock().unwrap();
        *pending = Some(PendingOAuth { rx, cancelled });
    }

    Ok(info)
}

/// Wait for the OAuth login to complete and add the account.
///
/// Duplicates with a stale OAuth session are replaced automatically. A healthy
/// duplicate is staged in memory and returns ACCOUNT_REPLACE_CONFIRMATION_REQUIRED:<name>;
/// the UI can retry with force_replace=true without making the user log in again.
#[tauri::command]
pub async fn complete_login(force_replace: Option<bool>) -> Result<AccountInfo, String> {
    let force_replace = force_replace.unwrap_or(false);

    let staged_account = {
        let mut staged = PENDING_COMPLETED_ACCOUNT.lock().unwrap();
        staged.take()
    };

    let account = if let Some(account) = staged_account {
        account
    } else {
        let pending = {
            let mut pending = PENDING_OAUTH.lock().unwrap();
            pending
                .take()
                .ok_or_else(|| "No pending OAuth login".to_string())?
        };

        wait_for_oauth_login(pending.rx)
            .await
            .map_err(|e| e.to_string())?
    };

    if !force_replace
        && duplicate_account_requires_confirmation(&account)
            .await
            .map_err(|e| e.to_string())?
    {
        let account_name = account.name.clone();
        PENDING_COMPLETED_ACCOUNT.lock().unwrap().replace(account);
        return Err(format!(
            "{ACCOUNT_REPLACE_CONFIRMATION_PREFIX}{account_name}"
        ));
    }

    let _auth_guard = AUTH_OPERATION_LOCK.lock().await;
    let stored = add_or_replace_account(account, true).map_err(|e| e.to_string())?;

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
    if let Some(pending_oauth) = pending.take() {
        pending_oauth.cancelled.store(true, Ordering::Relaxed);
    }
    drop(pending);

    PENDING_COMPLETED_ACCOUNT.lock().unwrap().take();
    Ok(())
}
