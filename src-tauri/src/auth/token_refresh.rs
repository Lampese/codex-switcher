//! ChatGPT OAuth token refresh helpers

use anyhow::{Context, Result};
use base64::Engine;
use chrono::Utc;
use tokio::time::{sleep, Duration};

use super::{
    load_accounts, read_current_auth, save_accounts, switch_to_account, sync_active_account_tokens,
    update_account_chatgpt_tokens, AUTH_OPERATION_LOCK,
};
use crate::types::{
    parse_chatgpt_id_token_claims, AccountsStore, AuthData, AuthDotJson, StoredAccount,
};

const DEFAULT_ISSUER: &str = "https://auth.openai.com";
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const EXPIRY_SKEW_SECONDS: i64 = 60;

#[derive(Debug, serde::Deserialize)]
struct RefreshTokenResponse {
    #[serde(default)]
    id_token: Option<String>,
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct RefreshTokenErrorResponse {
    #[serde(default)]
    error: Option<RefreshTokenErrorDetails>,
}

#[derive(Debug, serde::Deserialize)]
struct RefreshTokenErrorDetails {
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    code: Option<String>,
}

#[derive(Debug)]
struct TokenRefreshUpdate {
    id_token: String,
    access_token: String,
    refresh_token: String,
    id_token_error: Option<anyhow::Error>,
}

/// Ensure the account has a usable ChatGPT access token for background API calls.
///
/// Usage polling and warm-up do not need a fresh ID token. Refreshing only because
/// an ID token expired can unnecessarily consume a single-use refresh token while
/// the access token and the live Codex session are still perfectly valid.
pub async fn ensure_chatgpt_tokens_fresh(account: &StoredAccount) -> Result<StoredAccount> {
    if !chatgpt_access_token_needs_refresh(account) {
        return Ok(account.clone());
    }

    let _auth_guard = AUTH_OPERATION_LOCK.lock().await;
    ensure_chatgpt_tokens_fresh_with_policy_locked(account, false).await
}

/// Ensure the full ChatGPT OAuth token set is fresh while the caller holds
/// AUTH_OPERATION_LOCK. Account switching still needs a fresh ID token because
/// Codex can reject auth.json when its ID token is expired.
pub(crate) async fn ensure_chatgpt_tokens_fresh_locked(
    account: &StoredAccount,
) -> Result<StoredAccount> {
    match ensure_chatgpt_tokens_fresh_with_policy_locked(account, true).await {
        Ok(account) => Ok(account),
        Err(refresh_error) => {
            // A stale/invalidated refresh token must not make the Switch button
            // unusable while the stored access token is still accepted. We still
            // prefer a full refresh first (fresh ID token is best for Codex), but
            // if that cannot be done, fall back to the latest stored/live account
            // as long as its access token is still usable.
            let (fallback, _) = load_account_reconciling_live_auth(&account.id)?;
            if can_switch_with_existing_access_token(&fallback) {
                eprintln!(
                    "[Auth] Full OAuth refresh failed before switch for account {}: {refresh_error:#}. \
Proceeding with the existing usable access token.",
                    fallback.name
                );
                Ok(fallback)
            } else {
                Err(refresh_error)
            }
        }
    }
}

async fn ensure_chatgpt_tokens_fresh_with_policy_locked(
    account: &StoredAccount,
    require_fresh_id_token: bool,
) -> Result<StoredAccount> {
    if matches!(account.auth_data, AuthData::ApiKey { .. }) {
        return Ok(account.clone());
    }

    // Codex may have refreshed the active account while this task waited for
    // the lock. Prefer those live credentials over rotating the stored token.
    let (current, _) = load_account_reconciling_live_auth(&account.id)?;

    match &current.auth_data {
        AuthData::ApiKey { .. } => Ok(current.clone()),
        AuthData::ChatGPT {
            id_token,
            access_token,
            ..
        } => {
            let needs_refresh = if require_fresh_id_token {
                chatgpt_tokens_need_refresh_at(id_token, access_token, Utc::now().timestamp())
            } else {
                token_expired_or_near_expiry_at(access_token, Utc::now().timestamp())
            };

            if needs_refresh {
                let reason = if require_fresh_id_token {
                    "OAuth token"
                } else {
                    "access token"
                };
                println!(
                    "[Auth] {reason} expired/near expiry for account {}, refreshing",
                    current.name
                );
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

    let _auth_guard = AUTH_OPERATION_LOCK.lock().await;
    refresh_chatgpt_tokens_locked(account).await
}

async fn refresh_chatgpt_tokens_locked(account: &StoredAccount) -> Result<StoredAccount> {
    let (current, is_active) = load_account_reconciling_live_auth(&account.id)?;

    if is_active && crate::commands::process::ensure_codex_not_running().is_err() {
        println!(
            "[Auth] Using the running app's live credentials for active account {}",
            current.name
        );
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
    let mut store = load_accounts()?;
    let is_active = store.active_account_id.as_deref() == Some(account_id);

    if is_active {
        if let Some(auth) = read_current_auth()? {
            if reconcile_active_account_from_auth(&mut store, account_id, &auth) {
                save_accounts(&store)?;
            }
        }
    }

    let account = store
        .accounts
        .into_iter()
        .find(|stored| stored.id == account_id)
        .context("Account not found")?;
    Ok((account, is_active))
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

fn chatgpt_access_token_needs_refresh(account: &StoredAccount) -> bool {
    match &account.auth_data {
        AuthData::ApiKey { .. } => false,
        AuthData::ChatGPT { access_token, .. } => {
            token_expired_or_near_expiry_at(access_token, Utc::now().timestamp())
        }
    }
}

fn can_switch_with_existing_access_token(account: &StoredAccount) -> bool {
    !chatgpt_access_token_needs_refresh(account)
}

fn chatgpt_tokens_need_refresh(account: &StoredAccount) -> bool {
    match &account.auth_data {
        AuthData::ApiKey { .. } => false,
        AuthData::ChatGPT {
            id_token,
            access_token,
            ..
        } => chatgpt_tokens_need_refresh_at(id_token, access_token, Utc::now().timestamp()),
    }
}

fn chatgpt_tokens_need_refresh_at(id_token: &str, access_token: &str, now: i64) -> bool {
    id_token_needs_refresh_at(id_token, now) || token_expired_or_near_expiry_at(access_token, now)
}

fn id_token_needs_refresh_at(token: &str, now: i64) -> bool {
    match parse_jwt_exp(token) {
        Some(expiry) => expiry <= now + EXPIRY_SKEW_SECONDS,
        None => true,
    }
}

fn token_expired_or_near_expiry_at(token: &str, now: i64) -> bool {
    match parse_jwt_exp(token) {
        Some(expiry) => expiry <= now + EXPIRY_SKEW_SECONDS,
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

fn format_token_refresh_error(status: reqwest::StatusCode, body: &str) -> String {
    let parsed = serde_json::from_str::<RefreshTokenErrorResponse>(body).ok();
    let details = parsed.as_ref().and_then(|payload| payload.error.as_ref());
    let code = details.and_then(|error| error.code.as_deref());

    match code {
        Some("refresh_token_invalidated") => {
            "Session expired. Re-authenticate this account.".to_string()
        }
        Some("refresh_token_reused") => {
            "Session expired. Re-authenticate this account.".to_string()
        }
        _ => {
            if let Some(message) = details.and_then(|error| error.message.as_deref()) {
                format!("Could not refresh the saved Codex Switcher session ({status}). {message}")
            } else {
                format!(
                    "Could not refresh the saved Codex Switcher session ({status}). Please try again."
                )
            }
        }
    }
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
        eprintln!("[Auth] Token refresh failed: {status} - {body}");
        anyhow::bail!("{}", format_token_refresh_error(status, &body));
    }

    response
        .json::<RefreshTokenResponse>()
        .await
        .context("Failed to parse token refresh response")
}

#[cfg(test)]
mod tests {
    use super::{
        can_switch_with_existing_access_token, chatgpt_access_token_needs_refresh,
        chatgpt_tokens_need_refresh,
        chatgpt_tokens_need_refresh_at, format_token_refresh_error, merge_refresh_response,
        reconcile_active_account_from_auth, resolve_refreshed_id_token, RefreshTokenResponse,
    };
    use crate::types::{AccountsStore, AuthData, AuthDotJson, StoredAccount, TokenData};
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};

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
    fn background_refresh_ignores_expired_id_token_when_access_token_is_valid() {
        let now = chrono::Utc::now().timestamp();
        let account = StoredAccount::new_chatgpt(
            "Background".into(),
            None,
            None,
            None,
            jwt_with_exp(now - 3_600),
            jwt_with_exp(now + 3_600),
            "refresh".into(),
            None,
        );

        assert!(!chatgpt_access_token_needs_refresh(&account));
        assert!(chatgpt_tokens_need_refresh(&account));
    }

    #[test]
    fn switch_fallback_allows_valid_access_token_when_full_refresh_fails() {
        let now = chrono::Utc::now().timestamp();
        let account = StoredAccount::new_chatgpt(
            "Switch fallback".into(),
            None,
            None,
            None,
            jwt_with_exp(now - 3_600),
            jwt_with_exp(now + 3_600),
            "stale-refresh".into(),
            None,
        );

        assert!(can_switch_with_existing_access_token(&account));
    }

    #[test]
    fn switch_fallback_rejects_expired_access_token() {
        let now = chrono::Utc::now().timestamp();
        let account = StoredAccount::new_chatgpt(
            "Expired access".into(),
            None,
            None,
            None,
            jwt_with_exp(now - 3_600),
            jwt_with_exp(now - 3_600),
            "stale-refresh".into(),
            None,
        );

        assert!(!can_switch_with_existing_access_token(&account));
    }

    #[test]
    fn refresh_required_when_id_token_expired_but_access_token_valid_for_switching() {
        let now = 1_800_000_000;
        let id_token = jwt_with_exp(now - 3_600);
        let access_token = jwt_with_exp(now + 3_600);

        assert!(chatgpt_tokens_need_refresh_at(
            &id_token,
            &access_token,
            now
        ));
    }

    #[test]
    fn refresh_not_required_when_both_tokens_are_valid() {
        let now = 1_800_000_000;
        let id_token = jwt_with_exp(now + 3_600);
        let access_token = jwt_with_exp(now + 3_600);

        assert!(!chatgpt_tokens_need_refresh_at(
            &id_token,
            &access_token,
            now
        ));
    }

    #[test]
    fn refresh_required_when_access_token_expired() {
        let now = 1_800_000_000;
        let id_token = jwt_with_exp(now + 3_600);
        let access_token = jwt_with_exp(now - 3_600);

        assert!(chatgpt_tokens_need_refresh_at(
            &id_token,
            &access_token,
            now
        ));
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

    #[test]
    fn refresh_token_invalidated_error_is_human_readable() {
        let body = r#"{"error":{"message":"Your session has ended. Please log in again.","type":"invalid_request_error","param":null,"code":"refresh_token_invalidated"}}"#;
        let message = format_token_refresh_error(reqwest::StatusCode::UNAUTHORIZED, body);

        assert_eq!(message, "Session expired. Re-authenticate this account.");
        assert!(!message.contains("{\"error\""));
    }

    #[test]
    fn refresh_token_reused_error_is_human_readable() {
        let body = r#"{"error":{"message":"already used","code":"refresh_token_reused"}}"#;
        let message = format_token_refresh_error(reqwest::StatusCode::UNAUTHORIZED, body);

        assert_eq!(message, "Session expired. Re-authenticate this account.");
        assert!(!message.contains("{\"error\""));
    }
}
