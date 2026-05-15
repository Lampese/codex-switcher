//! Account storage module - manages reading and writing accounts.json

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};

use super::atomic_write::{parse_json_with_auto_restore, write_string_atomic};
use crate::types::{AccountsStore, AuthData, StoredAccount};

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

    // Auto-restore from .bak if JSON is corrupted
    let store: AccountsStore = parse_json_with_auto_restore(&path, &content)
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

    // Atomic write: temp file → rename, with auto .bak backup
    write_string_atomic(&path, &content)
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

/// Add a new account to the store
pub fn add_account(account: StoredAccount) -> Result<StoredAccount> {
    let mut store = load_accounts()?;
    let added = add_account_to_store(&mut store, account)?;
    save_accounts(&store)?;
    Ok(added)
}

fn add_account_to_store(
    store: &mut AccountsStore,
    account: StoredAccount,
) -> Result<StoredAccount> {
    // Check for duplicate names
    if store.accounts.iter().any(|a| a.name == account.name) {
        anyhow::bail!("An account with name '{}' already exists", account.name);
    }

    let account_clone = account.clone();
    store.accounts.push(account);
    Ok(account_clone)
}

/// Remove an account by ID
pub fn remove_account(account_id: &str) -> Result<()> {
    let mut store = load_accounts()?;
    remove_account_from_store(&mut store, account_id)?;
    save_accounts(&store)?;
    Ok(())
}

fn remove_account_from_store(store: &mut AccountsStore, account_id: &str) -> Result<()> {
    let initial_len = store.accounts.len();
    store.accounts.retain(|a| a.id != account_id);

    if store.accounts.len() == initial_len {
        anyhow::bail!("Account not found: {account_id}");
    }

    // If we removed the active account, clear it. Only an explicit switch should
    // make another account active because switching also writes Codex auth.json.
    if store.active_account_id.as_deref() == Some(account_id) {
        store.active_account_id = None;
    }

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

/// Update an account's metadata (name, email, plan_type, subscription expiry)
pub fn update_account_metadata(
    account_id: &str,
    name: Option<String>,
    email: Option<String>,
    plan_type: Option<String>,
    subscription_expires_at: Option<Option<DateTime<Utc>>>,
) -> Result<StoredAccount> {
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

    if let Some(subscription_expires_at) = subscription_expires_at {
        account.subscription_expires_at = subscription_expires_at;
    }

    let updated = account.clone();
    save_accounts(&store)?;
    Ok(updated)
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
    subscription_expires_at: Option<DateTime<Utc>>,
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

    if let Some(subscription_expires_at) = subscription_expires_at {
        account.subscription_expires_at = Some(subscription_expires_at);
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

#[cfg(test)]
mod tests {
    use super::{add_account_to_store, remove_account_from_store};
    use crate::types::{AccountsStore, StoredAccount};

    #[test]
    fn adding_first_account_does_not_mark_it_active() {
        let mut store = AccountsStore::default();
        let account = StoredAccount::new_api_key("new".to_string(), "sk-test".to_string());

        let added = add_account_to_store(&mut store, account).expect("account should be added");

        assert_eq!(store.accounts.len(), 1);
        assert_eq!(store.accounts[0].id, added.id);
        assert_eq!(store.active_account_id, None);
    }

    #[test]
    fn adding_account_keeps_existing_active_account() {
        let active = StoredAccount::new_api_key("active".to_string(), "sk-active".to_string());
        let active_id = active.id.clone();
        let mut store = AccountsStore {
            accounts: vec![active],
            active_account_id: Some(active_id.clone()),
            ..AccountsStore::default()
        };

        let added = StoredAccount::new_api_key("new".to_string(), "sk-new".to_string());
        add_account_to_store(&mut store, added).expect("account should be added");

        assert_eq!(store.accounts.len(), 2);
        assert_eq!(store.active_account_id.as_deref(), Some(active_id.as_str()));
    }

    #[test]
    fn removing_active_account_clears_active_instead_of_selecting_another_account() {
        let active = StoredAccount::new_api_key("active".to_string(), "sk-active".to_string());
        let active_id = active.id.clone();
        let other = StoredAccount::new_api_key("other".to_string(), "sk-other".to_string());
        let mut store = AccountsStore {
            accounts: vec![active, other],
            active_account_id: Some(active_id.clone()),
            ..AccountsStore::default()
        };

        remove_account_from_store(&mut store, &active_id).expect("account should be removed");

        assert_eq!(store.accounts.len(), 1);
        assert_eq!(store.active_account_id, None);
    }
}
