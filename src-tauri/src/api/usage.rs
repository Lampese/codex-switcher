//! Usage API client for fetching rate limits and credits

use anyhow::{Context, Result};
use base64::Engine as _;
use chrono::{DateTime, SecondsFormat, Utc};
use crypto_box::SecretKey as Curve25519SecretKey;
use ed25519_dalek::pkcs8::DecodePrivateKey;
use ed25519_dalek::{Signer as _, SigningKey};
use futures::{stream, StreamExt};
use reqwest::{
    header::{HeaderMap, HeaderName, HeaderValue, AUTHORIZATION, USER_AGENT},
    StatusCode,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest as _, Sha512};
use std::collections::{BTreeMap, HashMap};

use crate::auth::{ensure_chatgpt_tokens_fresh, refresh_chatgpt_tokens};
use crate::types::{
    parse_codex_access_token_claims, AuthData, CreditStatusDetails, RateLimitDetails,
    RateLimitStatusPayload, RateLimitWindow, StoredAccount, UsageInfo,
};

const CHATGPT_BACKEND_API: &str = "https://chatgpt.com/backend-api";
const CHATGPT_ACCOUNTS_CHECK_API: &str =
    "https://chatgpt.com/backend-api/accounts/check/v4-2023-04-27";
const CHATGPT_CODEX_USAGE_API: &str = "https://chatgpt.com/backend-api/wham/usage";
const CHATGPT_CODEX_RESPONSES_API: &str = "https://chatgpt.com/backend-api/codex/responses";
const AGENT_IDENTITY_AUTHAPI_BASE_URL: &str = "https://auth.openai.com/api/accounts";
const OPENAI_API: &str = "https://api.openai.com/v1";
const CODEX_USER_AGENT: &str = "codex-cli/1.0.0";

#[derive(Debug, Clone)]
pub struct ChatGptAccountMetadata {
    pub email: Option<String>,
    pub plan_type: Option<String>,
    pub subscription_expires_at: Option<DateTime<Utc>>,
}

fn codex_usage_url() -> &'static str {
    CHATGPT_CODEX_USAGE_API
}

#[derive(Debug, Deserialize)]
struct AccountsCheckResponse {
    #[serde(default)]
    accounts: HashMap<String, AccountsCheckEntry>,
}

#[derive(Debug, Deserialize)]
struct AccountsCheckEntry {
    #[serde(default)]
    account: Option<AccountsCheckAccount>,
    #[serde(default)]
    entitlement: Option<AccountsCheckEntitlement>,
}

