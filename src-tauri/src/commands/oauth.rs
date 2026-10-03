//! OAuth login Tauri commands

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

use anyhow::Context;

use crate::auth::oauth_server::{start_oauth_login, wait_for_oauth_login, OAuthLoginResult};
use crate::auth::{
    add_account, load_accounts, set_active_account, switch_to_account, touch_account,
    update_account_chatgpt_tokens, AUTH_OPERATION_LOCK,
};
use crate::types::{AccountInfo, AuthData, OAuthLoginInfo, StoredAccount};

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

/// Wait for the OAuth login to complete and add the account
#[tauri::command]
pub async fn complete_login() -> Result<AccountInfo, String> {
    let pending = take_pending_oauth()?;

    let account = wait_for_oauth_login(pending.rx)
        .await
        .map_err(|e| e.to_string())?;

    let _auth_guard = AUTH_OPERATION_LOCK.lock().await;

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

/// Wait for the OAuth login to complete and store its tokens on an existing account.
/// Used when the refresh token of that account is no longer accepted.
#[tauri::command]
pub async fn complete_reauthorize(account_id: String) -> Result<AccountInfo, String> {
    let pending = take_pending_oauth()?;

    let signed_in = wait_for_oauth_login(pending.rx)
        .await
        .map_err(|e| e.to_string())?;

    let _auth_guard = AUTH_OPERATION_LOCK.lock().await;

    let updated =
        store_reauthorized_tokens(&account_id, &signed_in).map_err(|e| format!("{e:#}"))?;

    let store = load_accounts().map_err(|e| e.to_string())?;
    let active_id = store.active_account_id.as_deref();
    if active_id == Some(account_id.as_str()) {
        switch_to_account(&updated).map_err(|e| e.to_string())?;
    }

    Ok(AccountInfo::from_stored(&updated, active_id))
}

fn take_pending_oauth() -> Result<PendingOAuth, String> {
    let mut pending = PENDING_OAUTH.lock().unwrap();
    pending
        .take()
        .ok_or_else(|| "No pending OAuth login".to_string())
}

fn store_reauthorized_tokens(
    account_id: &str,
    signed_in: &StoredAccount,
) -> anyhow::Result<StoredAccount> {
    let store = load_accounts()?;
    let expected = store
        .accounts
        .iter()
        .find(|account| account.id == account_id)
        .context("Account not found")?;
    ensure_same_chatgpt_account(expected, signed_in)?;

    let AuthData::ChatGPT {
        id_token,
        access_token,
        refresh_token,
        account_id: chatgpt_account_id,
    } = &signed_in.auth_data
    else {
        anyhow::bail!("The sign-in did not return ChatGPT tokens");
    };

    update_account_chatgpt_tokens(
        account_id,
        id_token.clone(),
        access_token.clone(),
        refresh_token.clone(),
        chatgpt_account_id.clone(),
        signed_in.email.clone(),
        signed_in.plan_type.clone(),
        signed_in.subscription_expires_at,
    )
}

/// Make sure a new sign-in belongs to the account whose tokens it replaces.
fn ensure_same_chatgpt_account(
    expected: &StoredAccount,
    signed_in: &StoredAccount,
) -> anyhow::Result<()> {
    let (
        AuthData::ChatGPT {
            account_id: expected_workspace,
            ..
        },
        AuthData::ChatGPT {
            account_id: signed_in_workspace,
            ..
        },
    ) = (&expected.auth_data, &signed_in.auth_data)
    else {
        anyhow::bail!("Only ChatGPT accounts can sign in again");
    };

    let same_email = match (&expected.email, &signed_in.email) {
        (Some(expected), Some(signed_in)) => Some(expected.eq_ignore_ascii_case(signed_in)),
        _ => None,
    };
    let same_workspace = match (expected_workspace, signed_in_workspace) {
        (Some(expected), Some(signed_in)) => Some(expected == signed_in),
        _ => None,
    };

    match (same_email, same_workspace) {
        (Some(false), _) | (_, Some(false)) => anyhow::bail!(
            "Signed in as {}, but this account is {}. Sign in to the same ChatGPT account and workspace.",
            signed_in.email.as_deref().unwrap_or("another account"),
            expected.email.as_deref().unwrap_or(&expected.name),
        ),
        (None, None) => anyhow::bail!(
            "Could not confirm that the sign-in belongs to {}",
            expected.name
        ),
        _ => Ok(()),
    }
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
    use super::ensure_same_chatgpt_account;
    use crate::types::StoredAccount;

    fn chatgpt(email: Option<&str>, account_id: Option<&str>) -> StoredAccount {
        StoredAccount::new_chatgpt(
            "Work".into(),
            email.map(String::from),
            None,
            None,
            "id".into(),
            "access".into(),
            "refresh".into(),
            account_id.map(String::from),
        )
    }

    #[test]
    fn accepts_a_sign_in_to_the_same_chatgpt_account() {
        let expected = chatgpt(Some("me@example.com"), Some("acct-1"));
        let signed_in = chatgpt(Some("Me@Example.com"), Some("acct-1"));

        assert!(ensure_same_chatgpt_account(&expected, &signed_in).is_ok());
    }

    #[test]
    fn accepts_a_matching_email_when_the_workspace_is_unknown() {
        let expected = chatgpt(Some("me@example.com"), None);
        let signed_in = chatgpt(Some("me@example.com"), Some("acct-1"));

        assert!(ensure_same_chatgpt_account(&expected, &signed_in).is_ok());
    }

    #[test]
    fn rejects_a_sign_in_to_another_workspace() {
        let expected = chatgpt(Some("me@example.com"), Some("acct-1"));
        let signed_in = chatgpt(Some("me@example.com"), Some("acct-2"));

        assert!(ensure_same_chatgpt_account(&expected, &signed_in).is_err());
    }

    #[test]
    fn rejects_a_sign_in_as_another_user() {
        let expected = chatgpt(Some("me@example.com"), None);
        let signed_in = chatgpt(Some("other@example.com"), None);

        let error = ensure_same_chatgpt_account(&expected, &signed_in).unwrap_err();
        assert!(error.to_string().contains("other@example.com"), "{error}");
    }

    #[test]
    fn rejects_a_sign_in_that_cannot_be_compared() {
        let expected = chatgpt(None, None);
        let signed_in = chatgpt(Some("me@example.com"), Some("acct-1"));

        assert!(ensure_same_chatgpt_account(&expected, &signed_in).is_err());
    }

    #[test]
    fn rejects_an_api_key_account() {
        let expected = StoredAccount::new_api_key("Key".into(), "sk-test".into());
        let signed_in = chatgpt(Some("me@example.com"), Some("acct-1"));

        assert!(ensure_same_chatgpt_account(&expected, &signed_in).is_err());
    }
}
