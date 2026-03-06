//! Usage query Tauri commands

use crate::api::usage::{get_account_usage, refresh_all_usage, warmup_account as send_warmup};
use crate::auth::{get_account, load_accounts};
use crate::types::{UsageInfo, WarmupSummary};

pub async fn warmup_accounts_by_ids(account_ids: &[String]) -> Result<WarmupSummary, String> {
    let store = load_accounts().map_err(|e| e.to_string())?;
    let selected_accounts: Vec<_> = store
        .accounts
        .iter()
        .filter(|account| {
            account_ids
                .iter()
                .any(|account_id| account_id == &account.id)
        })
        .cloned()
        .collect();

    let total_accounts = selected_accounts.len();
    let mut failed_account_ids = Vec::new();

    for account in &selected_accounts {
        if send_warmup(account).await.is_err() {
            failed_account_ids.push(account.id.clone());
        }
    }

    let warmed_accounts = total_accounts.saturating_sub(failed_account_ids.len());
    Ok(WarmupSummary {
        total_accounts,
        warmed_accounts,
        failed_account_ids,
    })
}

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

/// Send a minimal warm-up request for one account
#[tauri::command]
pub async fn warmup_account(account_id: String) -> Result<(), String> {
    let account = get_account(&account_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Account not found: {account_id}"))?;

    send_warmup(&account).await.map_err(|e| e.to_string())
}

/// Send minimal warm-up requests for all accounts
#[tauri::command]
pub async fn warmup_all_accounts() -> Result<WarmupSummary, String> {
    let store = load_accounts().map_err(|e| e.to_string())?;
    let account_ids: Vec<String> = store
        .accounts
        .into_iter()
        .map(|account| account.id)
        .collect();
    warmup_accounts_by_ids(&account_ids).await
}