#[derive(Debug, Deserialize)]
struct AccountsCheckAccount {
    #[serde(default)]
    plan_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AccountsCheckEntitlement {
    #[serde(default)]
    expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
struct RegisterTaskRequest {
    timestamp: String,
    signature: String,
}

#[derive(Debug, Deserialize)]
struct RegisterTaskResponse {
    #[serde(default)]
    task_id: Option<String>,
    #[serde(default, rename = "taskId")]
    task_id_camel: Option<String>,
    #[serde(default)]
    encrypted_task_id: Option<String>,
    #[serde(default, rename = "encryptedTaskId")]
    encrypted_task_id_camel: Option<String>,
}

/// Get usage information for an account
pub async fn get_account_usage(account: &StoredAccount) -> Result<UsageInfo> {
    println!("[Usage] Fetching usage for account: {}", account.name);

    match &account.auth_data {
        AuthData::ApiKey { .. } => {
            println!("[Usage] API key accounts don't support usage info");
            Ok(UsageInfo {
                account_id: account.id.clone(),
                plan_type: Some("api_key".to_string()),
                primary_used_percent: None,
                primary_window_minutes: None,
                primary_resets_at: None,
                secondary_used_percent: None,
                secondary_window_minutes: None,
                secondary_resets_at: None,
                has_credits: None,
                unlimited_credits: None,
                credits_balance: None,
                error: Some("Usage info not available for API key accounts".to_string()),
            })
        }
        AuthData::ChatGPT { .. } => get_usage_with_chatgpt_auth(account).await,
        AuthData::CodexAccessToken { .. } => get_usage_with_codex_access_token(account).await,
    }
}

/// Send a minimal authenticated request to warm up account traffic paths.
pub async fn warmup_account(account: &StoredAccount) -> Result<()> {
    println!(
        "[Warmup] Sending warm-up request for account: {}",
        account.name
    );

    match &account.auth_data {
        AuthData::ApiKey { key } => warmup_with_api_key(key).await,
        AuthData::ChatGPT { .. } => warmup_with_chatgpt_auth(account).await,
        AuthData::CodexAccessToken { .. } => {
            println!("[Warmup] Skipping warm-up for CODEX_ACCESS_TOKEN account");
            Ok(())
        }
    }
}

pub async fn fetch_chatgpt_account_metadata(
    account: &StoredAccount,
) -> Result<ChatGptAccountMetadata> {
    let (access_token, chatgpt_account_id) = extract_chatgpt_auth(account)?;
    let response =
        send_chatgpt_get_request(CHATGPT_ACCOUNTS_CHECK_API, access_token, chatgpt_account_id)
            .await?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Accounts check API error: {status} - {body}");
    }

    let payload: AccountsCheckResponse = response
        .json()
        .await
        .context("Failed to parse accounts check response")?;

    let selected_entry = chatgpt_account_id
        .and_then(|account_id| payload.accounts.get(account_id))
        .or_else(|| payload.accounts.get("default"))
        .or_else(|| payload.accounts.values().next())
        .context("Accounts check response did not include an account entry")?;

    Ok(ChatGptAccountMetadata {
        email: None,
        plan_type: selected_entry
            .account
            .as_ref()
            .and_then(|account| account.plan_type.clone()),
        subscription_expires_at: selected_entry
            .entitlement
            .as_ref()
            .and_then(|entitlement| entitlement.expires_at),
    })
}

pub async fn fetch_codex_access_token_account_metadata(
    account: &StoredAccount,
) -> Result<ChatGptAccountMetadata> {
    let AuthData::CodexAccessToken { token, .. } = &account.auth_data else {
        anyhow::bail!("Account is not using a Codex access token");
    };
    let claims = parse_codex_access_token_claims(token);

    Ok(ChatGptAccountMetadata {
        email: claims.email.or_else(|| account.email.clone()),
        plan_type: claims.plan_type.or_else(|| account.plan_type.clone()),
        subscription_expires_at: account.subscription_expires_at,
    })
}

async fn get_usage_with_chatgpt_auth(account: &StoredAccount) -> Result<UsageInfo> {
    let fresh_account = ensure_chatgpt_tokens_fresh(account).await?;
    let (access_token, chatgpt_account_id) = extract_chatgpt_auth(&fresh_account)?;

    let response = send_chatgpt_usage_request(access_token, chatgpt_account_id).await?;
    if response.status() == StatusCode::UNAUTHORIZED {
        println!(
            "[Usage] Unauthorized for account {}, refreshing token and retrying once",
            fresh_account.name
        );
        let refreshed_account = refresh_chatgpt_tokens(&fresh_account).await?;
        let (retry_token, retry_account_id) = extract_chatgpt_auth(&refreshed_account)?;
        let retry_response = send_chatgpt_usage_request(retry_token, retry_account_id).await?;
        return parse_usage_response(
            &refreshed_account.id,
            &refreshed_account.name,
            retry_response,
        )
        .await;
    }

    parse_usage_response(&fresh_account.id, &fresh_account.name, response).await
}

async fn get_usage_with_codex_access_token(account: &StoredAccount) -> Result<UsageInfo> {
    let headers = build_codex_access_token_headers(account).await?;
    let response = send_codex_usage_request(headers).await?;

    parse_usage_response(&account.id, &account.name, response).await
}

async fn parse_usage_response(
    account_id: &str,
    account_name: &str,
    response: reqwest::Response,
) -> Result<UsageInfo> {
    let status = response.status();
    println!("[Usage] Response status: {status}");

    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        println!("[Usage] Error response: {body}");
        return Ok(UsageInfo::error(
            account_id.to_string(),
            format!("API error: {status}"),
        ));
    }

    let body_text = response
        .text()
        .await
        .context("Failed to read response body")?;
    println!("[Usage] Response body received ({} bytes)", body_text.len());

    let payload: RateLimitStatusPayload =
        serde_json::from_str(&body_text).context("Failed to parse usage response")?;

    println!("[Usage] Parsed plan_type: {}", payload.plan_type);

    let usage = convert_payload_to_usage_info(account_id, payload);
    println!(
        "[Usage] {} - primary: {:?}%, plan: {:?}",
        account_name, usage.primary_used_percent, usage.plan_type
    );

    Ok(usage)
}

