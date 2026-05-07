//! Device-code ChatGPT login flow.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use reqwest::StatusCode;
use serde::{de, Deserialize, Deserializer, Serialize};
use tokio::sync::oneshot;

use super::oauth_server::{exchange_code_for_tokens, PkceCodes};
use crate::types::{parse_chatgpt_id_token_claims, DeviceLoginInfo, StoredAccount};

const DEFAULT_ISSUER: &str = "https://auth.openai.com";
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const DEVICE_TIMEOUT_SECONDS: u64 = 15 * 60;

#[derive(Debug, Clone)]
pub struct DeviceCode {
    verification_url: String,
    user_code: String,
    device_auth_id: String,
    interval_seconds: u64,
}

#[derive(Debug, Clone)]
pub struct DeviceLoginResult {
    pub account: StoredAccount,
}

#[derive(Debug, Deserialize)]
struct UserCodeResponse {
    device_auth_id: String,
    #[serde(alias = "usercode")]
    user_code: String,
    #[serde(
        default = "default_poll_interval",
        deserialize_with = "deserialize_interval"
    )]
    interval: u64,
}

#[derive(Debug, Serialize)]
struct UserCodeRequest {
    client_id: String,
}

#[derive(Debug, Serialize)]
struct TokenPollRequest {
    device_auth_id: String,
    user_code: String,
}

#[derive(Debug, Deserialize)]
struct DeviceAuthorizationResponse {
    authorization_code: String,
    code_challenge: String,
    code_verifier: String,
}

fn default_poll_interval() -> u64 {
    5
}

fn deserialize_interval<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    struct IntervalVisitor;

    impl<'de> de::Visitor<'de> for IntervalVisitor {
        type Value = u64;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a numeric interval or a numeric string")
        }

        fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
            Ok(value)
        }

        fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            u64::try_from(value).map_err(E::custom)
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            value.trim().parse::<u64>().map_err(E::custom)
        }
    }

    deserializer.deserialize_any(IntervalVisitor)
}

async fn request_user_code(client: &reqwest::Client) -> Result<DeviceCode> {
    let issuer = DEFAULT_ISSUER.trim_end_matches('/');
    let url = format!("{issuer}/api/accounts/deviceauth/usercode");
    let response = client
        .post(url)
        .json(&UserCodeRequest {
            client_id: CLIENT_ID.to_string(),
        })
        .send()
        .await
        .context("Failed to request device login code")?;

    if !response.status().is_success() {
        let status = response.status();
        if status == StatusCode::NOT_FOUND {
            anyhow::bail!(
                "Device code login is not enabled by the ChatGPT auth server. Use browser login instead."
            );
        }

        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Device code request failed: {status} - {body}");
    }

    let user_code: UserCodeResponse = response
        .json()
        .await
        .context("Failed to parse device login code response")?;

    Ok(DeviceCode {
        verification_url: format!("{issuer}/codex/device"),
        user_code: user_code.user_code,
        device_auth_id: user_code.device_auth_id,
        interval_seconds: user_code.interval.max(1),
    })
}

async fn poll_for_authorization_code(
    client: &reqwest::Client,
    device_code: &DeviceCode,
    cancelled: &AtomicBool,
) -> Result<DeviceAuthorizationResponse> {
    let issuer = DEFAULT_ISSUER.trim_end_matches('/');
    let url = format!("{issuer}/api/accounts/deviceauth/token");
    let timeout = Duration::from_secs(DEVICE_TIMEOUT_SECONDS);
    let start = Instant::now();

    loop {
        if cancelled.load(Ordering::Relaxed) {
            anyhow::bail!("Device login cancelled");
        }

        if start.elapsed() >= timeout {
            anyhow::bail!("Device login timed out after 15 minutes");
        }

        let response = client
            .post(&url)
            .json(&TokenPollRequest {
                device_auth_id: device_code.device_auth_id.clone(),
                user_code: device_code.user_code.clone(),
            })
            .send()
            .await
            .context("Failed to poll device login status")?;

        let status = response.status();
        if status.is_success() {
            return response
                .json()
                .await
                .context("Failed to parse device login authorization response");
        }

        if status != StatusCode::FORBIDDEN && status != StatusCode::NOT_FOUND {
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("Device login failed: {status} - {body}");
        }

        let remaining = timeout
            .checked_sub(start.elapsed())
            .unwrap_or_else(|| Duration::from_secs(0));
        let sleep_for = Duration::from_secs(device_code.interval_seconds).min(remaining);
        tokio::time::sleep(sleep_for).await;
    }
}

