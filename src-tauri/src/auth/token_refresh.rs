//! ChatGPT OAuth token refresh helpers

use anyhow::{Context, Result};
use base64::Engine;
use chrono::Utc;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use tokio::{
    sync::Mutex,
    time::{sleep, Duration},
};

use super::{
    load_accounts, mutate_accounts, read_current_auth, switch_to_account,
};
use crate::commands::process::has_running_codex_processes;
use crate::types::{parse_chatgpt_id_token_claims, AuthData, AuthState, StoredAccount};

const DEFAULT_ISSUER: &str = "https://auth.openai.com";
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const EXPIRY_SKEW_SECONDS: i64 = 60;

pub const AUTH_REAUTH_REQUIRED_ERROR: &str = "AUTH_REAUTH_REQUIRED";
pub const AUTH_REFRESH_BLOCKED_ERROR: &str = "AUTH_REFRESH_BLOCKED_BY_CODEX";

type AccountRefreshLock = Arc<Mutex<()>>;

// Refresh tokens are rotated by the OAuth server. Several UI surfaces can ask
// for usage at the same time, so refreshing the same account concurrently can
// invalidate the token used by the slower request. Keep one lock per account
// and let later callers reuse the credentials written by the first refresh.
static ACCOUNT_REFRESH_LOCKS: OnceLock<Mutex<HashMap<String, AccountRefreshLock>>> =
    OnceLock::new();

#[derive(Debug, serde::Deserialize)]
struct RefreshTokenResponse {
    #[serde(default)]
    id_token: Option<String>,
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
}

#[derive(Debug, thiserror::Error)]
#[error("AUTH_REAUTH_REQUIRED: {code}")]
struct PermanentRefreshError {
    code: String,
}

/// Ensure the account has a non-expired ChatGPT access token.
/// Returns an updated account when a refresh was performed.
pub async fn ensure_chatgpt_tokens_fresh(account: &StoredAccount) -> Result<StoredAccount> {
    let current = reconcile_active_account_from_codex_auth(account)?;
    if current.auth_state == AuthState::ReauthRequired {
        anyhow::bail!("{AUTH_REAUTH_REQUIRED_ERROR}: session_expired");
    }

    match &current.auth_data {
        AuthData::ApiKey { .. } => Ok(current),
        AuthData::ChatGPT { access_token, .. } => {
            if token_expired_or_near_expiry(access_token) {
                println!(
                    "[Auth] Access token expired/near expiry for account {}, refreshing",
                    current.name
                );
                refresh_chatgpt_tokens(&current).await
            } else {
                Ok(current)
            }
        }
    }
}