async fn warmup_with_chatgpt_auth(account: &StoredAccount) -> Result<()> {
    let fresh_account = ensure_chatgpt_tokens_fresh(account).await?;
    let (access_token, chatgpt_account_id) = extract_chatgpt_auth(&fresh_account)?;

    let mut response = send_chatgpt_warmup_request(access_token, chatgpt_account_id, true).await?;
    if response.status() == StatusCode::UNAUTHORIZED {
        println!(
            "[Warmup] Unauthorized for account {}, refreshing token and retrying once",
            fresh_account.name
        );
        let refreshed_account = refresh_chatgpt_tokens(&fresh_account).await?;
        let (retry_token, retry_account_id) = extract_chatgpt_auth(&refreshed_account)?;
        response = send_chatgpt_warmup_request(retry_token, retry_account_id, true).await?;
    }

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        println!("[Warmup] ChatGPT warm-up error response: {body}");
        anyhow::bail!("ChatGPT warm-up failed with status {status}");
    }

    let body = response.text().await.unwrap_or_default();
    log_warmup_response("ChatGPT", &body, true);

    Ok(())
}

async fn warmup_with_api_key(api_key: &str) -> Result<()> {
    let client = reqwest::Client::new();
    let payload = build_warmup_payload(false, true);
    let response = client
        .post(format!("{OPENAI_API}/responses"))
        .header(USER_AGENT, CODEX_USER_AGENT)
        .header(AUTHORIZATION, format!("Bearer {api_key}"))
        .json(&payload)
        .send()
        .await
        .context("Failed to send API key warm-up request")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        println!("[Warmup] API key warm-up error response: {body}");
        anyhow::bail!("API key warm-up failed with status {status}");
    }

    let body = response.text().await.unwrap_or_default();
    log_warmup_response("API key", &body, false);

    Ok(())
}

fn build_warmup_payload(stream: bool, include_max_output_tokens: bool) -> serde_json::Value {
    let mut payload = json!({
        "model": "gpt-5.4-mini",
        "instructions": "You are Codex.",
        "input": [
            {
                "type": "message",
                "role": "user",
                "content": [
                    {
                        "type": "input_text",
                        "text": "Hi"
                    }
                ]
            }
        ],
        "tools": [],
        "tool_choice": "auto",
        "parallel_tool_calls": false,
        "reasoning": {
            "effort": "low"
        },
        "store": false,
        "stream": stream
    });

    if include_max_output_tokens {
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("max_output_tokens".to_string(), json!(1));
        }
    }

    payload
}

fn build_chatgpt_headers(
    access_token: &str,
    chatgpt_account_id: Option<&str>,
) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static(CODEX_USER_AGENT));
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {access_token}")).context("Invalid access token")?,
    );

    if let Some(acc_id) = chatgpt_account_id {
        println!("[Usage] Using ChatGPT Account ID: {acc_id}");
        if let Ok(header_name) = HeaderName::from_bytes(b"chatgpt-account-id") {
            if let Ok(header_value) = HeaderValue::from_str(acc_id) {
                headers.insert(header_name, header_value);
            }
        }
    }

    Ok(headers)
}

pub(crate) async fn build_codex_access_token_headers(account: &StoredAccount) -> Result<HeaderMap> {
    let AuthData::CodexAccessToken {
        token,
        account_id,
        agent_runtime_id,
        agent_private_key,
        chatgpt_account_is_fedramp,
        task_id,
        ..
    } = &account.auth_data
    else {
        anyhow::bail!("Account is not using a Codex access token");
    };

    let claims = crate::types::parse_codex_access_token_claims(token);
    let account_id = account_id.as_deref().or(claims.account_id.as_deref());
    let agent_runtime_id = agent_runtime_id
        .as_deref()
        .or(claims.agent_runtime_id.as_deref());
    let agent_private_key = agent_private_key
        .as_deref()
        .or(claims.agent_private_key.as_deref());
    let is_fedramp = *chatgpt_account_is_fedramp || claims.chatgpt_account_is_fedramp;

    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static(CODEX_USER_AGENT));

    if let (Some(agent_runtime_id), Some(agent_private_key)) = (agent_runtime_id, agent_private_key)
    {
        let task_id = match task_id.as_deref().filter(|value| !value.trim().is_empty()) {
            Some(task_id) => task_id.to_string(),
            None => register_agent_task(agent_runtime_id, agent_private_key).await?,
        };
        let authorization =
            authorization_header_for_agent_task(agent_runtime_id, agent_private_key, &task_id)?;
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&authorization).context("Invalid Agent Identity header")?,
        );
    } else {
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", token.trim()))
                .context("Invalid access token")?,
        );
    }

    if let Some(acc_id) = account_id {
        println!("[Usage] Using Codex Account ID: {acc_id}");
        if let Ok(header_name) = HeaderName::from_bytes(b"ChatGPT-Account-ID") {
            if let Ok(header_value) = HeaderValue::from_str(acc_id) {
                headers.insert(header_name, header_value);
            }
        }
    }

    if is_fedramp {
        headers.insert(
            HeaderName::from_static("x-openai-fedramp"),
            HeaderValue::from_static("true"),
        );
    }

    Ok(headers)
}