async fn complete_device_code_login(
    account_name: String,
    device_code: DeviceCode,
    cancelled: Arc<AtomicBool>,
) -> Result<DeviceLoginResult> {
    let client = reqwest::Client::new();
    let code_response = poll_for_authorization_code(&client, &device_code, &cancelled).await?;
    let pkce = PkceCodes {
        code_verifier: code_response.code_verifier,
        code_challenge: code_response.code_challenge,
    };
    let issuer = DEFAULT_ISSUER.trim_end_matches('/');
    let redirect_uri = format!("{issuer}/deviceauth/callback");
    let tokens = exchange_code_for_tokens(
        issuer,
        CLIENT_ID,
        &redirect_uri,
        &pkce,
        &code_response.authorization_code,
    )
    .await
    .context("Device login token exchange failed")?;
    let claims = parse_chatgpt_id_token_claims(&tokens.id_token);
    let account = StoredAccount::new_chatgpt(
        account_name,
        claims.email,
        claims.plan_type,
        claims.subscription_expires_at,
        tokens.id_token,
        tokens.access_token,
        tokens.refresh_token,
        claims.account_id,
    );

    Ok(DeviceLoginResult { account })
}

pub async fn start_device_code_login(
    account_name: String,
) -> Result<(
    DeviceLoginInfo,
    oneshot::Receiver<Result<DeviceLoginResult>>,
    Arc<AtomicBool>,
)> {
    let client = reqwest::Client::new();
    let device_code = request_user_code(&client).await?;
    let login_info = DeviceLoginInfo {
        verification_url: device_code.verification_url.clone(),
        user_code: device_code.user_code.clone(),
        expires_in_seconds: DEVICE_TIMEOUT_SECONDS,
        interval_seconds: device_code.interval_seconds,
    };
    let (tx, rx) = oneshot::channel();
    let cancelled = Arc::new(AtomicBool::new(false));
    let cancelled_clone = cancelled.clone();

    thread::spawn(move || {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let result = runtime.block_on(complete_device_code_login(
            account_name,
            device_code,
            cancelled_clone,
        ));
        let _ = tx.send(result);
    });

    Ok((login_info, rx, cancelled))
}

pub async fn wait_for_device_code_login(
    rx: oneshot::Receiver<Result<DeviceLoginResult>>,
) -> Result<StoredAccount> {
    let result = rx.await.context("Device login was cancelled")??;
    Ok(result.account)
}

#[cfg(test)]
mod tests {
    use super::UserCodeResponse;

    #[test]
    fn parses_user_code_response_with_string_interval() {
        let response: UserCodeResponse = serde_json::from_str(
            r#"{"device_auth_id":"dev_123","user_code":"ABCD-EFGH","interval":"7"}"#,
        )
        .unwrap();

        assert_eq!(response.device_auth_id, "dev_123");
        assert_eq!(response.user_code, "ABCD-EFGH");
        assert_eq!(response.interval, 7);
    }

    #[test]
    fn parses_user_code_alias_and_numeric_interval() {
        let response: UserCodeResponse = serde_json::from_str(
            r#"{"device_auth_id":"dev_456","usercode":"WXYZ-1234","interval":3}"#,
        )
        .unwrap();

        assert_eq!(response.user_code, "WXYZ-1234");
        assert_eq!(response.interval, 3);
    }
}
