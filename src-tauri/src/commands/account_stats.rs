//! Account-scoped usage statistics from the Codex profile endpoint.

use chrono::{DateTime, Utc};
use reqwest::{
    header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, AUTHORIZATION, USER_AGENT},
    StatusCode,
};
use serde::{Deserialize, Serialize};

use crate::auth::{ensure_chatgpt_tokens_fresh, load_accounts, refresh_chatgpt_tokens};
use crate::types::{AuthData, AuthMode, StoredAccount};

const CHATGPT_PROFILE_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/profiles/me";
const CHATGPT_RESET_CREDITS_URL: &str =
    "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits";
const CODEX_USER_AGENT: &str = "codex-cli/1.0.0";

#[derive(Debug, Clone, Serialize)]
pub struct AccountUsageStats {
    pub account_id: String,
    pub available: bool,
    pub source: String,
    pub generated_at: Option<String>,
    pub stats_as_of: Option<String>,
    pub summary: AccountUsageSummary,
    pub activity: AccountUsageActivity,
    pub daily: Vec<AccountDailyUsage>,
    pub top_invocations: Vec<AccountTopInvocation>,
    pub reset_credits: Option<AccountResetCredits>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct AccountUsageSummary {
    pub lifetime_tokens: Option<i64>,
    pub peak_daily_tokens: Option<i64>,
    pub longest_task_seconds: Option<i64>,
    pub current_streak_days: Option<i64>,
    pub longest_streak_days: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct AccountUsageActivity {
    pub fast_mode_percent: Option<f64>,
    pub reasoning_effort: Option<String>,
    pub reasoning_effort_percent: Option<f64>,
    pub skills_explored: Option<i64>,
    pub total_skills_used: Option<i64>,
    pub total_threads: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AccountDailyUsage {
    pub date: String,
    pub tokens: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct AccountTopInvocation {
    pub kind: String,
    pub display_name: String,
    pub usage_count: i64,
    pub plugin_id: Option<String>,
    pub plugin_name: Option<String>,
    pub skill_id: Option<String>,
    pub skill_name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AccountResetCredits {
    pub available_count: i64,
    pub next_expires_at: Option<String>,
    pub credits: Vec<AccountResetCredit>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AccountResetCredit {
    pub id: String,
    pub reset_type: String,
    pub status: String,
    pub granted_at: Option<String>,
    pub expires_at: Option<String>,
    pub redeem_started_at: Option<String>,
    pub redeemed_at: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ProfileUsageResponse {
    #[serde(default)]
    stats: ProfileUsageStats,
    #[serde(default)]
    metadata: ProfileUsageMetadata,
}

#[derive(Debug, Default, Deserialize)]
struct ProfileUsageStats {
    #[serde(default)]
    lifetime_tokens: Option<i64>,
    #[serde(default)]
    peak_daily_tokens: Option<i64>,
    #[serde(default)]
    longest_running_turn_sec: Option<i64>,
    #[serde(default)]
    current_streak_days: Option<i64>,
    #[serde(default)]
    longest_streak_days: Option<i64>,
    #[serde(default)]
    daily_usage_buckets: Option<Vec<ProfileDailyUsageBucket>>,
    #[serde(default)]
    top_invocations: Option<Vec<ProfileTopInvocation>>,
    #[serde(default)]
    fast_mode_usage_percentage: Option<f64>,
    #[serde(default)]
    most_used_reasoning_effort: Option<String>,
    #[serde(default)]
    most_used_reasoning_effort_percentage: Option<f64>,
    #[serde(default)]
    unique_skills_used: Option<i64>,
    #[serde(default)]
    total_skills_used: Option<i64>,
    #[serde(default)]
    total_threads: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct ProfileDailyUsageBucket {
    start_date: String,
    tokens: i64,
}

#[derive(Debug, Deserialize)]
struct ProfileTopInvocation {
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    plugin_id: Option<String>,
    #[serde(default)]
    plugin_name: Option<String>,
    #[serde(default)]
    skill_id: Option<String>,
    #[serde(default)]
    skill_name: Option<String>,
    #[serde(default)]
    usage_count: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct ProfileUsageMetadata {
    #[serde(default)]
    stats_as_of: Option<String>,
    #[serde(default)]
    generated_at: Option<String>,
    #[serde(default)]
    stats_error: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ResetCreditsResponse {
    #[serde(default)]
    credits: Vec<ResetCredit>,
    #[serde(default)]
    available_count: i64,
}

#[derive(Debug, Deserialize)]
struct ResetCredit {
    id: String,
    reset_type: String,
    status: String,
    #[serde(default)]
    granted_at: Option<String>,
    #[serde(default)]
    expires_at: Option<String>,
    #[serde(default)]
    redeem_started_at: Option<String>,
    #[serde(default)]
    redeemed_at: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

#[tauri::command]
pub async fn get_account_usage_stats(account_id: String) -> Result<AccountUsageStats, String> {
    let store = load_accounts().map_err(|e| e.to_string())?;
    let account = store
        .accounts
        .iter()
        .find(|account| account.id == account_id)
        .ok_or_else(|| format!("Account not found: {account_id}"))?;

    if account.auth_mode != AuthMode::ChatGPT {
        return Ok(unavailable_stats(
            account_id,
            "Usage stats are available for ChatGPT accounts only.",
        ));
    }

    fetch_profile_usage(account)
        .await
        .map_err(|e| e.to_string())
}

async fn fetch_profile_usage(account: &StoredAccount) -> anyhow::Result<AccountUsageStats> {
    let fresh_account = ensure_chatgpt_tokens_fresh(account).await?;
    let mut response = send_profile_usage_request(&fresh_account).await?;

    if response.status() == StatusCode::UNAUTHORIZED {
        let refreshed_account = refresh_chatgpt_tokens(&fresh_account).await?;
        response = send_profile_usage_request(&refreshed_account).await?;
        return parse_profile_usage_with_reset_credits(&refreshed_account, response).await;
    }

    parse_profile_usage_with_reset_credits(&fresh_account, response).await
}

async fn send_profile_usage_request(account: &StoredAccount) -> anyhow::Result<reqwest::Response> {
    let (access_token, chatgpt_account_id) = extract_chatgpt_auth(account)?;
    let client = reqwest::Client::new();

    Ok(client
        .get(CHATGPT_PROFILE_USAGE_URL)
        .headers(build_chatgpt_headers(access_token, chatgpt_account_id)?)
        .send()
        .await?)
}

async fn parse_profile_usage_with_reset_credits(
    account: &StoredAccount,
    response: reqwest::Response,
) -> anyhow::Result<AccountUsageStats> {
    let mut stats = parse_profile_usage_response(&account.id, response).await?;

    if stats.available {
        stats.reset_credits = fetch_reset_credits(account).await.ok();
    }

    Ok(stats)
}

async fn parse_profile_usage_response(
    account_id: &str,
    response: reqwest::Response,
) -> anyhow::Result<AccountUsageStats> {
    let status = response.status();
    if !status.is_success() {
        return Ok(unavailable_stats(
            account_id.to_string(),
            &format!("Usage stats request failed: {status}"),
        ));
    }

    let payload: ProfileUsageResponse = response.json().await?;
    Ok(map_profile_usage(account_id, payload))
}

async fn fetch_reset_credits(account: &StoredAccount) -> anyhow::Result<AccountResetCredits> {
    let (access_token, chatgpt_account_id) = extract_chatgpt_auth(account)?;
    let client = reqwest::Client::new();
    let response = client
        .get(CHATGPT_RESET_CREDITS_URL)
        .headers(build_reset_credits_headers(
            access_token,
            chatgpt_account_id,
        )?)
        .send()
        .await?;

    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("Reset credits request failed: {status}");
    }

    let payload: ResetCreditsResponse = response.json().await?;
    Ok(map_reset_credits(payload, Utc::now()))
}

fn map_profile_usage(account_id: &str, payload: ProfileUsageResponse) -> AccountUsageStats {
    let stats_error = payload
        .metadata
        .stats_error
        .filter(|value| !value.is_empty());
    let available = stats_error.is_none();
    let stats = payload.stats;

    AccountUsageStats {
        account_id: account_id.to_string(),
        available,
        source: "chatgpt_backend".to_string(),
        generated_at: payload.metadata.generated_at,
        stats_as_of: payload.metadata.stats_as_of,
        summary: AccountUsageSummary {
            lifetime_tokens: stats.lifetime_tokens,
            peak_daily_tokens: stats.peak_daily_tokens,
            longest_task_seconds: stats.longest_running_turn_sec,
            current_streak_days: stats.current_streak_days,
            longest_streak_days: stats.longest_streak_days,
        },
        activity: AccountUsageActivity {
            fast_mode_percent: stats.fast_mode_usage_percentage,
            reasoning_effort: stats.most_used_reasoning_effort,
            reasoning_effort_percent: stats.most_used_reasoning_effort_percentage,
            skills_explored: stats.unique_skills_used,
            total_skills_used: stats.total_skills_used,
            total_threads: stats.total_threads,
        },
        daily: stats
            .daily_usage_buckets
            .unwrap_or_default()
            .into_iter()
            .map(|bucket| AccountDailyUsage {
                date: bucket.start_date,
                tokens: bucket.tokens,
            })
            .collect(),
        top_invocations: stats
            .top_invocations
            .unwrap_or_default()
            .into_iter()
            .map(map_top_invocation)
            .collect(),
        reset_credits: None,
        error: stats_error,
    }
}

fn map_reset_credits(payload: ResetCreditsResponse, now: DateTime<Utc>) -> AccountResetCredits {
    let next_expires_at = payload
        .credits
        .iter()
        .filter(|credit| credit.status == "available")
        .filter_map(|credit| {
            credit
                .expires_at
                .as_ref()
                .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                .map(|value| value.with_timezone(&Utc))
                .filter(|value| *value > now)
                .map(|value| value.to_rfc3339())
        })
        .min();

    AccountResetCredits {
        available_count: payload.available_count.max(0),
        next_expires_at,
        credits: payload.credits.into_iter().map(map_reset_credit).collect(),
    }
}

fn map_reset_credit(credit: ResetCredit) -> AccountResetCredit {
    AccountResetCredit {
        id: credit.id,
        reset_type: credit.reset_type,
        status: credit.status,
        granted_at: credit.granted_at,
        expires_at: credit.expires_at,
        redeem_started_at: credit.redeem_started_at,
        redeemed_at: credit.redeemed_at,
        title: credit.title,
        description: credit.description,
    }
}

fn map_top_invocation(invocation: ProfileTopInvocation) -> AccountTopInvocation {
    let kind = invocation.kind.unwrap_or_else(|| "unknown".to_string());
    let display_name = invocation
        .skill_name
        .as_ref()
        .or(invocation.plugin_name.as_ref())
        .cloned()
        .unwrap_or_else(|| "Unknown".to_string());

    AccountTopInvocation {
        kind,
        display_name,
        usage_count: invocation.usage_count.unwrap_or(0),
        plugin_id: invocation.plugin_id,
        plugin_name: invocation.plugin_name,
        skill_id: invocation.skill_id,
        skill_name: invocation.skill_name,
    }
}

fn unavailable_stats(account_id: String, message: &str) -> AccountUsageStats {
    AccountUsageStats {
        account_id,
        available: false,
        source: "chatgpt_backend".to_string(),
        generated_at: None,
        stats_as_of: None,
        summary: AccountUsageSummary::default(),
        activity: AccountUsageActivity::default(),
        daily: Vec::new(),
        top_invocations: Vec::new(),
        reset_credits: None,
        error: Some(message.to_string()),
    }
}

fn build_chatgpt_headers(
    access_token: &str,
    chatgpt_account_id: Option<&str>,
) -> anyhow::Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static(CODEX_USER_AGENT));
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {access_token}"))?,
    );

    if let Some(account_id) = chatgpt_account_id {
        headers.insert(
            HeaderName::from_static("chatgpt-account-id"),
            HeaderValue::from_str(account_id)?,
        );
    }

    Ok(headers)
}

fn build_reset_credits_headers(
    access_token: &str,
    chatgpt_account_id: Option<&str>,
) -> anyhow::Result<HeaderMap> {
    let mut headers = build_chatgpt_headers(access_token, chatgpt_account_id)?;
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
    headers.insert(
        HeaderName::from_static("openai-beta"),
        HeaderValue::from_static("codex-1"),
    );
    headers.insert(
        HeaderName::from_static("originator"),
        HeaderValue::from_static("Codex Desktop"),
    );
    Ok(headers)
}

fn extract_chatgpt_auth(account: &StoredAccount) -> anyhow::Result<(&str, Option<&str>)> {
    match &account.auth_data {
        AuthData::ChatGPT {
            access_token,
            account_id,
            ..
        } => Ok((access_token.as_str(), account_id.as_deref())),
        AuthData::ApiKey { .. } => anyhow::bail!("Account is not using ChatGPT OAuth"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_usage_response_maps_profile_stats() {
        let payload: ProfileUsageResponse = serde_json::from_value(serde_json::json!({
            "stats": {
                "lifetime_tokens": 1549926883,
                "peak_daily_tokens": 135100741,
                "longest_running_turn_sec": 10797,
                "current_streak_days": 3,
                "longest_streak_days": 10,
                "daily_usage_buckets": [
                    { "start_date": "2026-06-25", "tokens": 44880283 },
                    { "start_date": "2026-06-26", "tokens": 29 }
                ],
                "top_invocations": [
                    {
                        "type": "skill",
                        "skill_name": "dilekcebot-template-builder",
                        "usage_count": 95
                    },
                    {
                        "type": "plugin",
                        "plugin_name": "github",
                        "usage_count": 7
                    }
                ],
                "fast_mode_usage_percentage": 1.09,
                "most_used_reasoning_effort": "medium",
                "most_used_reasoning_effort_percentage": 51.12,
                "unique_skills_used": 12,
                "total_skills_used": 191,
                "total_threads": 500
            },
            "metadata": {
                "stats_as_of": "2026-06-26",
                "generated_at": "2026-06-26T14:56:19.430889Z",
                "stats_error": null
            }
        }))
        .unwrap();

        let stats = map_profile_usage("account-1", payload);

        assert!(stats.available);
        assert_eq!(stats.account_id, "account-1");
        assert_eq!(stats.summary.lifetime_tokens, Some(1_549_926_883));
        assert_eq!(stats.summary.peak_daily_tokens, Some(135_100_741));
        assert_eq!(stats.summary.longest_task_seconds, Some(10_797));
        assert_eq!(stats.summary.current_streak_days, Some(3));
        assert_eq!(stats.daily.len(), 2);
        assert_eq!(stats.daily[1].tokens, 29);
        assert_eq!(
            stats.top_invocations[0].display_name,
            "dilekcebot-template-builder"
        );
        assert_eq!(stats.top_invocations[1].display_name, "github");
        assert_eq!(stats.activity.total_threads, Some(500));
    }

    #[test]
    fn reset_credits_response_maps_available_count_and_next_expiry() {
        let payload: ResetCreditsResponse = serde_json::from_value(serde_json::json!({
            "credits": [
                {
                    "id": "expired-available",
                    "reset_type": "codex_rate_limits",
                    "status": "available",
                    "granted_at": "2026-05-18T00:39:53Z",
                    "expires_at": "2026-06-17T00:39:53Z"
                },
                {
                    "id": "later",
                    "reset_type": "codex_rate_limits",
                    "status": "available",
                    "granted_at": "2026-06-18T00:39:53.731630Z",
                    "expires_at": "2026-07-18T00:39:53.731630Z"
                },
                {
                    "id": "earlier",
                    "reset_type": "codex_rate_limits",
                    "status": "available",
                    "granted_at": "2026-06-12T04:03:43.263391Z",
                    "expires_at": "2026-07-12T04:03:43.263391Z",
                    "title": "One free rate limit reset"
                },
                {
                    "id": "redeemed",
                    "reset_type": "codex_rate_limits",
                    "status": "redeemed",
                    "granted_at": "2026-06-12T04:03:43Z",
                    "expires_at": "2026-07-10T04:03:43Z"
                }
            ],
            "available_count": 2
        }))
        .unwrap();

        let now = DateTime::parse_from_rfc3339("2026-06-26T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let stats = map_reset_credits(payload, now);

        assert_eq!(stats.available_count, 2);
        assert_eq!(stats.credits.len(), 4);
        assert_eq!(
            stats.next_expires_at.as_deref(),
            Some("2026-07-12T04:03:43.263391+00:00")
        );
        assert_eq!(
            stats.credits[2].title.as_deref(),
            Some("One free rate limit reset")
        );
    }
}