async fn register_agent_task(agent_runtime_id: &str, agent_private_key: &str) -> Result<String> {
    let timestamp = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let request = RegisterTaskRequest {
        timestamp: timestamp.clone(),
        signature: sign_task_registration_payload(agent_runtime_id, agent_private_key, &timestamp)?,
    };
    let url = format!(
        "{}/v1/agent/{}/task/register",
        AGENT_IDENTITY_AUTHAPI_BASE_URL.trim_end_matches('/'),
        agent_runtime_id
    );

    let response = reqwest::Client::new()
        .post(&url)
        .json(&request)
        .send()
        .await
        .with_context(|| format!("Failed to register Agent Identity task at {url}"))?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Agent Identity task registration failed: {status} - {body}");
    }

    let payload: RegisterTaskResponse = response
        .json()
        .await
        .context("Failed to parse Agent Identity task registration response")?;

    task_id_from_register_task_response(agent_private_key, payload)
}

fn task_id_from_register_task_response(
    private_key_pkcs8_base64: &str,
    response: RegisterTaskResponse,
) -> Result<String> {
    if let Some(task_id) = response.task_id.or(response.task_id_camel) {
        return Ok(task_id);
    }

    let encrypted_task_id = response
        .encrypted_task_id
        .or(response.encrypted_task_id_camel)
        .context("Agent Identity task registration response omitted task_id")?;

    decrypt_task_id_response(private_key_pkcs8_base64, &encrypted_task_id)
}

fn sign_task_registration_payload(
    agent_runtime_id: &str,
    private_key_pkcs8_base64: &str,
    timestamp: &str,
) -> Result<String> {
    sign_agent_identity_payload(
        private_key_pkcs8_base64,
        &format!("{agent_runtime_id}:{timestamp}"),
    )
}

fn authorization_header_for_agent_task(
    agent_runtime_id: &str,
    private_key_pkcs8_base64: &str,
    task_id: &str,
) -> Result<String> {
    let timestamp = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let signature = sign_agent_identity_payload(
        private_key_pkcs8_base64,
        &format!("{agent_runtime_id}:{task_id}:{timestamp}"),
    )?;
    let payload = serde_json::to_vec(&BTreeMap::from([
        ("agent_runtime_id", agent_runtime_id),
        ("signature", signature.as_str()),
        ("task_id", task_id),
        ("timestamp", timestamp.as_str()),
    ]))
    .context("Failed to serialize Agent Identity assertion")?;
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload);
    Ok(format!("AgentAssertion {encoded}"))
}

fn sign_agent_identity_payload(private_key_pkcs8_base64: &str, payload: &str) -> Result<String> {
    let signing_key = signing_key_from_private_key_pkcs8_base64(private_key_pkcs8_base64)?;
    let signature = signing_key.sign(payload.as_bytes());
    Ok(base64::engine::general_purpose::STANDARD.encode(signature.to_bytes()))
}

fn decrypt_task_id_response(
    private_key_pkcs8_base64: &str,
    encrypted_task_id: &str,
) -> Result<String> {
    let signing_key = signing_key_from_private_key_pkcs8_base64(private_key_pkcs8_base64)?;
    let ciphertext = base64::engine::general_purpose::STANDARD
        .decode(encrypted_task_id)
        .context("Encrypted Agent Identity task id is not valid base64")?;
    let plaintext = curve25519_secret_key_from_signing_key(&signing_key)
        .unseal(&ciphertext)
        .map_err(|_| anyhow::anyhow!("Failed to decrypt Agent Identity task id"))?;
    String::from_utf8(plaintext).context("Decrypted Agent Identity task id is not valid UTF-8")
}

