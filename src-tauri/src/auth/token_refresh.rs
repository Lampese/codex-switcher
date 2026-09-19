//! ChatGPT OAuth token refresh helpers

use anyhow::{Context, Result};
use base64::Engine;
use chrono::{Duration as ChronoDuration, Utc};
use reqwest::StatusCode;
use tokio::time::{sleep, Duration};

use super::storage::acquire_auth_operation_lock;
#[cfg(test)]
use super::sync_active_account_tokens;
use super::{
    load_accounts, mutate_accounts, read_current_auth, reconcile_active_projection,
    switch_to_account, update_account_chatgpt_tokens,
};
use crate::types::{parse_chatgpt_id_token_claims, AuthData, StoredAccount};
#[cfg(test)]
use crate::types::{AccountsStore, AuthDotJson};

const DEFAULT_ISSUER: &str = "https://auth.openai.com";
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const ID_TOKEN_REFRESH_WINDOW_SECONDS: i64 = 60;
const ACCESS_TOKEN_REFRESH_WINDOW_SECONDS: i64 = 5 * 60;
const TOKEN_REFRESH_INTERVAL_DAYS: i64 = 8;

#[derive(Debug, serde::Deserialize)]
struct RefreshTokenResponse {
    #[serde(default)]
    id_token: Option<String>,
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
}

#[derive(Debug)]
struct TokenRefreshUpdate {
    id_token: String,
    access_token: String,
    refresh_token: String,
    id_token_error: Option<anyhow::Error>,
}

/// Ensure the account has non-expired ChatGPT OAuth tokens.
/// Returns an updated account when a refresh was performed.
pub async fn ensure_chatgpt_tokens_fresh(account: &StoredAccount) -> Result<StoredAccount> {
    if !proactive_refresh_may_be_needed(account) {
        return Ok(account.clone());
    }

    let _auth_guard = acquire_auth_operation_lock().await?;
    ensure_chatgpt_tokens_fresh_locked(account).await
}

/// Ensure ChatGPT OAuth tokens are fresh while the caller holds the
/// cross-process auth-operation lock.
pub(crate) async fn ensure_chatgpt_tokens_fresh_locked(
    account: &StoredAccount,
) -> Result<StoredAccount> {
    if matches!(account.auth_data, AuthData::ApiKey { .. }) {
        return Ok(account.clone());
    }

    // Codex may have refreshed the active account while this task waited for
    // the lock. Prefer those live credentials over rotating the stored token.
    let (current, _) = load_account_reconciling_live_auth(&account.id)?;

    match &current.auth_data {
        AuthData::ApiKey { .. } => Ok(current.clone()),
        AuthData::ChatGPT { .. } => {
            if chatgpt_tokens_need_refresh(&current) {
                refresh_chatgpt_tokens_locked(&current).await
            } else {
                Ok(current)
            }
        }
    }
}

/// Force-refresh ChatGPT OAuth tokens for an account.
pub async fn refresh_chatgpt_tokens(account: &StoredAccount) -> Result<StoredAccount> {
    if matches!(account.auth_data, AuthData::ApiKey { .. }) {
        return Ok(account.clone());
    }

    let _auth_guard = acquire_auth_operation_lock().await?;
    refresh_chatgpt_tokens_locked(account).await
}

/// Retry policy for provider responses. Only 401 proves the access token was
/// rejected. A 403 may be authorization or edge/CDN behavior and must not
/// consume a rotating refresh token.
pub fn should_refresh_after_provider_status(status: StatusCode) -> bool {
    status == StatusCode::UNAUTHORIZED
}

/// Refresh after a provider 401 only if the rejected access token is still the
/// current token after acquiring the auth-operation lock. Concurrent callers
/// that observed the same old token therefore share one refresh result.
pub async fn refresh_chatgpt_tokens_after_unauthorized(
    account: &StoredAccount,
    rejected_access_token: &str,
) -> Result<StoredAccount> {
    if matches!(account.auth_data, AuthData::ApiKey { .. }) {
        return Ok(account.clone());
    }

    let _auth_guard = acquire_auth_operation_lock().await?;
    let (current, _) = load_account_reconciling_live_auth(&account.id)?;

    let AuthData::ChatGPT { access_token, .. } = &current.auth_data else {
        return Ok(current);
    };

    if !rejected_access_token_is_still_current(access_token, rejected_access_token) {
        return Ok(current);
    }

    refresh_chatgpt_tokens_locked(&current).await
}

