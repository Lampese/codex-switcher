//! OAuth login Tauri commands

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

use crate::auth::oauth_server::{start_oauth_login, wait_for_oauth_login, OAuthLoginResult};
use crate::auth::{
    add_account, load_accounts, mutate_accounts, same_chatgpt_identity, set_active_account,
    switch_to_account, touch_account,
};
use crate::types::{AccountInfo, AuthData, AuthMode, AuthState, OAuthLoginInfo, StoredAccount};

use super::process::ensure_codex_not_running;

pub const AUTH_IDENTITY_MISMATCH_ERROR: &str = "AUTH_IDENTITY_MISMATCH";

fn reauthenticated_account(
    original: &StoredAccount,
    replacement: &StoredAccount,
) -> anyhow::Result<StoredAccount> {
    if original.auth_mode != AuthMode::ChatGPT || replacement.auth_mode != AuthMode::ChatGPT {
        anyhow::bail!("Only ChatGPT accounts can be reauthenticated");
    }
    if !same_chatgpt_identity(original, replacement) {
        anyhow::bail!(
            "{AUTH_IDENTITY_MISMATCH_ERROR}: sign in with the same ChatGPT account"
        );
    }
    if !matches!(&replacement.auth_data, AuthData::ChatGPT { .. }) {
        anyhow::bail!("OAuth did not return ChatGPT credentials");
    }

    let mut updated = original.clone();
    updated.email = replacement.email.clone();
    updated.plan_type = replacement.plan_type.clone();
    updated.subscription_expires_at = replacement.subscription_expires_at;
    updated.auth_data = replacement.auth_data.clone();
    updated.auth_state = AuthState::Ready;
    Ok(updated)
}

struct PendingOAuth {
    rx: oneshot::Receiver<anyhow::Result<OAuthLoginResult>>,
    cancelled: Arc<AtomicBool>,
    replace_account_id: Option<String>,
}

// Global state for pending OAuth login
static PENDING_OAUTH: Mutex<Option<PendingOAuth>> = Mutex::new(None);

/// Start the OAuth login flow
#[tauri::command]
pub async fn start_login(
    account_name: String,
    replace_account_id: Option<String>,
) -> Result<OAuthLoginInfo, String> {
    // Cancel any previous pending flow so it does not keep the callback port occupied.
    if let Some(previous) = {
        let mut pending = PENDING_OAUTH.lock().unwrap();
        pending.take()
    } {
        previous.cancelled.store(true, Ordering::Relaxed);
    }

    let resolved_name = if let Some(account_id) = replace_account_id.as_deref() {
        let store = load_accounts().map_err(|e| e.to_string())?;
        let account = store
            .accounts
            .iter()
            .find(|account| account.id == account_id)
            .ok_or_else(|| format!("Account not found: {account_id}"))?;
        if account.auth_mode != AuthMode::ChatGPT {
            return Err("Only ChatGPT accounts can be reauthenticated".to_string());
        }
        account.name.clone()
    } else {
        account_name.trim().to_string()
    };

    let (info, rx, cancelled) = start_oauth_login(resolved_name)
        .await
        .map_err(|e| e.to_string())?;

    // Store the receiver for later
    {
        let mut pending = PENDING_OAUTH.lock().unwrap();
        *pending = Some(PendingOAuth {
            rx,
            cancelled,
            replace_account_id,
        });
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

    if let Some(account_id) = pending.replace_account_id {
        let updated = mutate_accounts(|store| {
            let was_active = store.active_account_id.as_deref() == Some(account_id.as_str());
            if was_active {
                ensure_codex_not_running().map_err(anyhow::Error::msg)?;
            }

            let account_index = store
                .accounts
                .iter()
                .position(|stored| stored.id == account_id)
                .ok_or_else(|| anyhow::anyhow!("Account not found: {account_id}"))?;
            let updated = reauthenticated_account(&store.accounts[account_index], &account)?;

            // For an active account, verify the process guard and auth.json
            // write before committing any account-store changes.
            if was_active {
                switch_to_account(&updated)?;
            }
            store.accounts[account_index] = updated.clone();
            Ok(updated)
        })
        .map_err(|e| e.to_string())?;

        let refreshed_store = load_accounts().map_err(|e| e.to_string())?;
        return Ok(AccountInfo::from_stored(
            &updated,
            refreshed_store.active_account_id.as_deref(),
        ));
    }

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

#[cfg(test)]
mod tests {
    use super::{reauthenticated_account, AUTH_IDENTITY_MISMATCH_ERROR};
    use crate::types::{AuthData, AuthState, StoredAccount};

    fn account(email: &str, account_id: &str, suffix: &str) -> StoredAccount {
        StoredAccount::new_chatgpt(
            format!("name-{suffix}"),
            Some(email.into()),
            Some(format!("plan-{suffix}")),
            None,
            format!("id-{suffix}"),
            format!("access-{suffix}"),
            format!("refresh-{suffix}"),
            Some(account_id.into()),
        )
    }

    #[test]
    fn reauthentication_preserves_local_identity_and_replaces_profile() {
        let mut original = account("user@example.com", "account-1", "old");
        original.auth_state = AuthState::ReauthRequired;
        original.last_used_at = Some(chrono::Utc::now());
        original.subscription_expires_at = Some(chrono::Utc::now());
        let replacement = account("USER@example.com", "account-1", "new");
        let updated = reauthenticated_account(&original, &replacement).unwrap();

        assert_eq!(updated.id, original.id);
        assert_eq!(updated.name, original.name);
        assert_eq!(updated.created_at, original.created_at);
        assert_eq!(updated.last_used_at, original.last_used_at);
        assert_eq!(updated.email, replacement.email);
        assert_eq!(updated.plan_type, replacement.plan_type);
        assert_eq!(updated.subscription_expires_at, replacement.subscription_expires_at);
        assert_eq!(updated.auth_state, AuthState::Ready);
        assert!(matches!(
            updated.auth_data,
            AuthData::ChatGPT { ref refresh_token, .. } if refresh_token == "refresh-new"
        ));
    }

    #[test]
    fn reauthentication_rejects_a_different_identity_without_a_result() {
        let original = account("user@example.com", "account-1", "old");
        let replacement = account("other@example.com", "account-2", "new");
        let error = reauthenticated_account(&original, &replacement).unwrap_err();

        assert!(error.to_string().starts_with(AUTH_IDENTITY_MISMATCH_ERROR));
    }
}
