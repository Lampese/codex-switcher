//! OAuth login Tauri commands

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

use crate::auth::oauth_server::{start_oauth_login, wait_for_oauth_login, OAuthLoginResult};
use crate::auth::{
    add_account, load_accounts, save_accounts, switch_to_account, AUTH_OPERATION_LOCK,
};
use crate::types::{
    parse_chatgpt_id_token_claims, AccountInfo, AuthData, OAuthLoginInfo, StoredAccount,
};

struct PendingOAuth {
    rx: Option<oneshot::Receiver<anyhow::Result<OAuthLoginResult>>>,
    cancelled: Arc<AtomicBool>,
    reauth_account_id: Option<String>,
}

static OAUTH_START_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static PENDING_OAUTH: Mutex<Option<PendingOAuth>> = Mutex::new(None);

/// Start an add-account or targeted re-login flow.
#[tauri::command]
pub async fn start_login(
    account_name: String,
    reauth_account_id: Option<String>,
) -> Result<OAuthLoginInfo, String> {
    let _start_guard = OAUTH_START_LOCK.lock().await;
    if let Some(id) = &reauth_account_id {
        let store = load_accounts().map_err(|e| e.to_string())?;
        let account = store
            .accounts
            .iter()
            .find(|a| &a.id == id)
            .ok_or("Account no longer exists")?;
        if !matches!(account.auth_data, AuthData::ChatGPT { .. }) {
            return Err("Only ChatGPT accounts support re-login".into());
        }
        if store.active_account_id.as_ref() == Some(id) {
            ensure_active_relogin_safe()?;
        }
    }

    if let Some(previous) = PENDING_OAUTH.lock().unwrap().take() {
        previous.cancelled.store(true, Ordering::Relaxed);
    }
    let (info, rx, cancelled) = start_oauth_login(account_name.trim().to_string())
        .await
        .map_err(|e| e.to_string())?;
    *PENDING_OAUTH.lock().unwrap() = Some(PendingOAuth {
        rx: Some(rx),
        cancelled,
        reauth_account_id,
    });
    Ok(info)
}

fn ensure_active_relogin_safe() -> Result<(), String> {
    super::process::ensure_codex_not_running().map_err(|_| {
        "Close Codex or switch to another account before re-logging in to the active account."
            .into()
    })
}

/// Match both the user and workspace; workspace IDs alone can be shared by a team.
fn validate_relogin_identity(old: &StoredAccount, new: &StoredAccount) -> Result<(), String> {
    let (
        AuthData::ChatGPT {
            id_token: old_token,
            account_id: old_id,
            ..
        },
        AuthData::ChatGPT {
            id_token: new_token,
            account_id: new_id,
            ..
        },
    ) = (&old.auth_data, &new.auth_data)
    else {
        return Err("Only ChatGPT accounts support re-login".into());
    };
    let old_claims = parse_chatgpt_id_token_claims(old_token);
    let new_claims = parse_chatgpt_id_token_claims(new_token);
    let old_workspace = old_claims.account_id.as_ref().or(old_id.as_ref());
    let new_workspace = new_claims.account_id.as_ref().or(new_id.as_ref());
    let same_user = match (old_claims.subject.as_deref(), new_claims.subject.as_deref()) {
        (Some(a), Some(b)) if !a.is_empty() && !b.is_empty() => a == b,
        _ => match (old.email.as_deref(), new.email.as_deref()) {
            (Some(a), Some(b)) if !a.trim().is_empty() && !b.trim().is_empty() => {
                a.eq_ignore_ascii_case(b)
            }
            _ => false,
        },
    };
    if !same_user || old_workspace.is_none_or(|id| id.is_empty()) || old_workspace != new_workspace
    {
        return Err("Login does not match this account and workspace. Sign in with the original account and select the same workspace.".into());
    }
    Ok(())
}

fn save_relogin(id: &str, new: StoredAccount) -> Result<StoredAccount, String> {
    save_relogin_with_check(id, new, ensure_active_relogin_safe)
}

fn save_relogin_with_check(
    id: &str,
    new: StoredAccount,
    check_active: impl FnOnce() -> Result<(), String>,
) -> Result<StoredAccount, String> {
    let mut store = load_accounts().map_err(|e| e.to_string())?;
    let active = store.active_account_id.as_deref() == Some(id);
    let account = store
        .accounts
        .iter_mut()
        .find(|a| a.id == id)
        .ok_or("Account no longer exists")?;
    validate_relogin_identity(account, &new)?;
    if active {
        check_active()?;
    }
    account.auth_data = new.auth_data;
    account.email = new.email;
    account.plan_type = new.plan_type;
    account.subscription_expires_at = new.subscription_expires_at;
    let updated = account.clone();
    if active {
        // Live auth is the source of truth during reconciliation. Write it first
        // so an I/O failure cannot later replace renewed tokens with stale ones.
        switch_to_account(&updated).map_err(|e| {
            format!("Could not update Codex auth; saved account was not changed: {e}")
        })?;
    }
    save_accounts(&store).map_err(|e| {
        if active {
            format!("Codex login renewed, but saving the account failed: {e}. Refresh usage to sync it again.")
        } else {
            e.to_string()
        }
    })?;
    Ok(updated)
}

/// Keep cancellation reachable while awaiting the browser and the auth lock.
#[tauri::command]
pub async fn complete_login() -> Result<AccountInfo, String> {
    let (rx, cancelled, reauth_account_id) = {
        let mut pending = PENDING_OAUTH.lock().unwrap();
        let pending = pending.as_mut().ok_or("No pending OAuth login")?;
        (
            pending
                .rx
                .take()
                .ok_or("OAuth login is already being completed")?,
            pending.cancelled.clone(),
            pending.reauth_account_id.clone(),
        )
    };
    let result = async {
        let account = wait_for_oauth_login(rx).await.map_err(|e| e.to_string())?;
        let _auth_guard = AUTH_OPERATION_LOCK.lock().await;
        if cancelled.load(Ordering::Relaxed) {
            return Err("OAuth login cancelled".into());
        }
        let stored = if let Some(id) = reauth_account_id {
            save_relogin(&id, account)?
        } else {
            add_account(account).map_err(|e| e.to_string())?
        };
        let store = load_accounts().map_err(|e| e.to_string())?;
        Ok(AccountInfo::from_stored(
            &stored,
            store.active_account_id.as_deref(),
        ))
    }
    .await;
    let mut pending = PENDING_OAUTH.lock().unwrap();
    if pending
        .as_ref()
        .is_some_and(|p| Arc::ptr_eq(&p.cancelled, &cancelled))
    {
        pending.take();
    }
    result
}

#[tauri::command]
pub async fn cancel_login() -> Result<(), String> {
    let _start_guard = OAUTH_START_LOCK.lock().await;
    if let Some(pending) = PENDING_OAUTH.lock().unwrap().take() {
        pending.cancelled.store(true, Ordering::Relaxed);
    }
    Ok(())
}

#[cfg(test)]
#[path = "oauth_tests.rs"]
mod tests;