async fn refresh_chatgpt_tokens_locked(account: &StoredAccount) -> Result<StoredAccount> {
    let (current, is_active) = load_account_reconciling_live_auth(&account.id)?;

    if is_active && crate::commands::process::ensure_codex_not_running().is_err() {
        return Ok(current);
    }

    let (current_id_token, current_refresh_token, current_account_id) = match &current.auth_data {
        AuthData::ChatGPT {
            id_token,
            refresh_token,
            account_id,
            ..
        } => (id_token.clone(), refresh_token.clone(), account_id.clone()),
        AuthData::ApiKey { .. } => return Ok(current),
    };

    if current_refresh_token.is_empty() {
        anyhow::bail!("Missing refresh token for account {}", current.name);
    }

    let refreshed = refresh_tokens_with_refresh_token(&current_refresh_token).await?;
    let next = merge_refresh_response(
        current_id_token,
        current_refresh_token,
        refreshed,
        Utc::now().timestamp(),
    );

    let claims = parse_chatgpt_id_token_claims(&next.id_token);
    let next_account_id = claims.account_id.or(current_account_id);

    let updated = update_account_chatgpt_tokens(
        &account.id,
        next.id_token,
        next.access_token,
        next.refresh_token,
        next_account_id,
        claims.email,
        claims.plan_type,
        claims.subscription_expires_at,
    )?;
    println!("[Auth] Refreshed OAuth tokens for: {}", updated.name);

    // Refresh tokens can be single-use. Persist a rotated replacement before
    // reporting an unusable ID token, so a later retry can still recover.
    if let Some(error) = next.id_token_error {
        return Err(error);
    }

    // Re-read active state after the network request before touching auth.json.
    let is_active = load_accounts()?.active_account_id.as_deref() == Some(account.id.as_str());
    if is_active {
        if let Err(err) = switch_to_account(&updated) {
            println!("[Auth] Failed to sync active auth.json after token refresh: {err}");
        }
    }

    Ok(updated)
}

#[cfg(test)]
fn reconcile_active_account_from_auth(
    store: &mut AccountsStore,
    account_id: &str,
    auth: &AuthDotJson,
) -> bool {
    if store.active_account_id.as_deref() != Some(account_id) {
        return false;
    }

    sync_active_account_tokens(store, auth)
}

