//! Account storage module - manages reading and writing accounts.json

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::types::{AccountsStore, AuthData, StoredAccount, UsageAutomationSettings};

/// Get the path to the codex-switcher config directory
pub fn get_config_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not find home directory")?;
    Ok(home.join(".codex-switcher"))
}

/// Get the path to accounts.json
pub fn get_accounts_file() -> Result<PathBuf> {
    Ok(get_config_dir()?.join("accounts.json"))
}

/// Load the accounts store from disk
pub fn load_accounts() -> Result<AccountsStore> {
    let path = get_accounts_file()?;

    if !path.exists() {
        return Ok(AccountsStore::default());
    }

    let content = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read accounts file: {}", path.display()))?;

    let store: AccountsStore = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse accounts file: {}", path.display()))?;

    Ok(store)
}

/// Save the accounts store to disk
pub fn save_accounts(store: &AccountsStore) -> Result<()> {
    let path = get_accounts_file()?;

    // Ensure the config directory exists
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create config directory: {}", parent.display()))?;
    }

    let content =
        serde_json::to_string_pretty(store).context("Failed to serialize accounts store")?;

    fs::write(&path, content)
        .with_context(|| format!("Failed to write accounts file: {}", path.display()))?;

    // Set restrictive permissions on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o600);
        fs::set_permissions(&path, perms)?;
    }

    Ok(())
}

/// Return a priority list containing each current account exactly once.
pub fn normalized_priority_account_ids(
    accounts: &[StoredAccount],
    priority_account_ids: &[String],
) -> Vec<String> {
    let account_ids: HashSet<&str> = accounts.iter().map(|account| account.id.as_str()).collect();
    let mut seen = HashSet::new();
    let mut normalized = Vec::with_capacity(accounts.len());

    for account_id in priority_account_ids {
        if account_ids.contains(account_id.as_str()) && seen.insert(account_id.as_str()) {
            normalized.push(account_id.clone());
        }
    }

    for account in accounts {
        if seen.insert(account.id.as_str()) {
            normalized.push(account.id.clone());
        }
    }

    normalized
}

fn sync_usage_automation_priority(store: &mut AccountsStore) {
    store.usage_automation.priority_account_ids = normalized_priority_account_ids(
        &store.accounts,
        &store.usage_automation.priority_account_ids,
    );
}

/// Add a new account to the store
pub fn add_account(account: StoredAccount) -> Result<StoredAccount> {
    let mut store = load_accounts()?;

    // Check for duplicate names
    if store.accounts.iter().any(|a| a.name == account.name) {
        anyhow::bail!("An account with name '{}' already exists", account.name);
    }

    let account_clone = account.clone();
    store.accounts.push(account);
    sync_usage_automation_priority(&mut store);

    // If this is the first account, make it active
    if store.accounts.len() == 1 {
        store.active_account_id = Some(account_clone.id.clone());
    }

    save_accounts(&store)?;
    Ok(account_clone)
}

/// Remove an account by ID
pub fn remove_account(account_id: &str) -> Result<()> {
    let mut store = load_accounts()?;

    let initial_len = store.accounts.len();
    store.accounts.retain(|a| a.id != account_id);

    if store.accounts.len() == initial_len {
        anyhow::bail!("Account not found: {account_id}");
    }

    // If we removed the active account, clear it or set to first available
    if store.active_account_id.as_deref() == Some(account_id) {
        store.active_account_id = store.accounts.first().map(|a| a.id.clone());
    }
    sync_usage_automation_priority(&mut store);

    save_accounts(&store)?;
    Ok(())
}

