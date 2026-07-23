//! OAuth login Tauri commands

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

use crate::auth::oauth_server::{start_oauth_login, wait_for_oauth_login, OAuthLoginResult};
use crate::auth::{
    add_account, load_accounts, set_active_account, switch_to_account, touch_account,
};
use crate::types::{AccountInfo, OAuthLoginInfo};

struct PendingOAuth {
    rx: oneshot::Receiver<anyhow::Result<OAuthLoginResult>>,
    cancelled: Arc<AtomicBool>,
}

// Global state for pending OAuth login
static PENDING_OAUTH: Mutex<Option<PendingOAuth>> = Mutex::new(None);

/// Start the OAuth login flow
#[tauri::command]
pub async fn start_login(account_name: String) -> Result<OAuthLoginInfo, String> {
    // Cancel any previous pending flow so it does not keep the callback port occupied.
    if let Some(previous) = {
        let mut pending = PENDING_OAUTH.lock().unwrap();
        pending.take()
    } {
        previous.cancelled.store(true, Ordering::Relaxed);
    }

    let (info, rx, cancelled) = start_oauth_login(account_name)
        .await
        .map_err(|e| e.to_string())?;

    // Store the receiver for later
    {
        let mut pending = PENDING_OAUTH.lock().unwrap();
        *pending = Some(PendingOAuth { rx, cancelled });
    }

    Ok(info)
}

/// Wait for the OAuth login to complete and add the account
#[tauri::command]
pub async fn complete_login() -> Result<AccountInfo, String> {
    let pending = {
        let mut pending = PENDING_OAUTH.lock().unwrap();
        pending
            .take()
            .ok_or_else(|| "No pending OAuth login".to_string())?
    };

    let account = wait_for_oauth_login(pending.rx)
        .await
        .map_err(|e| e.to_string())?;

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
    if let Some(pending_oauth) = pending.take() {
        pending_oauth.cancelled.store(true, Ordering::Relaxed);
    }
    Ok(())
}

/// Start an OAuth re-authentication flow for an existing account.
/// Same as start_login but does not create a new account — used to
/// replace stale tokens on an account that has expired credentials.
#[tauri::command]
pub async fn start_reauth(account_id: String) -> Result<OAuthLoginInfo, String> {
    // Reuse the same pending-oauth slot; cancel any previous flow first.
    if let Some(previous) = {
        let mut pending = PENDING_OAUTH.lock().unwrap();
        pending.take()
    } {
        previous.cancelled.store(true, Ordering::Relaxed);
    }

    // Load the account name so the OAuth flow can display it.
    let account_name = {
        use crate::auth::load_accounts;
        let store = load_accounts().map_err(|e| e.to_string())?;
        store
            .accounts
            .iter()
            .find(|a| a.id == account_id)
            .map(|a| a.name.clone())
            .ok_or_else(|| format!("Account not found: {account_id}"))?
    };

    let (info, rx, cancelled) = start_oauth_login(account_name)
        .await
        .map_err(|e| e.to_string())?;

    {
        let mut pending = PENDING_OAUTH.lock().unwrap();
        *pending = Some(PendingOAuth { rx, cancelled });
    }

    Ok(info)
}

/// Complete an OAuth re-authentication flow, updating the existing account's
/// tokens in-place rather than creating a new account entry.
#[tauri::command]
pub async fn complete_reauth(account_id: String) -> Result<AccountInfo, String> {
    use crate::auth::storage::update_account_chatgpt_tokens;
    use crate::types::{parse_chatgpt_id_token_claims, AuthData};

    let pending = {
        let mut pending = PENDING_OAUTH.lock().unwrap();
        pending
            .take()
            .ok_or_else(|| "No pending OAuth login".to_string())?
    };

    let fresh_account = wait_for_oauth_login(pending.rx)
        .await
        .map_err(|e| e.to_string())?;

    // Extract the new tokens from the OAuth result.
    let (id_token, access_token, refresh_token, chatgpt_account_id) = match &fresh_account.auth_data
    {
        AuthData::ChatGPT {
            id_token,
            access_token,
            refresh_token,
            account_id,
        } => (
            id_token.clone(),
            access_token.clone(),
            refresh_token.clone(),
            account_id.clone(),
        ),
        AuthData::ApiKey { .. } => {
            return Err("Re-auth returned an API key account unexpectedly".to_string())
        }
    };

    let claims = parse_chatgpt_id_token_claims(&id_token);

    // Update the existing account's tokens in-place, preserving its name and id.
    let updated = update_account_chatgpt_tokens(
        &account_id,
        id_token,
        access_token,
        refresh_token,
        chatgpt_account_id,
        claims.email,
        claims.plan_type,
        claims.subscription_expires_at,
    )
    .map_err(|e| e.to_string())?;

    // If this is the active account, sync auth.json too.
    let store = load_accounts().map_err(|e| e.to_string())?;
    if store.active_account_id.as_deref() == Some(&account_id) {
        switch_to_account(&updated).map_err(|e| e.to_string())?;
    }

    touch_account(&account_id).map_err(|e| e.to_string())?;

    let active_id = store.active_account_id.as_deref();
    Ok(AccountInfo::from_stored(&updated, active_id))
}
