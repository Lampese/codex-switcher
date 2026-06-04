//! Account storage module - manages reading and writing accounts.json

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};

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

fn normalized_identity_value(value: Option<&str>) -> Option<String> {
    let normalized = value?.trim().to_ascii_lowercase();
    if normalized.is_empty() || matches!(normalized.as_str(), "unknown" | "n/a" | "none" | "?") {
        None
    } else {
        Some(normalized)
    }
}

fn chatgpt_identity(account: &StoredAccount) -> Option<(String, String)> {
    match &account.auth_data {
        AuthData::ChatGPT { .. } => Some((
            normalized_identity_value(account.email.as_deref())?,
            normalized_identity_value(account.plan_type.as_deref())?,
        )),
        AuthData::ApiKey { .. } => None,
    }
}

fn accounts_have_same_chatgpt_identity(left: &StoredAccount, right: &StoredAccount) -> bool {
    match (chatgpt_identity(left), chatgpt_identity(right)) {
        (Some(left_identity), Some(right_identity)) => left_identity == right_identity,
        _ => false,
    }
}

fn account_plan_suffix(account: &StoredAccount) -> String {
    normalized_identity_value(account.plan_type.as_deref()).unwrap_or_else(|| "account".to_string())
}

fn ensure_unique_account_name(account: &mut StoredAccount, existing: &[StoredAccount]) {
    if !existing.iter().any(|a| a.name == account.name) {
        return;
    }

    let base_name = account.name.clone();
    let suffix = account_plan_suffix(account);
    let mut candidate = format!("{base_name} ({suffix})");
    let mut counter = 2;

    while existing.iter().any(|a| a.name == candidate) {
        candidate = format!("{base_name} ({suffix} {counter})");
        counter += 1;
    }

    account.name = candidate;
}

/// Add a new account to the store
pub fn add_account(mut account: StoredAccount) -> Result<StoredAccount> {
    let mut store = load_accounts()?;

    if store
        .accounts
        .iter()
        .any(|existing| accounts_have_same_chatgpt_identity(existing, &account))
    {
        let email = account.email.as_deref().unwrap_or("unknown email");
        let plan = account.plan_type.as_deref().unwrap_or("unknown plan");
        anyhow::bail!("An account with email '{email}' and plan '{plan}' already exists");
    }

    ensure_unique_account_name(&mut account, &store.accounts);

    // Check for duplicate names after automatic disambiguation.
    if store.accounts.iter().any(|a| a.name == account.name) {
        anyhow::bail!("An account with name '{}' already exists", account.name);
    }

    let account_clone = account.clone();
    store.accounts.push(account);

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
    use super::{accounts_have_same_chatgpt_identity, ensure_unique_account_name};
    use crate::types::StoredAccount;

    fn chatgpt_account(name: &str, email: &str, plan: &str, account_id: &str) -> StoredAccount {
        StoredAccount::new_chatgpt(
            name.to_string(),
            Some(email.to_string()),
            Some(plan.to_string()),
            None,
            "id-token".to_string(),
            "access-token".to_string(),
            "refresh-token".to_string(),
            Some(account_id.to_string()),
        )
    }

    #[test]
    fn same_email_different_plan_is_not_same_chatgpt_identity() {
        let plus = chatgpt_account("Personal", "user@example.com", "plus", "acc_plus");
        let team = chatgpt_account("Team", "USER@example.com", "team", "acc_team");

        assert!(!accounts_have_same_chatgpt_identity(&plus, &team));
    }

    #[test]
    fn same_email_same_plan_is_same_chatgpt_identity() {
        let first = chatgpt_account("First", "user@example.com", "plus", "acc_one");
        let second = chatgpt_account("Second", "USER@example.com", "PLUS", "acc_two");

        assert!(accounts_have_same_chatgpt_identity(&first, &second));
    }

    #[test]
    fn duplicate_display_name_for_distinct_identity_gets_plan_suffix() {
        let existing = chatgpt_account("user@example.com", "user@example.com", "plus", "acc_plus");
        let mut candidate =
            chatgpt_account("user@example.com", "user@example.com", "team", "acc_team");

        ensure_unique_account_name(&mut candidate, &[existing]);

        assert_eq!(candidate.name, "user@example.com (team)");
    }
}