/// Force-refresh ChatGPT OAuth tokens for an account.
pub async fn refresh_chatgpt_tokens(account: &StoredAccount) -> Result<StoredAccount> {
    let refresh_lock = account_refresh_lock(&account.id).await;
    let _guard = refresh_lock.lock().await;

    // Re-read both stores after acquiring the lock. Another UI request or the
    // active Codex process may already have rotated and persisted credentials.
    let latest_account = reconcile_active_account_from_codex_auth(account)?;

    if latest_account.auth_state == AuthState::ReauthRequired {
        anyhow::bail!("{AUTH_REAUTH_REQUIRED_ERROR}: session_expired");
    }

    if !same_chatgpt_session(account, &latest_account) {
        return Ok(latest_account);
    }

    let (current_id_token, current_refresh_token, current_account_id) =
        match &latest_account.auth_data {
            AuthData::ApiKey { .. } => return Ok(account.clone()),
            AuthData::ChatGPT {
                id_token,
                refresh_token,
                account_id,
                ..
            } => (id_token.clone(), refresh_token.clone(), account_id.clone()),
    };

    if current_refresh_token.is_empty() {
        let (current, marked) = mark_reauth_if_session_unchanged(&latest_account)?;
        if marked {
            anyhow::bail!("{AUTH_REAUTH_REQUIRED_ERROR}: missing_refresh_token");
        }
        return Ok(current);
    }

    let is_active = load_accounts()?.active_account_id.as_deref() == Some(account.id.as_str());
    let codex_running = if is_active {
        has_running_codex_processes().map_err(anyhow::Error::msg)?
    } else {
        false
    };
    if should_block_active_refresh(is_active, codex_running) {
        anyhow::bail!(
            "{AUTH_REFRESH_BLOCKED_ERROR}: close Codex before refreshing the active account"
        );
    }

    let refreshed = match refresh_tokens_with_refresh_token(&current_refresh_token).await {
        Ok(refreshed) => refreshed,
        Err(error) if error.downcast_ref::<PermanentRefreshError>().is_some() => {
            // Codex may have won a cross-process refresh race. Re-read auth.json
            // once and use it if the credentials changed before declaring the
            // session permanently expired.
            let reconciled = reconcile_active_account_from_codex_auth(&latest_account)?;
            if !same_chatgpt_session(&latest_account, &reconciled) {
                return Ok(reconciled);
            }
            let (current, marked) = mark_reauth_if_session_unchanged(&latest_account)?;
            if marked {
                return Err(error);
            }
            return Ok(current);
        }
        Err(error) => return Err(error),
    };
    let next_id_token = refreshed.id_token.unwrap_or(current_id_token);
    let next_refresh_token = refreshed
        .refresh_token
        .unwrap_or_else(|| current_refresh_token.clone());

    let claims = parse_chatgpt_id_token_claims(&next_id_token);
    let next_account_id = claims.account_id.or(current_account_id);

    let refreshed_email = claims.email;
    let refreshed_plan_type = claims.plan_type;
    let refreshed_subscription_expires_at = claims.subscription_expires_at;
    let mut candidate = latest_account.clone();
    candidate.auth_data = AuthData::ChatGPT {
        id_token: next_id_token,
        access_token: refreshed.access_token,
        refresh_token: next_refresh_token,
        account_id: next_account_id,
    };
    candidate.auth_state = AuthState::Ready;

    let updated = commit_refreshed_account(
        &latest_account,
        &candidate,
        refreshed_email,
        refreshed_plan_type,
        refreshed_subscription_expires_at,
    )?;

    Ok(updated)
}

/// Reconcile the active Switcher account with credentials written by Codex.
/// Only matching identities are adopted; an unrelated external login never
/// overwrites a saved account.
pub fn reconcile_active_account_from_codex_auth(account: &StoredAccount) -> Result<StoredAccount> {
    let store = load_accounts()?;
    let snapshot = store
        .accounts
        .into_iter()
        .find(|stored| stored.id == account.id)
        .with_context(|| format!("Account not found: {}", account.id))?;

    if store.active_account_id.as_deref() != Some(account.id.as_str()) {
        return Ok(snapshot);
    }

    let Some(auth) = read_current_auth()? else {
        return Ok(snapshot);
    };
    let Some(tokens) = auth.tokens else {
        return Ok(snapshot);
    };

    let claims = parse_chatgpt_id_token_claims(&tokens.id_token);
    let mut candidate = snapshot.clone();
    // Identity fields must come from auth.json itself. Falling back to the
    // stored identity here could make unrelated, unverifiable credentials look
    // like a match.
    candidate.email = claims.email.clone();
    candidate.plan_type = claims.plan_type.clone();
    candidate.subscription_expires_at = claims.subscription_expires_at;
    candidate.auth_data = AuthData::ChatGPT {
        id_token: tokens.id_token,
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        account_id: claims.account_id.or(tokens.account_id),
    };

    if candidate_session_is_older(&snapshot, &candidate)
        || !same_chatgpt_identity(&snapshot, &candidate)
        || same_chatgpt_session(&snapshot, &candidate)
    {
        return Ok(snapshot);
    }

    mutate_accounts(|store| {
        if store.active_account_id.as_deref() != Some(account.id.as_str()) {
            return store
                .accounts
                .iter()
                .find(|stored| stored.id == account.id)
                .cloned()
                .with_context(|| format!("Account not found: {}", account.id));
        }

        let current = store
            .accounts
            .iter_mut()
            .find(|stored| stored.id == account.id)
            .with_context(|| format!("Account not found: {}", account.id))?;

        // Do not overwrite credentials that changed after our auth.json
        // snapshot was read.
        if !same_chatgpt_session(&snapshot, current) {
            return Ok(current.clone());
        }
        if candidate_session_is_older(current, &candidate)
            || !same_chatgpt_identity(current, &candidate)
        {
            return Ok(current.clone());
        }

        current.auth_data = candidate.auth_data.clone();
        if candidate.email.is_some() {
            current.email = candidate.email.clone();
        }
        if candidate.plan_type.is_some() {
            current.plan_type = candidate.plan_type.clone();
        }
        if candidate.subscription_expires_at.is_some() {
            current.subscription_expires_at = candidate.subscription_expires_at;
        }
        current.auth_state = AuthState::Ready;
        Ok(current.clone())
    })
}

