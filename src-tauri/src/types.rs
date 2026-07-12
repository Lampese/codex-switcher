//! Core types for Codex Switcher

use base64::Engine;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The main storage structure for all accounts
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountsStore {
    /// Schema version for future migrations
    pub version: u32,
    /// List of all stored accounts
    pub accounts: Vec<StoredAccount>,
    /// Currently active account ID
    pub active_account_id: Option<String>,
    /// Set of account IDs that are masked (hidden)
    #[serde(default)]
    pub masked_account_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrayDisplayMode {
    IconAndSession,
    #[default]
    ActiveUsageText,
    Hidden,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DockDisplayMode {
    #[default]
    ShowInDock,
    MenuBarOnly,
}

fn default_close_behavior_prompt_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub tray_display_mode: TrayDisplayMode,
    pub dock_display_mode: DockDisplayMode,
    #[serde(default = "default_close_behavior_prompt_enabled")]
    pub close_behavior_prompt_enabled: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            tray_display_mode: TrayDisplayMode::default(),
            dock_display_mode: DockDisplayMode::default(),
            close_behavior_prompt_enabled: true,
        }
    }
}

impl Default for AccountsStore {
    fn default() -> Self {
        Self {
            version: 1,
            accounts: Vec::new(),
            active_account_id: None,
            masked_account_ids: Vec::new(),
        }
    }
}

/// A stored account with all its metadata and credentials
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredAccount {
    /// Unique identifier (UUID)
    pub id: String,
    /// User-defined display name
    pub name: String,
    /// Email extracted from ID token (for ChatGPT auth)
    pub email: Option<String>,
    /// Plan type: free, plus, pro, team, business, enterprise, edu
    pub plan_type: Option<String>,
    /// Subscription expiration extracted from ChatGPT ID token, when available
    #[serde(default)]
    pub subscription_expires_at: Option<DateTime<Utc>>,
    /// Authentication mode
    pub auth_mode: AuthMode,
    /// Authentication credentials
    pub auth_data: AuthData,
    /// When the account was added
    pub created_at: DateTime<Utc>,
    /// Last time this account was used
    pub last_used_at: Option<DateTime<Utc>>,
}

impl StoredAccount {
    /// Create a new account with API key authentication
    pub fn new_api_key(name: String, api_key: String) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name,
            email: None,
            plan_type: None,
            subscription_expires_at: None,
            auth_mode: AuthMode::ApiKey,
            auth_data: AuthData::ApiKey { key: api_key },
            created_at: Utc::now(),
            last_used_at: None,
        }
    }

    /// Create a new account with ChatGPT OAuth authentication
    pub fn new_chatgpt(
        name: String,
        email: Option<String>,
        plan_type: Option<String>,
        subscription_expires_at: Option<DateTime<Utc>>,
        id_token: String,
        access_token: String,
        refresh_token: String,
        account_id: Option<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name,
            email,
            plan_type,
            subscription_expires_at,
            auth_mode: AuthMode::ChatGPT,
            auth_data: AuthData::ChatGPT {
                id_token,
                access_token,
                refresh_token,
                account_id,
            },
            created_at: Utc::now(),
            last_used_at: None,
        }
    }

    /// Create a new account from a Codex CLI access token.
    pub fn new_codex_access_token(name: String, token: String) -> Self {
        let claims = parse_codex_access_token_claims(&token);

        Self {
            id: Uuid::new_v4().to_string(),
            name,
            email: claims.email.clone(),
            plan_type: claims.plan_type.clone(),
            subscription_expires_at: None,
            auth_mode: AuthMode::CodexAccessToken,
            auth_data: AuthData::CodexAccessToken {
                token,
                account_id: claims.account_id,
                agent_runtime_id: claims.agent_runtime_id,
                agent_private_key: claims.agent_private_key,
                chatgpt_user_id: claims.chatgpt_user_id,
                chatgpt_account_is_fedramp: claims.chatgpt_account_is_fedramp,
                task_id: None,
            },
            created_at: Utc::now(),
            last_used_at: None,
        }
    }
}

/// Authentication mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    /// Using an OpenAI API key
    ApiKey,
    /// Using ChatGPT OAuth tokens
    ChatGPT,
    /// Using a Codex CLI access token
    CodexAccessToken,
}

