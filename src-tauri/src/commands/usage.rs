//! Usage query Tauri commands

use crate::api::usage::{fetch_chatgpt_account_metadata, get_account_usage, refresh_all_usage};
use crate::auth::{
    ensure_chatgpt_tokens_fresh, get_account, load_accounts, update_account_metadata,
};
use crate::types::{AccountInfo, AuthData, UsageInfo, WarmupSummary};

/// Fetch usage info for a specific account (shared by the Tauri command and web mode).
pub async fn fetch_usage(account_id: &str) -> Result<UsageInfo, String> {
    let account = get_account(account_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Account not found: {account_id}"))?;

    get_account_usage(&account).await.map_err(|e| e.to_string())
}

/// Get usage info for a specific account
#[tauri::command]
pub async fn get_usage(app: tauri::AppHandle, account_id: String) -> Result<UsageInfo, String> {
    let usage = fetch_usage(&account_id).await?;

    // Keep the tray menu/title in sync with whichever UI fetched fresh usage.
    #[cfg(desktop)]
    crate::tray::ingest_usage(&app, vec![usage.clone()]);
    #[cfg(not(desktop))]
    let _ = app;

    Ok(usage)
}

/// Refresh account metadata for a specific account.
/// For ChatGPT accounts this ensures OAuth tokens are valid and pulls live subscription metadata.
/// For API key accounts this is a no-op.
#[tauri::command]
pub async fn refresh_account_metadata(account_id: String) -> Result<AccountInfo, String> {
    let account = get_account(&account_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Account not found: {account_id}"))?;

    let updated = match &account.auth_data {
        AuthData::ApiKey { .. } => account,
        AuthData::ChatGPT { .. } => {
            let refreshed = ensure_chatgpt_tokens_fresh(&account)
                .await
                .map_err(|e| e.to_string())?;
            let live_metadata = fetch_chatgpt_account_metadata(&refreshed)
                .await
                .map_err(|e| e.to_string())?;

            update_account_metadata(
                &account_id,
                None,
                None,
                live_metadata.plan_type,
                Some(live_metadata.subscription_expires_at),
            )
            .map_err(|e| e.to_string())?
        }
    };

    let store = load_accounts().map_err(|e| e.to_string())?;
    let active_id = store.active_account_id.as_deref();
    Ok(AccountInfo::from_stored(&updated, active_id))
}

/// Refresh usage info for all accounts
#[tauri::command]
pub async fn refresh_all_accounts_usage() -> Result<Vec<UsageInfo>, String> {
    let store = load_accounts().map_err(|e| e.to_string())?;
    Ok(refresh_all_usage(&store.accounts).await)
}

/// Send a minimal warm-up request for one account
#[tauri::command]
pub async fn warmup_account(account_id: String) -> Result<(), String> {
    let account = get_account(&account_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Account not found: {account_id}"))?;

    crate::warmup_scheduler::run_manual_account(&account)
        .await
        .map_err(|e| e.to_string())
}

/// Send minimal warm-up requests for all accounts
#[tauri::command]
pub async fn warmup_all_accounts() -> Result<WarmupSummary, String> {
    let store = load_accounts().map_err(|e| e.to_string())?;
    crate::warmup_scheduler::run_manual_all(store.accounts)
        .await
        .map_err(|e| e.to_string())
}