fn commit_refreshed_account(
    expected: &StoredAccount,
    candidate: &StoredAccount,
    refreshed_email: Option<String>,
    refreshed_plan_type: Option<String>,
    refreshed_subscription_expires_at: Option<chrono::DateTime<Utc>>,
) -> Result<StoredAccount> {
    let (updated, sync_error) = mutate_accounts(|store| {
        let account_index = store
            .accounts
            .iter()
            .position(|stored| stored.id == expected.id)
            .with_context(|| format!("Account not found: {}", expected.id))?;
        let current = &store.accounts[account_index];

        // OAuth reauthentication or auth.json reconciliation may have replaced
        // this session while the refresh request was in flight.
        if !same_chatgpt_session(expected, current) {
            return Ok((current.clone(), None));
        }

        let mut updated = current.clone();
        updated.auth_data = candidate.auth_data.clone();
        updated.auth_state = AuthState::Ready;
        if let Some(email) = refreshed_email.clone() {
            updated.email = Some(email);
        }
        if let Some(plan_type) = refreshed_plan_type.clone() {
            updated.plan_type = Some(plan_type);
        }
        if let Some(subscription_expires_at) = refreshed_subscription_expires_at {
            updated.subscription_expires_at = Some(subscription_expires_at);
        }
        let sync_error = if store.active_account_id.as_deref() == Some(expected.id.as_str()) {
            switch_to_account(&updated).err().map(|error| error.to_string())
        } else {
            None
        };
        store.accounts[account_index] = updated.clone();
        Ok((updated, sync_error))
    })?;

    if let Some(error) = sync_error {
        println!(
            "[Auth] Failed to sync active auth.json after token refresh for {}: {error}",
            updated.name
        );
    }
    Ok(updated)
}

fn mark_reauth_if_session_unchanged(
    expected: &StoredAccount,
) -> Result<(StoredAccount, bool)> {
    mutate_accounts(|store| {
        let current = store
            .accounts
            .iter_mut()
            .find(|stored| stored.id == expected.id)
            .with_context(|| format!("Account not found: {}", expected.id))?;
        if !same_chatgpt_session(expected, current) {
            return Ok((current.clone(), false));
        }

        current.auth_state = AuthState::ReauthRequired;
        Ok((current.clone(), true))
    })
}

async fn account_refresh_lock(account_id: &str) -> AccountRefreshLock {
    let locks = ACCOUNT_REFRESH_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut locks = locks.lock().await;
    Arc::clone(
        locks
            .entry(account_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(()))),
    )
}

fn same_chatgpt_session(left: &StoredAccount, right: &StoredAccount) -> bool {
    match (&left.auth_data, &right.auth_data) {
        (
            AuthData::ChatGPT {
                id_token: left_id_token,
                access_token: left_access_token,
                refresh_token: left_refresh_token,
                ..
            },
            AuthData::ChatGPT {
                id_token: right_id_token,
                access_token: right_access_token,
                refresh_token: right_refresh_token,
                ..
            },
        ) => {
            left_id_token == right_id_token
                && left_access_token == right_access_token
                && left_refresh_token == right_refresh_token
        }
        _ => false,
    }
}

fn candidate_session_is_older(current: &StoredAccount, candidate: &StoredAccount) -> bool {
    let access_expiry = |account: &StoredAccount| match &account.auth_data {
        AuthData::ChatGPT { access_token, .. } => parse_jwt_exp(access_token),
        AuthData::ApiKey { .. } => None,
    };

    matches!(
        (access_expiry(current), access_expiry(candidate)),
        (Some(current_expiry), Some(candidate_expiry)) if candidate_expiry < current_expiry
    )
}

pub fn same_chatgpt_identity(left: &StoredAccount, right: &StoredAccount) -> bool {
    let (left_account_id, left_email) = chatgpt_identity(left);
    let (right_account_id, right_email) = chatgpt_identity(right);
    let mut compared = false;

    if let (Some(left_id), Some(right_id)) = (left_account_id, right_account_id) {
        compared = true;
        if left_id != right_id {
            return false;
        }
    }

    if let (Some(left_email), Some(right_email)) = (left_email, right_email) {
        compared = true;
        if !left_email.eq_ignore_ascii_case(right_email) {
            return false;
        }
    }

    compared
}

