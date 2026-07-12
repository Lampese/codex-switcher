//! ChatGPT OAuth token refresh helpers

use anyhow::{Context, Result};
use base64::Engine;
use chrono::Utc;
use tokio::time::{sleep, Duration};

use super::{mutate_accounts, switch_to_account};
use crate::types::{parse_chatgpt_id_token_claims, AuthData, StoredAccount};

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

/// Ensure the account has a non-expired ChatGPT access token.
/// Returns an updated account when a refresh was performed.
pub async fn ensure_chatgpt_tokens_fresh(account: &StoredAccount) -> Result<StoredAccount> {
    match &account.auth_data {
        AuthData::ApiKey { .. } | AuthData::CodexAccessToken { .. } => Ok(account.clone()),
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
    let (current_id_token, current_refresh_token, current_account_id) = match &account.auth_data {
        AuthData::ApiKey { .. } | AuthData::CodexAccessToken { .. } => return Ok(account.clone()),
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

    let refreshed = refresh_tokens_with_refresh_token(&current_refresh_token).await?;
    let next_id_token = refreshed.id_token.unwrap_or(current_id_token);
    let next_refresh_token = refreshed
        .refresh_token
        .unwrap_or_else(|| current_refresh_token.clone());

    let claims = parse_chatgpt_id_token_claims(&next_id_token);
    let next_account_id = claims.account_id.or(current_account_id);

    mutate_accounts(|store| {
        let stored = store
            .accounts
            .iter_mut()
            .find(|a| a.id == account.id)
            .context("Account not found")?;

        match &mut stored.auth_data {
            AuthData::ChatGPT {
                id_token,
                access_token,
                refresh_token,
                account_id,
            } => {
                *id_token = next_id_token;
                *access_token = refreshed.access_token;
                *refresh_token = next_refresh_token;
                if let Some(new_account_id) = next_account_id {
                    *account_id = Some(new_account_id);
                }
            }
            AuthData::ApiKey { .. } | AuthData::CodexAccessToken { .. } => {
                anyhow::bail!("Cannot update OAuth tokens for this account type");
            }
        }

        if let Some(email) = claims.email {
            stored.email = Some(email);
        }
        if let Some(plan_type) = claims.plan_type {
            stored.plan_type = Some(plan_type);
        }
        if let Some(expires_at) = claims.subscription_expires_at {
            stored.subscription_expires_at = Some(expires_at);
        }

        let updated = stored.clone();

        // Keep ~/.codex/auth.json in sync when this is the active account.
        // Checked and written under the store lock so a concurrent switch
        // can't interleave and end up with auth.json holding stale tokens or
        // the wrong account. Best-effort: the refreshed tokens must persist
        // even if this sync write fails (the old refresh token may already
        // be invalidated server-side).
        if store.active_account_id.as_deref() == Some(updated.id.as_str()) {
            if let Err(err) = switch_to_account(&updated) {
                println!("[Auth] Failed to sync active auth.json after token refresh: {err}");
            }
        }

        Ok(updated)
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
