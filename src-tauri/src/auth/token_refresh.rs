//! ChatGPT OAuth token refresh helpers

use anyhow::{Context, Result};
use base64::Engine;
use chrono::Utc;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use tokio::sync::Mutex;
use tokio::time::{sleep, Duration};

use super::{load_accounts, read_current_auth, switch_to_account, update_account_chatgpt_tokens};
use crate::types::{parse_chatgpt_id_token_claims, AuthData, StoredAccount, TokenData};

const DEFAULT_ISSUER: &str = "https://auth.openai.com";
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const EXPIRY_SKEW_SECONDS: i64 = 60;

static REFRESH_LOCKS: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();

#[derive(Debug, serde::Deserialize)]
struct RefreshTokenResponse {
    #[serde(default)]
    id_token: Option<String>,
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
}

/// Ensure the account has a non-expired ChatGPT access token.
/// Returns an updated account when a refresh was performed.
pub async fn ensure_chatgpt_tokens_fresh(account: &StoredAccount) -> Result<StoredAccount> {
    match &account.auth_data {
        AuthData::ApiKey { .. } => Ok(account.clone()),
        AuthData::ChatGPT { access_token, .. } => {
            if token_expired_or_near_expiry(access_token) {
                println!(
                    "[Auth] Access token expired/near expiry for account {}, refreshing",
                    account.name
                );
                refresh_chatgpt_tokens(account).await
            } else {
                Ok(account.clone())
            }
        }
    }
}

/// Force-refresh ChatGPT OAuth tokens for an account.
pub async fn refresh_chatgpt_tokens(account: &StoredAccount) -> Result<StoredAccount> {
    if matches!(account.auth_data, AuthData::ApiKey { .. }) {
        return Ok(account.clone());
    }

    let refresh_lock = account_refresh_lock(&account.id).await;
    let _guard = refresh_lock.lock().await;

    let latest_account = load_accounts()?
        .accounts
        .into_iter()
        .find(|candidate| candidate.id == account.id)
        .unwrap_or_else(|| account.clone());
    let latest_account = sync_account_from_current_auth_if_matching(&latest_account)?;

    if chatgpt_refresh_token(&latest_account) != chatgpt_refresh_token(account)
        && !chatgpt_access_token_expired_or_near_expiry(&latest_account)
    {
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
        anyhow::bail!("Missing refresh token for account {}", latest_account.name);
    }

    let refreshed = refresh_tokens_with_refresh_token(&current_refresh_token).await?;
    let next_id_token = refreshed.id_token.unwrap_or(current_id_token);
    let next_refresh_token = refreshed
        .refresh_token
        .unwrap_or_else(|| current_refresh_token.clone());

    let claims = parse_chatgpt_id_token_claims(&next_id_token);
    let next_account_id = claims.account_id.or(current_account_id);

    let is_active =
        load_accounts()?.active_account_id.as_deref() == Some(latest_account.id.as_str());

    let updated = update_account_chatgpt_tokens(
        &latest_account.id,
        next_id_token,
        refreshed.access_token,
        next_refresh_token,
        next_account_id,
        claims.email,
        claims.plan_type,
        claims.subscription_expires_at,
    )?;

    // Keep ~/.codex/auth.json in sync when this is the active account.
    if is_active {
        if let Err(err) = switch_to_account(&updated) {
            println!("[Auth] Failed to sync active auth.json after token refresh: {err}");
        }
    }

    Ok(updated)
}