fn chatgpt_identity(account: &StoredAccount) -> (Option<&str>, Option<&str>) {
    let account_id = match &account.auth_data {
        AuthData::ChatGPT { account_id, .. } => account_id.as_deref(),
        AuthData::ApiKey { .. } => None,
    };
    (
        account_id.filter(|value| !value.trim().is_empty()),
        account.email.as_deref().filter(|value| !value.trim().is_empty()),
    )
}

fn should_block_active_refresh(is_active: bool, codex_running: bool) -> bool {
    is_active && codex_running
}

fn is_permanent_refresh_error_code(code: &str) -> bool {
    matches!(
        code,
        "refresh_token_invalidated"
            | "refresh_token_reused"
            | "refresh_token_expired"
            | "invalid_refresh_token"
            | "invalid_grant"
    )
}

fn refresh_error_code(payload: &serde_json::Value) -> &str {
    payload
        .pointer("/error/code")
        .and_then(serde_json::Value::as_str)
        .or_else(|| payload.get("code").and_then(serde_json::Value::as_str))
        .or_else(|| payload.get("error").and_then(serde_json::Value::as_str))
        .unwrap_or("unknown_refresh_error")
}

/// Build a new ChatGPT account from a refresh token.
/// This is used by slim import to recreate full credentials.
pub async fn create_chatgpt_account_from_refresh_token(
    account_name: String,
    refresh_token: String,
) -> Result<StoredAccount> {
    if refresh_token.trim().is_empty() {
        anyhow::bail!("Missing refresh token for account {account_name}");
    }

    let refreshed = refresh_tokens_with_refresh_token(&refresh_token).await?;
    let id_token = refreshed
        .id_token
        .context("Refresh response did not include id_token")?;
    let next_refresh_token = refreshed.refresh_token.unwrap_or(refresh_token);
    let claims = parse_chatgpt_id_token_claims(&id_token);

    Ok(StoredAccount::new_chatgpt(
        account_name,
        claims.email,
        claims.plan_type,
        claims.subscription_expires_at,
        id_token,
        refreshed.access_token,
        next_refresh_token,
        claims.account_id,
    ))
}

fn token_expired_or_near_expiry(access_token: &str) -> bool {
    match parse_jwt_exp(access_token) {
        Some(expiry) => expiry <= Utc::now().timestamp() + EXPIRY_SKEW_SECONDS,
        None => false,
    }
}

fn parse_jwt_exp(token: &str) -> Option<i64> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return None;
    }

    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(parts[1])
        .ok()?;
    let json: serde_json::Value = serde_json::from_slice(&payload).ok()?;
    json.get("exp").and_then(|v| v.as_i64())
}

async fn refresh_tokens_with_refresh_token(refresh_token: &str) -> Result<RefreshTokenResponse> {
    let client = reqwest::Client::new();
    let body = format!(
        "grant_type=refresh_token&refresh_token={}&client_id={}",
        urlencoding::encode(refresh_token),
        urlencoding::encode(CLIENT_ID),
    );

    let mut last_send_error = None;
    let mut response = None;

    for attempt in 1..=3u8 {
        match client
            .post(format!("{DEFAULT_ISSUER}/oauth/token"))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body.clone())
            .send()
            .await
        {
            Ok(resp) => {
                response = Some(resp);
                break;
            }
            Err(err) => {
                last_send_error = Some(err);
                if attempt < 3 {
                    sleep(Duration::from_millis(250 * u64::from(attempt))).await;
                }
            }
        }
    }

    let response = match response {
        Some(resp) => resp,
        None => {
            let err = last_send_error.context("Failed to send token refresh request")?;
            return Err(err.into());
        }
    };

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        let payload = serde_json::from_str::<serde_json::Value>(&body).unwrap_or_default();
        let code = refresh_error_code(&payload);

        if is_permanent_refresh_error_code(code) {
            return Err(PermanentRefreshError {
                code: code.to_string(),
            }
            .into());
        }

        anyhow::bail!("Token refresh failed: {status} ({code})");
    }

    response
        .json::<RefreshTokenResponse>()
        .await
        .context("Failed to parse token refresh response")
}