fn signing_key_from_private_key_pkcs8_base64(private_key_pkcs8_base64: &str) -> Result<SigningKey> {
    let private_key = base64::engine::general_purpose::STANDARD
        .decode(private_key_pkcs8_base64)
        .context("Agent Identity private key is not valid base64")?;
    SigningKey::from_pkcs8_der(&private_key)
        .context("Agent Identity private key is not valid PKCS#8")
}

fn curve25519_secret_key_from_signing_key(signing_key: &SigningKey) -> Curve25519SecretKey {
    let digest = Sha512::digest(signing_key.to_bytes());
    let mut secret_key = [0u8; 32];
    secret_key.copy_from_slice(&digest[..32]);
    secret_key[0] &= 248;
    secret_key[31] &= 127;
    secret_key[31] |= 64;
    Curve25519SecretKey::from_bytes(secret_key)
}

fn extract_chatgpt_auth(account: &StoredAccount) -> Result<(&str, Option<&str>)> {
    match &account.auth_data {
        AuthData::ChatGPT {
            access_token,
            account_id,
            ..
        } => Ok((access_token.as_str(), account_id.as_deref())),
        AuthData::CodexAccessToken {
            token, account_id, ..
        } => Ok((token.as_str(), account_id.as_deref())),
        AuthData::ApiKey { .. } => anyhow::bail!("Account is not using ChatGPT OAuth"),
    }
}

async fn send_chatgpt_usage_request(
    access_token: &str,
    chatgpt_account_id: Option<&str>,
) -> Result<reqwest::Response> {
    send_chatgpt_get_request(
        &format!("{CHATGPT_BACKEND_API}/wham/usage"),
        access_token,
        chatgpt_account_id,
    )
    .await
}

async fn send_codex_usage_request(headers: HeaderMap) -> Result<reqwest::Response> {
    send_get_request_with_headers(codex_usage_url(), headers).await
}

async fn send_chatgpt_get_request(
    url: &str,
    access_token: &str,
    chatgpt_account_id: Option<&str>,
) -> Result<reqwest::Response> {
    let headers = build_chatgpt_headers(access_token, chatgpt_account_id)?;
    send_get_request_with_headers(url, headers).await
}

async fn send_get_request_with_headers(url: &str, headers: HeaderMap) -> Result<reqwest::Response> {
    println!("[Usage] Requesting: {url}");

    reqwest::Client::new()
        .get(url)
        .headers(headers)
        .send()
        .await
        .with_context(|| format!("Failed to send GET request to {url}"))
}

async fn send_chatgpt_warmup_request(
    access_token: &str,
    chatgpt_account_id: Option<&str>,
    stream: bool,
) -> Result<reqwest::Response> {
    let client = reqwest::Client::new();
    let headers = build_chatgpt_headers(access_token, chatgpt_account_id)?;
    let payload = build_warmup_payload(stream, false);

    client
        .post(CHATGPT_CODEX_RESPONSES_API)
        .headers(headers)
        .json(&payload)
        .send()
        .await
        .context("Failed to send ChatGPT warm-up request")
}

fn log_warmup_response(source: &str, body: &str, is_sse: bool) {
    if body.trim().is_empty() {
        println!("[Warmup] {source} warm-up response was empty");
        return;
    }

    let preview = truncate_text(body, 300);
    println!("[Warmup] {source} warm-up response preview: {preview}");

    let extracted = if is_sse {
        extract_text_from_sse(body)
    } else {
        extract_text_from_json(body)
    };

    if let Some(message) = extracted {
        let message_preview = truncate_text(&message, 200);
        println!("[Warmup] {source} warm-up message: {message_preview}");
    }
}

fn truncate_text(text: &str, max_len: usize) -> String {
    if text.len() <= max_len {
        return text.to_string();
    }
    let mut out = text[..max_len].to_string();
    out.push_str("...");
    out
}

fn extract_text_from_sse(body: &str) -> Option<String> {
    let mut last_text: Option<String> = None;
    for line in body.lines() {
        let line = line.trim();
        if !line.starts_with("data:") {
            continue;
        }
        let data = line.trim_start_matches("data:").trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(data) {
            if let Some(text) = extract_last_text_from_value(&value) {
                last_text = Some(text);
            }
        }
    }
    last_text.filter(|text| !text.trim().is_empty())
}