async fn account_refresh_lock(account_id: &str) -> Arc<Mutex<()>> {
    let locks = REFRESH_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = locks.lock().await;
    map.entry(account_id.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

fn sync_account_from_current_auth_if_matching(account: &StoredAccount) -> Result<StoredAccount> {
    let AuthData::ChatGPT {
        id_token: stored_id_token,
        access_token: stored_access_token,
        refresh_token: stored_refresh_token,
        account_id: stored_account_id,
    } = &account.auth_data
    else {
        return Ok(account.clone());
    };

    let Some(auth) = read_current_auth()? else {
        return Ok(account.clone());
    };
    let Some(tokens) = auth.tokens else {
        return Ok(account.clone());
    };

    if !same_chatgpt_identity(account, &tokens) {
        return Ok(account.clone());
    }

    if tokens.id_token == *stored_id_token
        && tokens.access_token == *stored_access_token
        && tokens.refresh_token == *stored_refresh_token
    {
        return Ok(account.clone());
    }

    let claims = parse_chatgpt_id_token_claims(&tokens.id_token);
    let next_account_id = claims.account_id.or_else(|| stored_account_id.clone());

    update_account_chatgpt_tokens(
        &account.id,
        tokens.id_token,
        tokens.access_token,
        tokens.refresh_token,
        next_account_id,
        claims.email,
        claims.plan_type,
        claims.subscription_expires_at,
    )
}

fn same_chatgpt_identity(account: &StoredAccount, tokens: &TokenData) -> bool {
    let claims = parse_chatgpt_id_token_claims(&tokens.id_token);
    let stored_account_id = match &account.auth_data {
        AuthData::ChatGPT { account_id, .. } => account_id.as_deref(),
        AuthData::ApiKey { .. } => return false,
    };

    if let (Some(stored), Some(current)) = (stored_account_id, claims.account_id.as_deref()) {
        return stored == current;
    }

    if let (Some(stored), Some(current)) = (account.email.as_deref(), claims.email.as_deref()) {
        return stored.eq_ignore_ascii_case(current);
    }

    false
}

fn chatgpt_refresh_token(account: &StoredAccount) -> Option<&str> {
    match &account.auth_data {
        AuthData::ChatGPT { refresh_token, .. } => Some(refresh_token.as_str()),
        AuthData::ApiKey { .. } => None,
    }
}

fn chatgpt_access_token_expired_or_near_expiry(account: &StoredAccount) -> bool {
    match &account.auth_data {
        AuthData::ChatGPT { access_token, .. } => token_expired_or_near_expiry(access_token),
        AuthData::ApiKey { .. } => false,
    }
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
        anyhow::bail!("Token refresh failed: {status} - {body}");
    }

    response
        .json::<RefreshTokenResponse>()
        .await
        .context("Failed to parse token refresh response")
}

#[cfg(test)]
mod tests {
    use super::same_chatgpt_identity;
    use crate::types::{StoredAccount, TokenData};
    use base64::Engine;

    #[test]
    fn same_chatgpt_identity_matches_account_id() {
        let account = StoredAccount::new_chatgpt(
            "work".to_string(),
            Some("user@example.com".to_string()),
            None,
            None,
            id_token(Some("user@example.com"), Some("acc_123")),
            "access".to_string(),
            "refresh".to_string(),
            Some("acc_123".to_string()),
        );
        let tokens = token_data(Some("other@example.com"), Some("acc_123"));

        assert!(same_chatgpt_identity(&account, &tokens));
    }

    #[test]
    fn same_chatgpt_identity_falls_back_to_email() {
        let account = StoredAccount::new_chatgpt(
            "work".to_string(),
            Some("User@Example.com".to_string()),
            None,
            None,
            id_token(Some("User@Example.com"), None),
            "access".to_string(),
            "refresh".to_string(),
            None,
        );
        let tokens = token_data(Some("user@example.com"), None);

        assert!(same_chatgpt_identity(&account, &tokens));
    }

    #[test]
    fn same_chatgpt_identity_rejects_different_account() {
        let account = StoredAccount::new_chatgpt(
            "work".to_string(),
            Some("user@example.com".to_string()),
            None,
            None,
            id_token(Some("user@example.com"), Some("acc_123")),
            "access".to_string(),
            "refresh".to_string(),
            Some("acc_123".to_string()),
        );
        let tokens = token_data(Some("user@example.com"), Some("acc_456"));

        assert!(!same_chatgpt_identity(&account, &tokens));
    }

    fn token_data(email: Option<&str>, account_id: Option<&str>) -> TokenData {
        TokenData {
            id_token: id_token(email, account_id),
            access_token: "next-access".to_string(),
            refresh_token: "next-refresh".to_string(),
            account_id: account_id.map(String::from),
        }
    }

    fn id_token(email: Option<&str>, account_id: Option<&str>) -> String {
        let mut payload = serde_json::Map::new();
        if let Some(email) = email {
            payload.insert("email".to_string(), serde_json::json!(email));
        }

        let mut auth = serde_json::Map::new();
        if let Some(account_id) = account_id {
            auth.insert(
                "chatgpt_account_id".to_string(),
                serde_json::json!(account_id),
            );
        }
        payload.insert(
            "https://api.openai.com/auth".to_string(),
            serde_json::Value::Object(auth),
        );

        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::Value::Object(payload).to_string());
        format!("header.{encoded}.signature")
    }
}