#[cfg(test)]
mod tests {
    use super::{
        candidate_session_is_older, is_permanent_refresh_error_code, refresh_error_code,
        same_chatgpt_identity, same_chatgpt_session, should_block_active_refresh,
    };
    use base64::Engine;
    use crate::types::{AuthData, StoredAccount};

    fn account(email: Option<&str>, account_id: Option<&str>, token_suffix: &str) -> StoredAccount {
        StoredAccount::new_chatgpt(
            "Test account".into(),
            email.map(str::to_string),
            Some("plus".into()),
            None,
            format!("id-{token_suffix}"),
            format!("access-{token_suffix}"),
            format!("refresh-{token_suffix}"),
            account_id.map(str::to_string),
        )
    }

    #[test]
    fn reauth_identity_requires_matching_comparable_fields() {
        let original = account(Some("User@example.com"), Some("account-1"), "old");
        let matching = account(Some("user@example.com"), Some("account-1"), "new");
        assert!(same_chatgpt_identity(&original, &matching));

        let wrong_id = account(Some("user@example.com"), Some("account-2"), "new");
        assert!(!same_chatgpt_identity(&original, &wrong_id));

        let wrong_email = account(Some("other@example.com"), Some("account-1"), "new");
        assert!(!same_chatgpt_identity(&original, &wrong_email));

        let email_only = account(Some("user@example.com"), None, "new");
        assert!(same_chatgpt_identity(&original, &email_only));

        let unverifiable = account(None, None, "new");
        assert!(!same_chatgpt_identity(&original, &unverifiable));
    }

    #[test]
    fn token_session_comparison_detects_rotated_credentials() {
        let original = account(Some("user@example.com"), Some("account-1"), "old");
        let mut same = original.clone();
        assert!(same_chatgpt_session(&original, &same));

        let AuthData::ChatGPT { refresh_token, .. } = &mut same.auth_data else {
            panic!("expected ChatGPT credentials");
        };
        *refresh_token = "refresh-rotated".into();
        assert!(!same_chatgpt_session(&original, &same));
    }

    #[test]
    fn permanent_refresh_error_codes_are_stable() {
        for code in [
            "refresh_token_invalidated",
            "refresh_token_reused",
            "refresh_token_expired",
            "invalid_refresh_token",
            "invalid_grant",
        ] {
            assert!(is_permanent_refresh_error_code(code));
        }
        assert!(!is_permanent_refresh_error_code("temporarily_unavailable"));
    }

    #[test]
    fn refresh_error_code_supports_provider_error_shapes() {
        assert_eq!(
            refresh_error_code(&serde_json::json!({
                "error": { "code": "refresh_token_invalidated" }
            })),
            "refresh_token_invalidated"
        );
        assert_eq!(
            refresh_error_code(&serde_json::json!({ "error": "invalid_grant" })),
            "invalid_grant"
        );
    }

    #[test]
    fn only_active_running_account_blocks_refresh_rotation() {
        assert!(should_block_active_refresh(true, true));
        assert!(!should_block_active_refresh(true, false));
        assert!(!should_block_active_refresh(false, true));
    }

    #[test]
    fn auth_json_reconciliation_does_not_regress_access_token_expiry() {
        let jwt = |expiry: i64| {
            let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(format!(r#"{{"exp":{expiry}}}"#));
            format!("header.{payload}.signature")
        };
        let mut current = account(Some("user@example.com"), Some("account-1"), "current");
        let mut older = account(Some("user@example.com"), Some("account-1"), "older");
        let AuthData::ChatGPT { access_token, .. } = &mut current.auth_data else {
            unreachable!();
        };
        *access_token = jwt(2_000);
        let AuthData::ChatGPT { access_token, .. } = &mut older.auth_data else {
            unreachable!();
        };
        *access_token = jwt(1_000);

        assert!(candidate_session_is_older(&current, &older));
        assert!(!candidate_session_is_older(&older, &current));
    }

    #[tokio::test]
    async fn same_account_refreshes_share_one_lock() {
        let account_id = format!("lock-test-{}", uuid::Uuid::new_v4());
        let first = super::account_refresh_lock(&account_id).await;
        let second = super::account_refresh_lock(&account_id).await;

        assert!(std::sync::Arc::ptr_eq(&first, &second));
        let _guard = first.lock().await;
        assert!(second.try_lock().is_err());
    }
}
