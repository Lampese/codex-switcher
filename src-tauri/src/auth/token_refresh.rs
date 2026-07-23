//! ChatGPT OAuth token refresh helpers

use anyhow::{Context, Result};
use base64::Engine;
use chrono::Utc;
use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};
use tokio::time::{sleep, Duration};

use super::{load_accounts, switch_to_account, update_account_chatgpt_tokens};
use crate::types::{parse_chatgpt_id_token_claims, AuthData, StoredAccount};

const DEFAULT_ISSUER: &str = "https://auth.openai.com";
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
/// Refresh the token only when it expires within 5 minutes.
/// The old value was 60 seconds which caused unnecessary refreshes on
/// every warmup/usage cycle, burning refresh tokens and causing
/// refresh_token_reused errors when Codex CLI or another instance
/// also refreshed at the same time.
const EXPIRY_SKEW_SECONDS: i64 = 5 * 60;

/// In-memory set of account IDs currently being refreshed.
/// Prevents two concurrent calls from both starting a refresh for the
/// same account (race that burns the refresh token).
static REFRESHING_IDS: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

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
/// Uses a per-account in-memory lock so concurrent callers (warmup +
/// usage refresh polling) never both trigger a refresh for the same
/// account simultaneously — which would burn the refresh token.
pub async fn refresh_chatgpt_tokens(account: &StoredAccount) -> Result<StoredAccount> {
    let (current_id_token, current_refresh_token, current_account_id) = match &account.auth_data {
        AuthData::ApiKey { .. } => return Ok(account.clone()),
        AuthData::ChatGPT {
            id_token,
            refresh_token,
            account_id,
            ..
        } => (id_token.clone(), refresh_token.clone(), account_id.clone()),
    };

    if current_refresh_token.is_empty() {
        anyhow::bail!("Missing refresh token for account {}", account.name);
    }

    // Try to acquire the per-account refresh lock.
    // If another task is already refreshing this account, skip and return
    // the current (still-valid-for-a-moment) account to avoid double-refresh.
    let acquired = {
        if let Ok(mut set) = REFRESHING_IDS.lock() {
            set.insert(account.id.clone())
        } else {
            false
        }
    };

    if !acquired {
        println!(
            "[Auth] Refresh already in progress for account {}, skipping duplicate",
            account.name
        );
        return Ok(account.clone());
    }

    let result = refresh_tokens_with_refresh_token(&current_refresh_token).await;

    // Always release the lock, even on error.
    if let Ok(mut set) = REFRESHING_IDS.lock() {
        set.remove(&account.id);
    }

    let refreshed = result?;
    let next_id_token = refreshed.id_token.unwrap_or(current_id_token);
    let next_refresh_token = refreshed
        .refresh_token
        .unwrap_or_else(|| current_refresh_token.clone());

    let claims = parse_chatgpt_id_token_claims(&next_id_token);
    let next_account_id = claims.account_id.or(current_account_id);

    let is_active = load_accounts()?.active_account_id.as_deref() == Some(account.id.as_str());

    let updated = update_account_chatgpt_tokens(
        &account.id,
        next_id_token,
        refreshed.access_token,
        next_refresh_token,
        next_account_id,
        claims.email,
        claims.plan_type,
        claims.subscription_expires_at,
    )?;

    if is_active {
        if let Err(err) = switch_to_account(&updated) {
            println!("[Auth] Failed to sync active auth.json after token refresh: {err}");
        }
    }

    Ok(updated)
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