/// Authentication data (credentials)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthData {
    /// API key authentication
    ApiKey {
        /// The API key
        key: String,
    },
    /// ChatGPT OAuth authentication
    ChatGPT {
        /// JWT ID token containing user info
        id_token: String,
        /// Access token for API calls
        access_token: String,
        /// Refresh token for token renewal
        refresh_token: String,
        /// ChatGPT account ID
        account_id: Option<String>,
    },
    /// Codex CLI access token authentication
    CodexAccessToken {
        /// The raw CODEX_ACCESS_TOKEN value
        token: String,
        /// Account ID from token claims, when present
        #[serde(default)]
        account_id: Option<String>,
        /// Agent runtime ID from agent identity JWT claims, when present
        #[serde(default)]
        agent_runtime_id: Option<String>,
        /// Agent private key from agent identity JWT claims, when present
        #[serde(default)]
        agent_private_key: Option<String>,
        /// ChatGPT user ID from agent identity JWT claims, when present
        #[serde(default)]
        chatgpt_user_id: Option<String>,
        /// Whether this account should route through FedRAMP headers
        #[serde(default)]
        chatgpt_account_is_fedramp: bool,
        /// Optional registered task ID for signed Agent Identity requests
        #[serde(default)]
        task_id: Option<String>,
    },
}

#[derive(Debug, Clone, Default)]
pub struct ChatGptIdTokenClaims {
    pub email: Option<String>,
    pub plan_type: Option<String>,
    pub account_id: Option<String>,
    pub subscription_expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default)]
pub struct CodexAccessTokenClaims {
    pub email: Option<String>,
    pub plan_type: Option<String>,
    pub account_id: Option<String>,
    pub agent_runtime_id: Option<String>,
    pub agent_private_key: Option<String>,
    pub chatgpt_user_id: Option<String>,
    pub chatgpt_account_is_fedramp: bool,
}

pub fn parse_codex_access_token_claims(token: &str) -> CodexAccessTokenClaims {
    let Some(json) = parse_jwt_payload(token) else {
        return CodexAccessTokenClaims::default();
    };

    CodexAccessTokenClaims {
        email: json.get("email").and_then(|v| v.as_str()).map(String::from),
        plan_type: json
            .get("plan_type")
            .and_then(|v| v.as_str())
            .map(String::from),
        account_id: json
            .get("account_id")
            .and_then(|v| v.as_str())
            .map(String::from),
        agent_runtime_id: json
            .get("agent_runtime_id")
            .and_then(|v| v.as_str())
            .map(String::from),
        agent_private_key: json
            .get("agent_private_key")
            .and_then(|v| v.as_str())
            .map(String::from),
        chatgpt_user_id: json
            .get("chatgpt_user_id")
            .and_then(|v| v.as_str())
            .map(String::from),
        chatgpt_account_is_fedramp: json
            .get("chatgpt_account_is_fedramp")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    }
}

pub fn parse_chatgpt_id_token_claims(id_token: &str) -> ChatGptIdTokenClaims {
    let Some(json) = parse_jwt_payload(id_token) else {
        return ChatGptIdTokenClaims::default();
    };

    let auth_claims = json.get("https://api.openai.com/auth");

    ChatGptIdTokenClaims {
        email: json.get("email").and_then(|v| v.as_str()).map(String::from),
        plan_type: auth_claims
            .and_then(|auth| auth.get("chatgpt_plan_type"))
            .and_then(|v| v.as_str())
            .map(String::from),
        account_id: auth_claims
            .and_then(|auth| auth.get("chatgpt_account_id"))
            .and_then(|v| v.as_str())
            .map(String::from),
        subscription_expires_at: auth_claims
            .and_then(|auth| auth.get("chatgpt_subscription_active_until"))
            .and_then(|v| v.as_str())
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.with_timezone(&Utc)),
    }
}

fn parse_jwt_payload(token: &str) -> Option<serde_json::Value> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return None;
    }

    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(parts[1])
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(parts[1]))
        .ok()?;

    serde_json::from_slice(&payload).ok()
}

// ============================================================================
// Types for Codex's auth.json format (for compatibility)
// ============================================================================

