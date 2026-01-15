//! Usage query Tauri commands

use crate::api::usage::{get_account_usage, refresh_all_usage};
use crate::auth::{get_account, load_accounts};
use crate::types::UsageInfo;

/// Get usage info for a specific account
#[tauri::command]
pub async fn get_usage(account_id: String) -> Result<UsageInfo, String> {
    let account = get_account(&account_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Account not found: {account_id}"))?;

    get_account_usage(&account).await.map_err(|e| e.to_string())
}

/// Refresh usage info for all accounts
#[tauri::command]
pub async fn refresh_all_accounts_usage() -> Result<Vec<UsageInfo>, String> {
    let store = load_accounts().map_err(|e| e.to_string())?;
    Ok(refresh_all_usage(&store.accounts).await)
}
