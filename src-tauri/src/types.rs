//! Core types for Codex Switcher

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

fn default_app_settings_version() -> u32 {
    1
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportSecurityMode {
    LessSecure,
    Passphrase,
    Keychain,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ScheduledWarmupSettings {
    pub enabled: bool,
    pub local_time: String,
    pub account_ids: Vec<String>,
    pub last_run_local_date: Option<String>,
    pub last_missed_prompt_local_date: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    #[serde(default = "default_app_settings_version")]
    pub version: u32,
    #[serde(default)]
    pub export_security_mode: Option<ExportSecurityMode>,
    #[serde(default)]
    pub scheduled_warmup: Option<ScheduledWarmupSettings>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            version: 1,
            export_security_mode: None,
            scheduled_warmup: None,
        }
    }
}

/// The main storage structure for all accounts
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountsStore {
    /// Schema version for future migrations
    pub version: u32,
    /// List of all stored accounts
    pub accounts: Vec<StoredAccount>,
    /// Currently active account ID
    pub active_account_id: Option<String>,
}

impl Default for AccountsStore {
    fn default() -> Self {
        Self {
            version: 1,
            accounts: Vec::new(),
            active_account_id: None,
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
}

/// Authentication mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    /// Using an OpenAI API key
    ApiKey,
    /// Using ChatGPT OAuth tokens
    ChatGPT,
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
}

// ============================================================================
// Types for Codex's auth.json format (for compatibility)
// ============================================================================

/// The official Codex auth.json format
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthDotJson {
    /// OpenAI API key (for API key auth mode)
    #[serde(rename = "OPENAI_API_KEY", skip_serializing_if = "Option::is_none")]
    pub openai_api_key: Option<String>,
    /// OAuth tokens (for ChatGPT auth mode)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens: Option<TokenData>,
    /// Last token refresh timestamp
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_refresh: Option<DateTime<Utc>>,
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
    pub auth_mode: AuthMode,
    pub is_active: bool,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

impl AccountInfo {
    pub fn from_stored(account: &StoredAccount, active_id: Option<&str>) -> Self {
        Self {
            id: account.id.clone(),
            name: account.name.clone(),
            email: account.email.clone(),
            plan_type: account.plan_type.clone(),
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledWarmupStatus {
    pub schedule: Option<ScheduledWarmupSettings>,
    pub valid_account_ids: Vec<String>,
    pub missed_run_today: bool,
    pub next_run_local_iso: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledWarmupEvent {
    pub summary: WarmupSummary,
    pub trigger: String,
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
    pub primary_window: Option<RateLimitWindow>,
    pub secondary_window: Option<RateLimitWindow>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RateLimitWindow {
    pub used_percent: f64,
    pub limit_window_seconds: Option<i32>,
    pub reset_at: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreditStatusDetails {
    pub has_credits: bool,
    pub unlimited: bool,
    #[serde(default)]
    pub balance: Option<String>,
}