/// Update the active account ID
pub fn set_active_account(account_id: &str) -> Result<()> {
    let mut store = load_accounts()?;

    // Verify the account exists
    if !store.accounts.iter().any(|a| a.id == account_id) {
        anyhow::bail!("Account not found: {account_id}");
    }

    store.active_account_id = Some(account_id.to_string());
    save_accounts(&store)?;
    Ok(())
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

/// Update an account's last_used_at timestamp
pub fn touch_account(account_id: &str) -> Result<()> {
    let mut store = load_accounts()?;

    if let Some(account) = store.accounts.iter_mut().find(|a| a.id == account_id) {
        account.last_used_at = Some(chrono::Utc::now());
        save_accounts(&store)?;
    }

    Ok(())
}

/// Update an account's metadata (name, email, plan_type)
pub fn update_account_metadata(
    account_id: &str,
    name: Option<String>,
    email: Option<String>,
    plan_type: Option<String>,
) -> Result<()> {
    let mut store = load_accounts()?;

    // Check for duplicate names first (if renaming)
    if let Some(ref new_name) = name {
        if store
            .accounts
            .iter()
            .any(|a| a.id != account_id && a.name == *new_name)
        {
            anyhow::bail!("An account with name '{new_name}' already exists");
        }
    }

    // Now find and update the account
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

    save_accounts(&store)?;
    Ok(())
}

/// Update ChatGPT OAuth tokens for an account and return the updated account.
pub fn update_account_chatgpt_tokens(
    account_id: &str,
    id_token: String,
    access_token: String,
    refresh_token: String,
    chatgpt_account_id: Option<String>,
    email: Option<String>,
    plan_type: Option<String>,
) -> Result<StoredAccount> {
    let mut store = load_accounts()?;

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

    let updated = account.clone();
    save_accounts(&store)?;
    Ok(updated)
}

/// Get the list of masked account IDs
pub fn get_masked_account_ids() -> Result<Vec<String>> {
    let store = load_accounts()?;
    Ok(store.masked_account_ids.clone())
}

/// Set the list of masked account IDs
pub fn set_masked_account_ids(ids: Vec<String>) -> Result<()> {
    let mut store = load_accounts()?;
    store.masked_account_ids = ids;
    save_accounts(&store)?;
    Ok(())
}

/// Get usage warning and automatic switching settings.
pub fn get_usage_automation_settings() -> Result<UsageAutomationSettings> {
    let store = load_accounts()?;
    let mut settings = store.usage_automation.clone();
    settings.priority_account_ids =
        normalized_priority_account_ids(&store.accounts, &settings.priority_account_ids);
    Ok(settings)
}

/// Set usage warning and automatic switching settings.
pub fn set_usage_automation_settings(mut settings: UsageAutomationSettings) -> Result<()> {
    if !settings.warning_remaining_percent.is_finite()
        || !settings.auto_switch_remaining_percent.is_finite()
    {
        anyhow::bail!("Usage thresholds must be finite numbers");
    }

    if !(0.0..=100.0).contains(&settings.warning_remaining_percent)
        || !(0.0..=100.0).contains(&settings.auto_switch_remaining_percent)
    {
        anyhow::bail!("Usage thresholds must be between 0 and 100");
    }

    if settings.warning_remaining_percent < settings.auto_switch_remaining_percent {
        anyhow::bail!("Warning threshold must be greater than or equal to auto-switch threshold");
    }

    let mut store = load_accounts()?;
    settings.priority_account_ids =
        normalized_priority_account_ids(&store.accounts, &settings.priority_account_ids);
    store.usage_automation = settings;
    save_accounts(&store)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::normalized_priority_account_ids;
    use crate::types::StoredAccount;

    fn account_with_id(id: &str) -> StoredAccount {
        let mut account = StoredAccount::new_api_key(id.to_string(), format!("key-{id}"));
        account.id = id.to_string();
        account
    }

    #[test]
    fn normalized_priority_removes_invalid_and_duplicate_ids() {
        let accounts = vec![
            account_with_id("first"),
            account_with_id("second"),
            account_with_id("third"),
        ];

        let normalized = normalized_priority_account_ids(
            &accounts,
            &[
                "missing".to_string(),
                "second".to_string(),
                "second".to_string(),
                "first".to_string(),
            ],
        );

        assert_eq!(normalized, vec!["second", "first", "third"]);
    }

    #[test]
    fn normalized_priority_preserves_account_order_when_empty() {
        let accounts = vec![account_with_id("first"), account_with_id("second")];
        let normalized = normalized_priority_account_ids(&accounts, &[]);

        assert_eq!(normalized, vec!["first", "second"]);
    }
}
