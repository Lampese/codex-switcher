//! Usage query Tauri commands

use crate::api::usage::{
    fetch_chatgpt_account_metadata, get_account_usage, refresh_all_usage,
    warmup_account as send_warmup,
};
use crate::auth::{get_account, load_accounts, refresh_chatgpt_tokens, update_account_metadata};
use crate::types::{AccountInfo, AuthData, UsageInfo, WarmupSummary};
use futures::{stream, StreamExt};

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

/// Force-refresh account metadata for a specific account.
/// For ChatGPT accounts this refreshes OAuth tokens and pulls live subscription metadata.
/// For API key accounts this is a no-op.
#[tauri::command]
pub async fn refresh_account_metadata(account_id: String) -> Result<AccountInfo, String> {
    let account = get_account(&account_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Account not found: {account_id}"))?;

    let updated = match &account.auth_data {
        AuthData::ApiKey { .. } => account,
        AuthData::ChatGPT { .. } => {
            let refreshed = refresh_chatgpt_tokens(&account)
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

    send_warmup(&account).await.map_err(|e| e.to_string())
}

/// Send minimal warm-up requests for all accounts
#[tauri::command]
pub async fn warmup_all_accounts() -> Result<WarmupSummary, String> {
    let store = load_accounts().map_err(|e| e.to_string())?;
    let total_accounts = store.accounts.len();
    let concurrency = total_accounts.min(10).max(1);

    // Collect (account_id, account_name, Option<error>)
    let results: Vec<(String, String, Option<String>)> = stream::iter(store.accounts.into_iter())
        .map(|account| async move {
            let id = account.id.clone();
            let name = account.name.clone();
            match send_warmup(&account).await {
                Ok(()) => (id, name, None),
                Err(e) => {
                    // Truncate long error messages for the toast
                    let msg = e.to_string();
                    let short = if msg.len() > 120 {
                        format!("{}…", &msg[..120])
                    } else {
                        msg
                    };
                    (id, name, Some(short))
                }
            }
        })
        .buffer_unordered(concurrency)
        .collect()
        .await;

    let mut failed_account_ids = Vec::new();
    let mut failed_account_errors = Vec::new();

    for (id, name, err) in results {
        if let Some(e) = err {
            failed_account_ids.push(id);
            failed_account_errors.push((name, e));
        }
    }

    let warmed_accounts = total_accounts.saturating_sub(failed_account_ids.len());
    Ok(WarmupSummary {
        total_accounts,
        warmed_accounts,
        failed_account_ids,
        failed_account_errors,
    })
}