/// The official Codex auth.json format
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthDotJson {
    /// Explicit auth mode used by newer Codex auth.json formats
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_mode: Option<String>,
    /// OpenAI API key (for API key auth mode)
    #[serde(rename = "OPENAI_API_KEY", skip_serializing_if = "Option::is_none")]
    pub openai_api_key: Option<String>,
    /// OAuth tokens (for ChatGPT auth mode)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens: Option<TokenData>,
    /// Last token refresh timestamp
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_refresh: Option<DateTime<Utc>>,
    /// Agent identity auth material used by CODEX_ACCESS_TOKEN JWTs
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_identity: Option<serde_json::Value>,
    /// Personal access token auth material used by access tokens prefixed with at-
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub personal_access_token: Option<String>,
}

/// Token data stored in auth.json
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenData {
    /// JWT ID token
    pub id_token: String,
    /// Access token
    pub access_token: String,
    /// Refresh token
    pub refresh_token: String,
    /// Account ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
}

// ============================================================================
// Types for frontend communication
// ============================================================================

/// Account info sent to the frontend (without sensitive data)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountInfo {
    pub id: String,
    pub name: String,
    pub email: Option<String>,
    pub plan_type: Option<String>,
    pub subscription_expires_at: Option<DateTime<Utc>>,
    pub auth_mode: AuthMode,
    pub is_active: bool,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

impl AccountInfo {
    pub fn from_stored(account: &StoredAccount, active_id: Option<&str>) -> Self {
        let fallback_subscription_expires_at = match &account.auth_data {
            AuthData::ChatGPT { id_token, .. } => {
                parse_chatgpt_id_token_claims(id_token).subscription_expires_at
            }
            AuthData::ApiKey { .. } | AuthData::CodexAccessToken { .. } => None,
        };

        Self {
            id: account.id.clone(),
            name: account.name.clone(),
            email: account.email.clone(),
            plan_type: account.plan_type.clone(),
            subscription_expires_at: account
                .subscription_expires_at
                .clone()
                .or(fallback_subscription_expires_at),
            auth_mode: account.auth_mode,
            is_active: active_id == Some(&account.id),
            created_at: account.created_at,
            last_used_at: account.last_used_at,
        }
    }
}

/// Usage information for an account
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageInfo {
    /// Account ID
    pub account_id: String,
    /// Plan type
    pub plan_type: Option<String>,
    /// Primary rate limit window usage (percentage 0-100)
    pub primary_used_percent: Option<f64>,
    /// Primary window duration in minutes
    pub primary_window_minutes: Option<i64>,
    /// Primary window reset timestamp (unix seconds)
    pub primary_resets_at: Option<i64>,
    /// Secondary rate limit window usage (percentage 0-100)
    pub secondary_used_percent: Option<f64>,
    /// Secondary window duration in minutes
    pub secondary_window_minutes: Option<i64>,
    /// Secondary window reset timestamp (unix seconds)
    pub secondary_resets_at: Option<i64>,
    /// Whether the account has credits
    pub has_credits: Option<bool>,
    /// Whether credits are unlimited
    pub unlimited_credits: Option<bool>,
    /// Credit balance string (e.g., "$10.50")
    pub credits_balance: Option<String>,
    /// Error message if usage fetch failed
    pub error: Option<String>,
}

impl UsageInfo {
    pub fn error(account_id: String, error: String) -> Self {
        Self {
            account_id,
            plan_type: None,
            primary_used_percent: None,
            primary_window_minutes: None,
            primary_resets_at: None,
            secondary_used_percent: None,
            secondary_window_minutes: None,
            secondary_resets_at: None,
            has_credits: None,
            unlimited_credits: None,
            credits_balance: None,
            error: Some(error),
        }
    }
}

/// Warm-up execution summary across accounts
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WarmupSummary {
    /// Number of accounts that were targeted
    pub total_accounts: usize,
    /// Number of accounts whose warm-up request succeeded
    pub warmed_accounts: usize,
    /// Account IDs whose warm-up request failed
    pub failed_account_ids: Vec<String>,
}

/// Import summary for account config import operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportAccountsSummary {
    /// Number of accounts found in the imported payload.
    pub total_in_payload: usize,
    /// Number of accounts actually imported.
    pub imported_count: usize,
    /// Number of accounts skipped because they already exist.
    pub skipped_count: usize,
}

/// OAuth login information returned to frontend
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthLoginInfo {
    /// The authorization URL to open in browser
    pub auth_url: String,
    /// The local callback port
    pub callback_port: u16,
}

// ============================================================================
// API Response types (from Codex backend)
// ============================================================================