fn load_account_reconciling_live_auth(account_id: &str) -> Result<(StoredAccount, bool)> {
    let auth = read_current_auth()?;

    mutate_accounts(|store| {
        reconcile_active_projection(store, auth.as_ref());

        let is_active = store.active_account_id.as_deref() == Some(account_id);
        let account = store
            .accounts
            .iter()
            .find(|stored| stored.id == account_id)
            .cloned()
            .context("Account not found")?;
        Ok((account, is_active))
    })
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

fn chatgpt_tokens_need_refresh(account: &StoredAccount) -> bool {
    match &account.auth_data {
        AuthData::ApiKey { .. } => false,
        AuthData::ChatGPT { access_token, .. } => should_refresh_proactively_at(
            access_token,
            account
                .last_refresh_at
                .map(|timestamp| timestamp.timestamp()),
            Utc::now().timestamp(),
        ),
    }
}

fn proactive_refresh_may_be_needed(account: &StoredAccount) -> bool {
    match &account.auth_data {
        AuthData::ApiKey { .. } => false,
        AuthData::ChatGPT { access_token, .. } => {
            parse_jwt_exp(access_token).is_none() || chatgpt_tokens_need_refresh(account)
        }
    }
}

fn should_refresh_proactively_at(
    access_token: &str,
    last_refresh_at: Option<i64>,
    now: i64,
) -> bool {
    if parse_jwt_exp(access_token).is_some() {
        return access_token_needs_refresh_at(access_token, now);
    }

    last_refresh_at.is_some_and(|last_refresh| {
        last_refresh < now - ChronoDuration::days(TOKEN_REFRESH_INTERVAL_DAYS).num_seconds()
    })
}

fn access_token_needs_refresh_at(access_token: &str, now: i64) -> bool {
    token_expired_or_near_expiry_at(access_token, now)
}

fn id_token_needs_refresh_at(token: &str, now: i64) -> bool {
    match parse_jwt_exp(token) {
        Some(expiry) => expiry <= now + ID_TOKEN_REFRESH_WINDOW_SECONDS,
        None => true,
    }
}

fn rejected_access_token_is_still_current(current: &str, rejected: &str) -> bool {
    current == rejected
}

fn token_expired_or_near_expiry_at(token: &str, now: i64) -> bool {
    match parse_jwt_exp(token) {
        Some(expiry) => expiry <= now + ACCESS_TOKEN_REFRESH_WINDOW_SECONDS,
        None => false,
    }
}

fn resolve_refreshed_id_token(
    current_id_token: String,
    refreshed_id_token: Option<String>,
    now: i64,
) -> Result<String> {
    match refreshed_id_token {
        Some(id_token) if id_token_needs_refresh_at(&id_token, now) => {
            anyhow::bail!("Token refresh returned an invalid or expired id_token")
        }
        Some(id_token) => Ok(id_token),
        None if id_token_needs_refresh_at(&current_id_token, now) => {
            anyhow::bail!(
                "Token refresh did not return a fresh id_token; sign in to the account again"
            )
        }
        None => Ok(current_id_token),
    }
}

fn merge_refresh_response(
    current_id_token: String,
    current_refresh_token: String,
    refreshed: RefreshTokenResponse,
    now: i64,
) -> TokenRefreshUpdate {
    let (id_token, id_token_error) =
        match resolve_refreshed_id_token(current_id_token.clone(), refreshed.id_token, now) {
            Ok(id_token) => (id_token, None),
            Err(error) => (current_id_token, Some(error)),
        };

    TokenRefreshUpdate {
        id_token,
        access_token: refreshed.access_token,
        refresh_token: refreshed.refresh_token.unwrap_or(current_refresh_token),
        id_token_error,
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
            .timeout(Duration::from_secs(10))
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
        anyhow::bail!("Token refresh failed: {status} - {body}");
    }

    response
        .json::<RefreshTokenResponse>()
        .await
        .context("Failed to parse token refresh response")
}

#[cfg(test)]
mod tests {
    use super::{
        access_token_needs_refresh_at, chatgpt_tokens_need_refresh, merge_refresh_response,
        reconcile_active_account_from_auth, rejected_access_token_is_still_current,
        resolve_refreshed_id_token, should_refresh_after_provider_status,
        should_refresh_proactively_at, RefreshTokenResponse,
    };
    use crate::types::{AccountsStore, AuthData, AuthDotJson, StoredAccount, TokenData};
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use reqwest::StatusCode;

    fn jwt_with_exp(exp: i64) -> String {
        let payload = URL_SAFE_NO_PAD.encode(format!(r#"{{"exp":{exp}}}"#));
        format!("header.{payload}.signature")
    }

    fn account_jwt(account_id: &str, exp: i64, signature: &str) -> String {
        let payload = URL_SAFE_NO_PAD.encode(format!(
            r#"{{"exp":{exp},"https://api.openai.com/auth":{{"chatgpt_account_id":"{account_id}"}}}}"#
        ));
        format!("header.{payload}.{signature}")
    }

    #[test]
    fn concurrent_unauthorized_retry_reuses_a_replaced_access_token() {
        assert!(rejected_access_token_is_still_current(
            "old-token",
            "old-token"
        ));
        assert!(!rejected_access_token_is_still_current(
            "new-token",
            "old-token"
        ));
    }

    #[test]
    fn provider_retry_refreshes_only_on_unauthorized() {
        assert!(should_refresh_after_provider_status(
            StatusCode::UNAUTHORIZED
        ));
        assert!(!should_refresh_after_provider_status(StatusCode::FORBIDDEN));
        assert!(!should_refresh_after_provider_status(
            StatusCode::TOO_MANY_REQUESTS
        ));
        assert!(!should_refresh_after_provider_status(
            StatusCode::INTERNAL_SERVER_ERROR
        ));
    }

    #[test]
    fn parseable_fresh_access_token_does_not_use_old_refresh_age() {
        let now = 1_800_000_000;

        assert!(!should_refresh_proactively_at(
            &jwt_with_exp(now + 3_600),
            Some(now - 9 * 24 * 60 * 60),
            now,
        ));
    }

    #[test]
    fn unparseable_access_token_uses_recent_refresh_age() {
        let now = 1_800_000_000;

        assert!(!should_refresh_proactively_at(
            "opaque-access-token",
            Some(now - 7 * 24 * 60 * 60),
            now,
        ));
    }

    #[test]
    fn unparseable_access_token_older_than_eight_days_requires_refresh() {
        let now = 1_800_000_000;

        assert!(should_refresh_proactively_at(
            "opaque-access-token",
            Some(now - (8 * 24 * 60 * 60 + 1)),
            now,
        ));
    }

    #[test]
    fn unparseable_access_token_without_refresh_age_does_not_refresh() {
        let now = 1_800_000_000;

        assert!(!should_refresh_proactively_at(
            "opaque-access-token",
            None,
            now,
        ));
    }

    #[test]
    fn refresh_not_required_when_only_id_token_is_expired() {
        let now = 1_800_000_000;
        let access_token = jwt_with_exp(now + 3_600);

        assert!(!access_token_needs_refresh_at(&access_token, now));
    }

    #[test]
    fn refresh_not_required_when_both_tokens_are_valid() {
        let now = 1_800_000_000;
        let access_token = jwt_with_exp(now + 3_600);

        assert!(!access_token_needs_refresh_at(&access_token, now));
    }

    #[test]
    fn access_token_refresh_window_matches_codex_five_minutes() {
        let now = 1_800_000_000;
        assert!(access_token_needs_refresh_at(
            &jwt_with_exp(now + 5 * 60),
            now
        ));
        assert!(!access_token_needs_refresh_at(
            &jwt_with_exp(now + 5 * 60 + 1),
            now
        ));
    }

    #[test]
    fn refresh_required_when_access_token_expired() {
        let now = 1_800_000_000;
        let access_token = jwt_with_exp(now - 3_600);

        assert!(access_token_needs_refresh_at(&access_token, now));
    }

    #[test]
    fn expired_id_token_requires_replacement_from_refresh_response() {
        let now = 1_800_000_000;
        let current_id_token = jwt_with_exp(now - 3_600);

        let error = resolve_refreshed_id_token(current_id_token, None, now).unwrap_err();

        assert!(error
            .to_string()
            .contains("did not return a fresh id_token"));
    }

    #[test]
    fn valid_id_token_can_be_preserved_when_refresh_response_omits_it() {
        let now = 1_800_000_000;
        let current_id_token = jwt_with_exp(now + 3_600);

        let resolved = resolve_refreshed_id_token(current_id_token.clone(), None, now).unwrap();

        assert_eq!(resolved, current_id_token);
    }

    #[test]
    fn refreshed_id_token_uses_its_own_short_validation_window() {
        let now = 1_800_000_000;
        let refreshed_id_token = jwt_with_exp(now + 2 * 60);

        assert_eq!(
            resolve_refreshed_id_token(String::new(), Some(refreshed_id_token.clone()), now)
                .unwrap(),
            refreshed_id_token
        );
    }

    #[test]
    fn active_account_uses_fresh_tokens_from_live_auth() {
        let now = chrono::Utc::now().timestamp();
        let mut account = StoredAccount::new_chatgpt(
            "Active".into(),
            None,
            None,
            None,
            account_jwt("workspace-a", now - 3_600, "stored"),
            jwt_with_exp(now - 3_600),
            "stored-refresh".into(),
            Some("workspace-a".into()),
        );
        account.last_refresh_at = None;
        let local_id = account.id.clone();
        let mut store = AccountsStore {
            accounts: vec![account.clone()],
            active_account_id: Some(local_id.clone()),
            ..AccountsStore::default()
        };
        let auth = AuthDotJson {
            openai_api_key: None,
            tokens: Some(TokenData {
                id_token: account_jwt("workspace-a", now + 3_600, "live"),
                access_token: jwt_with_exp(now + 3_600),
                refresh_token: "live-refresh".into(),
                account_id: Some("workspace-a".into()),
            }),
            last_refresh: None,
        };

        assert!(chatgpt_tokens_need_refresh(&account));
        assert!(reconcile_active_account_from_auth(
            &mut store, &local_id, &auth
        ));

        account = store.accounts.remove(0);
        assert!(!chatgpt_tokens_need_refresh(&account));
        let AuthData::ChatGPT { refresh_token, .. } = account.auth_data else {
            panic!("expected ChatGPT account");
        };
        assert_eq!(refresh_token, "live-refresh");
    }

    #[test]
    fn inactive_account_does_not_use_live_auth() {
        let now = chrono::Utc::now().timestamp();
        let account = StoredAccount::new_chatgpt(
            "Inactive".into(),
            None,
            None,
            None,
            account_jwt("workspace-a", now - 3_600, "stored"),
            jwt_with_exp(now - 3_600),
            "stored-refresh".into(),
            Some("workspace-a".into()),
        );
        let local_id = account.id.clone();
        let mut store = AccountsStore {
            accounts: vec![account],
            active_account_id: Some("different-local-account".into()),
            ..AccountsStore::default()
        };
        let auth = AuthDotJson {
            openai_api_key: None,
            tokens: Some(TokenData {
                id_token: account_jwt("workspace-a", now + 3_600, "live"),
                access_token: jwt_with_exp(now + 3_600),
                refresh_token: "live-refresh".into(),
                account_id: Some("workspace-a".into()),
            }),
            last_refresh: None,
        };

        assert!(!reconcile_active_account_from_auth(
            &mut store, &local_id, &auth
        ));
        let AuthData::ChatGPT { refresh_token, .. } = &store.accounts[0].auth_data else {
            panic!("expected ChatGPT account");
        };
        assert_eq!(refresh_token, "stored-refresh");
    }

    #[test]
    fn rotated_refresh_token_is_retained_when_id_token_is_missing() {
        let now = 1_800_000_000;
        let current_id_token = jwt_with_exp(now - 3_600);
        let refreshed = RefreshTokenResponse {
            id_token: None,
            access_token: "new-access".into(),
            refresh_token: Some("rotated-refresh".into()),
        };

        let update = merge_refresh_response(
            current_id_token.clone(),
            "old-refresh".into(),
            refreshed,
            now,
        );

        assert_eq!(update.id_token, current_id_token);
        assert_eq!(update.refresh_token, "rotated-refresh");
        assert!(update.id_token_error.is_some());
    }
}