fn extract_text_from_json(body: &str) -> Option<String> {
    let value = serde_json::from_str::<Value>(body).ok()?;
    extract_last_text_from_value(&value)
}

fn extract_last_text_from_value(value: &Value) -> Option<String> {
    let mut last: Option<String> = None;
    collect_last_text(value, &mut last);
    last
}

fn collect_last_text(value: &Value, last: &mut Option<String>) {
    match value {
        Value::Object(map) => {
            for (key, val) in map {
                if matches!(key.as_str(), "text" | "delta" | "output_text") {
                    if let Value::String(text) = val {
                        if !text.is_empty() {
                            *last = Some(text.clone());
                        }
                    }
                }
                collect_last_text(val, last);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_last_text(item, last);
            }
        }
        _ => {}
    }
}

/// Convert API response to UsageInfo
fn convert_payload_to_usage_info(account_id: &str, payload: RateLimitStatusPayload) -> UsageInfo {
    let (primary, secondary) = extract_rate_limits(payload.rate_limit);
    let credits = extract_credits(payload.credits);

    UsageInfo {
        account_id: account_id.to_string(),
        plan_type: Some(payload.plan_type),
        primary_used_percent: primary.as_ref().map(|w| w.used_percent),
        primary_window_minutes: primary.as_ref().and_then(window_minutes),
        primary_resets_at: primary.as_ref().and_then(|w| w.reset_at),
        secondary_used_percent: secondary.as_ref().map(|w| w.used_percent),
        secondary_window_minutes: secondary.as_ref().and_then(window_minutes),
        secondary_resets_at: secondary.as_ref().and_then(|w| w.reset_at),
        has_credits: credits.as_ref().map(|c| c.has_credits),
        unlimited_credits: credits.as_ref().map(|c| c.unlimited),
        credits_balance: credits.and_then(|c| c.balance),
        error: None,
    }
}

fn window_minutes(window: &RateLimitWindow) -> Option<i64> {
    window.window_duration_mins.or_else(|| {
        window
            .limit_window_seconds
            .map(|s| (i64::from(s) + 59) / 60)
    })
}

fn extract_rate_limits(
    rate_limit: Option<RateLimitDetails>,
) -> (Option<RateLimitWindow>, Option<RateLimitWindow>) {
    match rate_limit {
        Some(details) => (details.primary_window, details.secondary_window),
        None => (None, None),
    }
}

fn extract_credits(credits: Option<CreditStatusDetails>) -> Option<CreditStatusDetails> {
    credits
}

/// Refresh all account usage
pub async fn refresh_all_usage(accounts: &[StoredAccount]) -> Vec<UsageInfo> {
    println!("[Usage] Refreshing usage for {} accounts", accounts.len());

    let concurrency = accounts.len().min(10).max(1);
    let results: Vec<UsageInfo> = stream::iter(accounts.iter().cloned())
        .map(|account| async move {
            match get_account_usage(&account).await {
                Ok(info) => info,
                Err(e) => {
                    println!("[Usage] Error for {}: {}", account.name, e);
                    UsageInfo::error(account.id.clone(), e.to_string())
                }
            }
        })
        .buffer_unordered(concurrency)
        .collect()
        .await;

    println!("[Usage] Refresh complete");
    results
}

#[cfg(test)]
mod tests {
    use super::{
        authorization_header_for_agent_task, build_codex_access_token_headers, codex_usage_url,
        convert_payload_to_usage_info,
    };
    use crate::types::{AuthData, AuthMode, RateLimitStatusPayload, StoredAccount};
    use base64::Engine as _;
    use chrono::Utc;
    use ed25519_dalek::pkcs8::EncodePrivateKey;
    use ed25519_dalek::SigningKey;
    use reqwest::header::AUTHORIZATION;
    use serde_json::json;
    use uuid::Uuid;

    #[test]
    fn codex_access_token_usage_uses_chatgpt_wham_usage_endpoint() {
        assert_eq!(
            codex_usage_url(),
            "https://chatgpt.com/backend-api/wham/usage"
        );
    }

    #[test]
    fn codex_access_token_headers_include_account_id_when_available() {
        let headers = super::build_chatgpt_headers("token", Some("acc_123")).unwrap();

        assert_eq!(
            headers
                .get("ChatGPT-Account-Id")
                .and_then(|value| value.to_str().ok()),
            Some("acc_123")
        );
    }

