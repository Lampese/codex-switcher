//! Read-only integration with the signed-in Cursor desktop app.
//!
//! Cursor owns login and credential persistence. We only read its local
//! session database with a read-only SQLite connection and use the resulting
//! short-lived session to query Cursor's own usage endpoints.

use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use base64::Engine;
use chrono::{DateTime, Utc};
use reqwest::StatusCode;
use rusqlite::{OpenFlags, OptionalExtension};
use serde::Deserialize;

use crate::types::{CursorAccountInfo, UsageInfo};

const CURSOR_ACCOUNT_ID: &str = "cursor:desktop";
const CURSOR_BASE_URL: &str = "https://cursor.com";
static CURSOR_LOGIN_GENERATION: AtomicU64 = AtomicU64::new(0);

#[tauri::command]
pub async fn start_cursor_login() -> Result<(), String> {
    launch_cursor().map_err(|error| error.to_string())?;
    CURSOR_LOGIN_GENERATION.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

/// Wait for Cursor's official login to produce a usable local session.
#[tauri::command]
pub async fn complete_cursor_login() -> Result<CursorAccountInfo, String> {
    let generation = CURSOR_LOGIN_GENERATION.load(Ordering::Relaxed);
    if generation == 0 {
        return Err("No pending Cursor login".into());
    }

    for _ in 0..120 {
        if CURSOR_LOGIN_GENERATION.load(Ordering::Relaxed) != generation {
            return Err("Cursor login was cancelled".into());
        }
        if let Ok(account) = fetch_cursor_account().await {
            return Ok(account);
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    Err("Cursor login was not detected. Finish signing in from Cursor and try again.".into())
}

/// Cancel a pending Cursor login without changing Cursor's own session.
#[tauri::command]
pub async fn cancel_cursor_login() -> Result<(), String> {
    CURSOR_LOGIN_GENERATION.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

#[tauri::command]
pub async fn cursor_account() -> Result<Option<CursorAccountInfo>, String> {
    match fetch_cursor_account().await {
        Ok(account) => Ok(Some(account)),
        Err(CursorError::NotSignedIn) => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

#[tauri::command]
pub async fn cursor_usage() -> Result<UsageInfo, String> {
    fetch_cursor_usage()
        .await
        .map_err(|error| error.to_string())
}

#[derive(Debug, thiserror::Error)]
enum CursorError {
    #[error("Cursor is not signed in")]
    NotSignedIn,
    #[error("Cursor session could not be read: {0}")]
    Session(String),
    #[error("Cursor usage request failed: {0}")]
    Request(String),
}

fn state_db_in(data_dir: PathBuf) -> PathBuf {
    data_dir
        .join("User")
        .join("globalStorage")
        .join("state.vscdb")
}

fn cursor_state_db_path() -> Option<PathBuf> {
    if let Some(data_dir) = std::env::var_os("CODEX_SWITCHER_CURSOR_DATA_DIR") {
        return Some(state_db_in(data_dir.into()));
    }

    let standard = state_db_in(dirs::config_dir()?.join("Cursor"));
    if standard.is_file() {
        return Some(standard);
    }

    // Keep redirected Cursor profiles usable without adding another settings
    // surface. This supports the common `cursor-data/Roaming-Cursor` migration
    // layout and remains read-only; the standard directory always wins.
    #[cfg(target_os = "windows")]
    for letter in b'C'..=b'Z' {
        let migrated = state_db_in(PathBuf::from(format!(
            "{}:\\cursor-data\\Roaming-Cursor",
            letter as char
        )));
        if migrated.is_file() {
            return Some(migrated);
        }
    }

    Some(standard)
}

fn load_cursor_access_token() -> Result<String, CursorError> {
    let path = cursor_state_db_path().ok_or(CursorError::NotSignedIn)?;
    if !path.exists() {
        return Err(CursorError::NotSignedIn);
    }

    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let connection = rusqlite::Connection::open_with_flags(&path, flags)
        .map_err(|error| CursorError::Session(error.to_string()))?;
    connection
        .busy_timeout(Duration::from_millis(250))
        .map_err(|error| CursorError::Session(error.to_string()))?;
    let value: Option<String> = connection
        .query_row(
            "SELECT value FROM ItemTable WHERE key = ?1 LIMIT 1",
            ["cursorAuth/accessToken"],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| CursorError::Session(error.to_string()))?;
    value
        .filter(|value| !value.trim().is_empty())
        .ok_or(CursorError::NotSignedIn)
}

fn cursor_cookie(access_token: &str) -> String {
    let subject = jwt_subject(access_token).unwrap_or_default();
    format!(
        "WorkosCursorSessionToken={subject}%3A%3A{}",
        access_token.trim()
    )
}

fn jwt_subject(token: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    let json: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    json.get("sub")?.as_str().map(str::to_string)
}

fn cursor_client() -> Result<reqwest::Client, CursorError> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|error| CursorError::Request(error.to_string()))
}

async fn cursor_get(path: &str) -> Result<reqwest::Response, CursorError> {
    let cookie = cursor_cookie(&load_cursor_access_token()?);
    let response = cursor_client()?
        .get(format!("{CURSOR_BASE_URL}{path}"))
        .header("Cookie", cookie)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|error| CursorError::Request(error.to_string()))?;
    if matches!(
        response.status(),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
    ) {
        return Err(CursorError::NotSignedIn);
    }
    if !response.status().is_success() {
        return Err(CursorError::Request(format!("HTTP {}", response.status())));
    }
    Ok(response)
}

async fn fetch_cursor_account() -> Result<CursorAccountInfo, CursorError> {
    let (me, usage) = tokio::try_join!(
        async {
            cursor_get("/api/auth/me")
                .await?
                .json::<CursorUser>()
                .await
                .map_err(|error| CursorError::Request(error.to_string()))
        },
        fetch_usage_summary()
    )?;
    let plan_type = usage.membership_type.as_deref().map(format_plan);
    let name = me
        .email
        .clone()
        .or(me.name)
        .unwrap_or_else(|| "Cursor".into());
    Ok(CursorAccountInfo {
        id: CURSOR_ACCOUNT_ID.into(),
        name,
        email: me.email,
        plan_type,
        is_connected: true,
    })
}

async fn fetch_usage_summary() -> Result<CursorUsageSummary, CursorError> {
    cursor_get("/api/usage-summary")
        .await?
        .json::<CursorUsageSummary>()
        .await
        .map_err(|error| CursorError::Request(error.to_string()))
}

async fn fetch_cursor_usage() -> Result<UsageInfo, CursorError> {
    let summary = fetch_usage_summary().await?;
    Ok(cursor_usage_info(&summary))
}

fn cursor_usage_info(summary: &CursorUsageSummary) -> UsageInfo {
    let reset_at = summary
        .billing_cycle_end
        .as_deref()
        .and_then(parse_timestamp);
    let plan = summary
        .individual_usage
        .as_ref()
        .and_then(|usage| usage.plan.as_ref());
    // Cursor exposes two independent model pools. `apiPercentUsed` backs the
    // dashboard's "Other Models" meter; `totalPercentUsed` is an overall
    // spend meter and must not be projected as third-party model usage.
    let primary = plan.and_then(|plan| plan.api_percent_used.map(clamp_percent));
    let secondary = plan.and_then(|plan| plan.auto_percent_used.map(clamp_percent));
    UsageInfo {
        account_id: CURSOR_ACCOUNT_ID.into(),
        plan_type: summary.membership_type.as_deref().map(format_plan),
        primary_used_percent: primary,
        primary_window_minutes: None,
        primary_resets_at: reset_at,
        secondary_used_percent: secondary,
        secondary_window_minutes: None,
        secondary_resets_at: reset_at,
        has_credits: None,
        unlimited_credits: summary.is_unlimited,
        credits_balance: None,
        error: None,
    }
}

fn clamp_percent(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(0.0, 100.0)
    } else {
        0.0
    }
}

fn parse_timestamp(value: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&Utc).timestamp())
}

