//! Shared helpers for working with ChatGPT OAuth tokens.

use base64::Engine;
use chrono::Utc;

#[derive(Debug, Clone, Default)]
pub struct ChatGptTokenClaims {
    pub email: Option<String>,
    pub plan_type: Option<String>,
    pub account_id: Option<String>,
}

/// Parse claims from a JWT ID token without signature validation.
pub fn parse_chatgpt_token_claims(id_token: &str) -> ChatGptTokenClaims {
    let parts: Vec<&str> = id_token.split('.').collect();
    if parts.len() != 3 {
        return ChatGptTokenClaims::default();
    }

    let payload = match base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(parts[1]) {
        Ok(bytes) => bytes,
        Err(_) => return ChatGptTokenClaims::default(),
    };

    let json: serde_json::Value = match serde_json::from_slice(&payload) {
        Ok(v) => v,
        Err(_) => return ChatGptTokenClaims::default(),
    };

    let auth_claims = json.get("https://api.openai.com/auth");

    ChatGptTokenClaims {
        email: json
            .get("email")
            .and_then(|value| value.as_str())
            .map(String::from),
        plan_type: auth_claims
            .and_then(|auth| auth.get("chatgpt_plan_type"))
            .and_then(|value| value.as_str())
            .map(String::from),
        account_id: auth_claims
            .and_then(|auth| auth.get("chatgpt_account_id"))
            .and_then(|value| value.as_str())
            .map(String::from),
    }
}

/// Parse the exp claim from any JWT without signature validation.
pub fn parse_jwt_exp(token: &str) -> Option<i64> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return None;
    }

    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(parts[1])
        .ok()?;
    let json: serde_json::Value = serde_json::from_slice(&payload).ok()?;
    json.get("exp").and_then(|value| value.as_i64())
}

pub fn token_expired_or_near_expiry(access_token: &str, skew_seconds: i64) -> bool {
    match parse_jwt_exp(access_token) {
        Some(expiry) => expiry <= Utc::now().timestamp() + skew_seconds,
        None => false,
    }
}