/// Rate limit status from API
#[derive(Debug, Clone, Deserialize)]
pub struct RateLimitStatusPayload {
    pub plan_type: String,
    #[serde(default)]
    pub rate_limit: Option<RateLimitDetails>,
    #[serde(default)]
    pub credits: Option<CreditStatusDetails>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RateLimitDetails {
    #[serde(alias = "primary")]
    pub primary_window: Option<RateLimitWindow>,
    #[serde(alias = "secondary")]
    pub secondary_window: Option<RateLimitWindow>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RateLimitWindow {
    pub used_percent: f64,
    #[serde(default)]
    pub limit_window_seconds: Option<i32>,
    #[serde(default)]
    pub window_duration_mins: Option<i64>,
    #[serde(alias = "resets_at")]
    pub reset_at: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreditStatusDetails {
    pub has_credits: bool,
    pub unlimited: bool,
    #[serde(default)]
    pub balance: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{
        parse_chatgpt_id_token_claims, AppSettings, AuthData, AuthMode, DockDisplayMode,
        StoredAccount, TrayDisplayMode,
    };
    use base64::Engine;

    #[test]
    fn parses_subscription_expiry_from_realistic_id_token_claims() {
        let payload = r#"{"email":"user@example.com","https://api.openai.com/auth":{"chatgpt_plan_type":"plus","chatgpt_account_id":"acc_123","chatgpt_subscription_active_until":"2026-04-23T05:03:38+00:00"}}"#;
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload);
        let token = format!("header.{encoded}.signature");

        let claims = parse_chatgpt_id_token_claims(&token);

        assert_eq!(claims.email.as_deref(), Some("user@example.com"));
        assert_eq!(claims.plan_type.as_deref(), Some("plus"));
        assert_eq!(claims.account_id.as_deref(), Some("acc_123"));
        assert_eq!(
            claims
                .subscription_expires_at
                .map(|value| value.to_rfc3339()),
            Some("2026-04-23T05:03:38+00:00".to_string())
        );
    }

    #[test]
    fn app_settings_default_missing_dock_display_mode_to_show_in_dock() {
        let settings: AppSettings =
            serde_json::from_str(r#"{"tray_display_mode":"active_usage_text"}"#).unwrap();

        assert_eq!(settings.tray_display_mode, TrayDisplayMode::ActiveUsageText);
        assert_eq!(settings.dock_display_mode, DockDisplayMode::ShowInDock);
        assert!(settings.close_behavior_prompt_enabled);
    }

    #[test]
    fn creates_codex_access_token_account_from_jwt_claims() {
        let payload = r#"{"account_id":"acc_env_123","agent_runtime_id":"agent-runtime-123","agent_private_key":"private-key","chatgpt_user_id":"user-123","chatgpt_account_is_fedramp":true,"email":"token-user@example.com","plan_type":"team","exp":2098606399}"#;
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload);
        let token = format!("header.{encoded}.signature");

        let account =
            StoredAccount::new_codex_access_token("Env Account".to_string(), token.clone());

        assert_eq!(account.name, "Env Account");
        assert_eq!(account.email.as_deref(), Some("token-user@example.com"));
        assert_eq!(account.plan_type.as_deref(), Some("team"));
        assert_eq!(account.auth_mode, AuthMode::CodexAccessToken);
        match account.auth_data {
            AuthData::CodexAccessToken {
                token: stored_token,
                account_id,
                agent_runtime_id,
                agent_private_key,
                chatgpt_user_id,
                chatgpt_account_is_fedramp,
                task_id,
            } => {
                assert_eq!(stored_token, token);
                assert_eq!(account_id.as_deref(), Some("acc_env_123"));
                assert_eq!(agent_runtime_id.as_deref(), Some("agent-runtime-123"));
                assert_eq!(agent_private_key.as_deref(), Some("private-key"));
                assert_eq!(chatgpt_user_id.as_deref(), Some("user-123"));
                assert!(chatgpt_account_is_fedramp);
                assert_eq!(task_id, None);
            }
            other => panic!("expected CodexAccessToken auth data, got {other:?}"),
        }
    }

    #[test]
    fn serializes_codex_access_token_auth_mode_as_snake_case() {
        let encoded = serde_json::to_string(&AuthMode::CodexAccessToken).unwrap();

        assert_eq!(encoded, "\"codex_access_token\"");
    }
}
