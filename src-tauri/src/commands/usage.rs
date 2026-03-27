//! Usage query Tauri commands

use crate::api::usage::{get_account_usage, refresh_all_usage, warmup_account as send_warmup};
use crate::auth::{get_account, load_accounts, update_account_cached_usage};
use crate::types::{UsageInfo, WarmupSummary};
use chrono::Utc;
use futures::{stream, StreamExt};

fn persist_usage_snapshot(usage: &UsageInfo) {
    if usage.error.is_some() {
        return;
    }

    if let Err(error) =
        update_account_cached_usage(&usage.account_id, usage.clone(), Utc::now())
    {
        eprintln!(
            "[Usage] Failed to persist usage snapshot for {}: {}",
            usage.account_id, error
        );
    }
}

/// Get usage info for a specific account
#[tauri::command]
pub async fn get_usage(account_id: String) -> Result<UsageInfo, String> {
    let account = get_account(&account_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Account not found: {account_id}"))?;

    let usage = get_account_usage(&account).await.map_err(|e| e.to_string())?;
    persist_usage_snapshot(&usage);
    Ok(usage)
}

/// Refresh usage info for all accounts
#[tauri::command]
pub async fn refresh_all_accounts_usage() -> Result<Vec<UsageInfo>, String> {
    let store = load_accounts().map_err(|e| e.to_string())?;
    let usage_list = refresh_all_usage(&store.accounts).await;

    for usage in &usage_list {
        persist_usage_snapshot(usage);
    }

    Ok(usage_list)
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
    let total_accounts = store.accounts.len();
    let concurrency = total_accounts.min(10).max(1);

    let results: Vec<(String, bool)> = stream::iter(store.accounts.into_iter())
        .map(|account| async move {
            let account_id = account.id.clone();
            let failed = send_warmup(&account).await.is_err();
            (account_id, failed)
        })
        .buffer_unordered(concurrency)
        .collect()
        .await;

    let failed_account_ids = results
        .into_iter()
        .filter_map(|(account_id, failed)| failed.then_some(account_id))
        .collect::<Vec<_>>();

    let warmed_accounts = total_accounts.saturating_sub(failed_account_ids.len());
    Ok(WarmupSummary {
        total_accounts,
        warmed_accounts,
        failed_account_ids,
    })
}
