//! Account storage module - manages reading and writing accounts.json

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};

use super::{default_client, initialize_default_store, AccountStoreRuntimeGuard};
use crate::types::{AccountsStore, AppSettings, AuthData, StoredAccount};

/// Get the path to the codex-switcher config directory
pub fn get_config_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not find home directory")?;
    Ok(home.join(".codex-switcher"))
}

/// Get the path to accounts.json
pub fn get_accounts_file() -> Result<PathBuf> {
    Ok(get_config_dir()?.join("accounts.json"))
}

pub fn get_settings_file() -> Result<PathBuf> {
    Ok(get_config_dir()?.join("settings.json"))
}

/// Start the sole account-store writer before serving UI or web requests.
pub(crate) fn initialize_accounts() -> Result<AccountStoreRuntimeGuard> {
    initialize_default_store(get_accounts_file()?)
}

/// Return the worker-published in-memory snapshot without disk I/O.
pub fn load_accounts() -> Result<AccountsStore> {
    default_client()?.snapshot()
}

pub fn load_app_settings() -> Result<AppSettings> {
    let path = get_settings_file()?;

    if !path.exists() {
        return Ok(AppSettings::default());
    }

    let content = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read settings file: {}", path.display()))?;

    let settings: AppSettings = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse settings file: {}", path.display()))?;

    Ok(settings)
}

pub fn save_app_settings(settings: &AppSettings) -> Result<()> {
    let path = get_settings_file()?;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create config directory: {}", parent.display()))?;
    }

    let content = serde_json::to_string_pretty(settings).context("Failed to serialize settings")?;
    fs::write(&path, content)
        .with_context(|| format!("Failed to write settings file: {}", path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o600);
        fs::set_permissions(&path, perms)?;
    }

    Ok(())
}

/// Apply one serialized mutation on the account-store worker.
pub(crate) async fn update_accounts<Output, Update>(update: Update) -> Result<Output>
where
    Output: Send + 'static,
    Update: FnOnce(&mut AccountsStore) -> Result<Output> + Send + 'static,
{
    default_client()?.update(update).await
}

/// Add a new account to the store
pub async fn add_account(account: StoredAccount) -> Result<StoredAccount> {
    update_accounts(move |store| {
        if store.accounts.iter().any(|a| a.name == account.name) {
            anyhow::bail!("An account with name '{}' already exists", account.name);
        }
        let account_clone = account.clone();
        store.accounts.push(account);
        if store.accounts.len() == 1 {
            store.active_account_id = Some(account_clone.id.clone());
        }
        Ok(account_clone)
    })
    .await
}

/// Remove an account by ID
pub async fn remove_account(account_id: &str) -> Result<()> {
    let account_id = account_id.to_string();
    update_accounts(move |store| {
        let initial_len = store.accounts.len();
        store.accounts.retain(|a| a.id != account_id);
        if store.accounts.len() == initial_len {
            anyhow::bail!("Account not found: {account_id}");
        }
        if store.active_account_id.as_deref() == Some(account_id.as_str()) {
            store.active_account_id = store.accounts.first().map(|a| a.id.clone());
        }
        Ok(())
    })
    .await
}

/// Activate an account and record its use in one durable update.
pub async fn set_active_account(account_id: &str) -> Result<()> {
    let account_id = account_id.to_string();
    update_accounts(move |store| {
        let account = store
            .accounts
            .iter_mut()
            .find(|account| account.id == account_id)
            .with_context(|| format!("Account not found: {account_id}"))?;
        super::switch_to_account(account)?;
        account.last_used_at = Some(Utc::now());
        store.active_account_id = Some(account_id);
        Ok(())
    })
    .await
}

/// Get an account by ID
pub fn get_account(account_id: &str) -> Result<Option<StoredAccount>> {
    let store = load_accounts()?;
    Ok(store.accounts.into_iter().find(|a| a.id == account_id))
}

/// Get the currently active account
pub fn get_active_account() -> Result<Option<StoredAccount>> {
    let store = load_accounts()?;
    let active_id = match &store.active_account_id {
        Some(id) => id,
        None => return Ok(None),
    };
    Ok(store.accounts.into_iter().find(|a| a.id == *active_id))
}

/// Update an account's metadata (name, email, plan_type, subscription expiry)
pub async fn update_account_metadata(
    account_id: &str,
    name: Option<String>,
    email: Option<String>,
    plan_type: Option<String>,
    subscription_expires_at: Option<Option<DateTime<Utc>>>,
) -> Result<StoredAccount> {
    let account_id = account_id.to_string();
    update_accounts(move |store| {
        if let Some(ref new_name) = name {
            if store
                .accounts
                .iter()
                .any(|a| a.id != account_id && a.name == *new_name)
            {
                anyhow::bail!("An account with name '{new_name}' already exists");
            }
        }
        let account = store
            .accounts
            .iter_mut()
            .find(|a| a.id == account_id)
            .context("Account not found")?;
        if let Some(new_name) = name {
            account.name = new_name;
        }
        if email.is_some() {
            account.email = email;
        }
        if plan_type.is_some() {
            account.plan_type = plan_type;
        }
        if let Some(subscription_expires_at) = subscription_expires_at {
            account.subscription_expires_at = subscription_expires_at;
        }
        Ok(account.clone())
    })
    .await
}

/// Update ChatGPT OAuth tokens for an account and return the updated account.
pub async fn update_account_chatgpt_tokens(
    account_id: &str,
    id_token: String,
    access_token: String,
    refresh_token: String,
    chatgpt_account_id: Option<String>,
    email: Option<String>,
    plan_type: Option<String>,
    subscription_expires_at: Option<DateTime<Utc>>,
) -> Result<StoredAccount> {
    let account_id = account_id.to_string();
    update_accounts(move |store| {
        let is_active = store.active_account_id.as_deref() == Some(account_id.as_str());
        let account = store
            .accounts
            .iter_mut()
            .find(|a| a.id == account_id)
            .context("Account not found")?;
        match &mut account.auth_data {
            AuthData::ChatGPT {
                id_token: stored_id_token,
                access_token: stored_access_token,
                refresh_token: stored_refresh_token,
                account_id: stored_account_id,
            } => {
                *stored_id_token = id_token;
                *stored_access_token = access_token;
                *stored_refresh_token = refresh_token;
                if let Some(new_account_id) = chatgpt_account_id {
                    *stored_account_id = Some(new_account_id);
                }
            }
            AuthData::ApiKey { .. } => {
                anyhow::bail!("Cannot update OAuth tokens for an API key account");
            }
        }
        if let Some(new_email) = email {
            account.email = Some(new_email);
        }
        if let Some(new_plan_type) = plan_type {
            account.plan_type = Some(new_plan_type);
        }
        if let Some(subscription_expires_at) = subscription_expires_at {
            account.subscription_expires_at = Some(subscription_expires_at);
        }
        if is_active {
            super::switch_to_account(account)?;
        }
        Ok(account.clone())
    })
    .await
}

/// Get the list of masked account IDs
pub fn get_masked_account_ids() -> Result<Vec<String>> {
    let store = load_accounts()?;
    Ok(store.masked_account_ids.clone())
}

/// Set the list of masked account IDs
pub async fn set_masked_account_ids(ids: Vec<String>) -> Result<()> {
    update_accounts(move |store| {
        store.masked_account_ids = ids;
        Ok(())
    })
    .await
}