fn format_plan(value: &str) -> String {
    format!(
        "Cursor {}",
        value
            .chars()
            .next()
            .map(char::to_uppercase)
            .into_iter()
            .flatten()
            .chain(value.chars().skip(1))
            .collect::<String>()
    )
}

fn launch_cursor() -> anyhow::Result<()> {
    #[cfg(target_os = "windows")]
    {
        let local = std::env::var_os("LOCALAPPDATA")
            .ok_or_else(|| anyhow::anyhow!("LOCALAPPDATA is unavailable"))?;
        let executable = PathBuf::from(local)
            .join("Programs")
            .join("cursor")
            .join("Cursor.exe");
        if !executable.exists() {
            anyhow::bail!("Cursor is not installed in the current Windows account");
        }
        std::process::Command::new(executable).spawn()?;
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .args(["-a", "Cursor"])
            .spawn()?;
        Ok(())
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("cursor").spawn()?;
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CursorUser {
    email: Option<String>,
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CursorUsageSummary {
    billing_cycle_end: Option<String>,
    membership_type: Option<String>,
    is_unlimited: Option<bool>,
    individual_usage: Option<CursorIndividualUsage>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CursorIndividualUsage {
    plan: Option<CursorPlanUsage>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CursorPlanUsage {
    auto_percent_used: Option<f64>,
    api_percent_used: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_uses_jwt_subject_without_exposing_other_claims() {
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(br#"{"sub":"user_123","email":"private@example.com"}"#);
        let token = format!("header.{payload}.signature");
        assert_eq!(
            cursor_cookie(&token),
            format!("WorkosCursorSessionToken=user_123%3A%3A{token}")
        );
    }

    #[test]
    fn cursor_usage_maps_api_to_third_party_and_auto_to_first_party() {
        let summary = CursorUsageSummary {
            billing_cycle_end: Some("2026-09-29T00:00:00Z".into()),
            membership_type: Some("pro".into()),
            is_unlimited: Some(false),
            individual_usage: Some(CursorIndividualUsage {
                plan: Some(CursorPlanUsage {
                    auto_percent_used: Some(13.0),
                    api_percent_used: Some(81.0),
                }),
            }),
        };

        let usage = cursor_usage_info(&summary);

        assert_eq!(usage.primary_used_percent, Some(81.0));
        assert_eq!(usage.secondary_used_percent, Some(13.0));
        assert_eq!(usage.primary_resets_at, usage.secondary_resets_at);
    }
}