    #[test]
    fn agent_identity_authorization_header_uses_agent_assertion_scheme() {
        let signing_key = SigningKey::from_bytes(&[7u8; 32]);
        let private_key = signing_key.to_pkcs8_der().unwrap();
        let private_key_base64 =
            base64::engine::general_purpose::STANDARD.encode(private_key.as_bytes());

        let header = authorization_header_for_agent_task(
            "agent-runtime-123",
            &private_key_base64,
            "task-123",
        )
        .unwrap();
        let encoded = header.strip_prefix("AgentAssertion ").unwrap();
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded)
            .unwrap();
        let envelope: serde_json::Value = serde_json::from_slice(&payload).unwrap();

        assert_eq!(envelope["agent_runtime_id"], "agent-runtime-123");
        assert_eq!(envelope["task_id"], "task-123");
        assert!(envelope["timestamp"].as_str().is_some());
        assert!(envelope["signature"].as_str().is_some());
    }

    #[tokio::test]
    async fn codex_access_token_headers_use_agent_assertion_when_agent_claims_exist() {
        let signing_key = SigningKey::from_bytes(&[8u8; 32]);
        let private_key = signing_key.to_pkcs8_der().unwrap();
        let private_key_base64 =
            base64::engine::general_purpose::STANDARD.encode(private_key.as_bytes());
        let sample_access_token = ["header", "payload", "signature"].join(".");
        let account = StoredAccount {
            id: Uuid::new_v4().to_string(),
            name: "agent".to_string(),
            email: Some("agent@example.com".to_string()),
            plan_type: Some("k12".to_string()),
            subscription_expires_at: None,
            auth_mode: AuthMode::CodexAccessToken,
            auth_data: AuthData::CodexAccessToken {
                token: sample_access_token,
                account_id: Some("account-123".to_string()),
                agent_runtime_id: Some("agent-runtime-123".to_string()),
                agent_private_key: Some(private_key_base64),
                chatgpt_user_id: Some("user-123".to_string()),
                chatgpt_account_is_fedramp: false,
                task_id: Some("task-123".to_string()),
            },
            created_at: Utc::now(),
            last_used_at: None,
        };

        let headers = build_codex_access_token_headers(&account).await.unwrap();

        assert_eq!(
            headers
                .get("ChatGPT-Account-ID")
                .and_then(|value| value.to_str().ok()),
            Some("account-123")
        );
        assert!(headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("AgentAssertion ")));
    }

    #[tokio::test]
    async fn codex_access_token_metadata_comes_from_token_claims() {
        let payload = r#"{"email":"k12@example.com","plan_type":"k12","account_id":"account-123","agent_runtime_id":"agent-runtime-123"}"#;
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload);
        let account =
            StoredAccount::new_codex_access_token("agent".to_string(), format!("h.{encoded}.s"));

        let metadata = super::fetch_codex_access_token_account_metadata(&account)
            .await
            .unwrap();

        assert_eq!(metadata.email.as_deref(), Some("k12@example.com"));
        assert_eq!(metadata.plan_type.as_deref(), Some("k12"));
    }

    #[test]
    fn converts_codex_usage_payload_aliases() {
        let payload: RateLimitStatusPayload = serde_json::from_value(json!({
            "plan_type": "k12",
            "rate_limit": {
                "primary": {
                    "used_percent": 12.5,
                    "window_duration_mins": 300,
                    "resets_at": 1783250000
                },
                "secondary": {
                    "used_percent": 2.25,
                    "window_duration_mins": 10080,
                    "resets_at": 1783850000
                }
            },
            "credits": {
                "has_credits": true,
                "unlimited": false,
                "balance": "$10.00"
            }
        }))
        .unwrap();

        let usage = convert_payload_to_usage_info("acc_123", payload);

        assert_eq!(usage.plan_type.as_deref(), Some("k12"));
        assert_eq!(usage.primary_used_percent, Some(12.5));
        assert_eq!(usage.primary_window_minutes, Some(300));
        assert_eq!(usage.primary_resets_at, Some(1783250000));
        assert_eq!(usage.secondary_used_percent, Some(2.25));
        assert_eq!(usage.secondary_window_minutes, Some(10080));
        assert_eq!(usage.secondary_resets_at, Some(1783850000));
        assert_eq!(usage.has_credits, Some(true));
        assert_eq!(usage.unlimited_credits, Some(false));
        assert_eq!(usage.credits_balance.as_deref(), Some("$10.00"));
        assert_eq!(usage.error, None);
    }
}
