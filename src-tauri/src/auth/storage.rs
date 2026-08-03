//! Account storage module - manages reading and writing accounts.json

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};

use crate::types::{AccountsStore, AppSettings, AuthData, AuthState, StoredAccount};

static ACCOUNTS_STORE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn accounts_store_lock() -> Result<std::sync::MutexGuard<'static, ()>> {
    ACCOUNTS_STORE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| anyhow::anyhow!("Accounts store lock was poisoned"))
}

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

/// Load the accounts store from disk
pub fn load_accounts() -> Result<AccountsStore> {
    let _guard = accounts_store_lock()?;
    load_accounts_unlocked()
}

fn load_accounts_unlocked() -> Result<AccountsStore> {
    let path = get_accounts_file()?;
    load_accounts_from_path(&path)
}

fn load_accounts_from_path(path: &Path) -> Result<AccountsStore> {
    if !path.exists() {
        return Ok(AccountsStore::default());
    }

    let content = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read accounts file: {}", path.display()))?;

    let store: AccountsStore = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse accounts file: {}", path.display()))?;

    Ok(store)
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

fn save_accounts_to_path(path: &Path, store: &AccountsStore) -> Result<()> {
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

/// Atomically load, mutate, and save the accounts store within this process.
/// All credential rotations must use this helper so concurrent account updates
/// cannot overwrite one another with an older snapshot.
pub fn mutate_accounts<T>(
    mutator: impl FnOnce(&mut AccountsStore) -> Result<T>,
) -> Result<T> {
    let path = get_accounts_file()?;
    mutate_accounts_at_path(&path, mutator)
}

fn mutate_accounts_at_path<T>(
    path: &Path,
    mutator: impl FnOnce(&mut AccountsStore) -> Result<T>,
) -> Result<T> {
    let _guard = accounts_store_lock()?;
    let mut store = load_accounts_from_path(path)?;
    let result = mutator(&mut store)?;
    save_accounts_to_path(path, &store)?;
    Ok(result)
}

/// Add a new account to the store
pub fn add_account(account: StoredAccount) -> Result<StoredAccount> {
    mutate_accounts(|store| {
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
}

/// Remove an account by ID
pub fn remove_account(account_id: &str) -> Result<()> {
    mutate_accounts(|store| {
        let initial_len = store.accounts.len();
        store.accounts.retain(|a| a.id != account_id);

        if store.accounts.len() == initial_len {
            anyhow::bail!("Account not found: {account_id}");
        }

        if store.active_account_id.as_deref() == Some(account_id) {
            store.active_account_id = store.accounts.first().map(|a| a.id.clone());
        }
        Ok(())
    })
}

/// Update the active account ID
pub fn set_active_account(account_id: &str) -> Result<()> {
    mutate_accounts(|store| {
        if !store.accounts.iter().any(|a| a.id == account_id) {
            anyhow::bail!("Account not found: {account_id}");
        }
        store.active_account_id = Some(account_id.to_string());
        Ok(())
    })
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
    mutate_accounts(|store| {
        if let Some(account) = store.accounts.iter_mut().find(|a| a.id == account_id) {
            account.last_used_at = Some(chrono::Utc::now());
        }
        Ok(())
    })
}

/// Update an account's metadata (name, email, plan_type, subscription expiry)
pub fn update_account_metadata(
    account_id: &str,
    name: Option<String>,
    email: Option<String>,
    plan_type: Option<String>,
    subscription_expires_at: Option<Option<DateTime<Utc>>>,
) -> Result<StoredAccount> {
    mutate_accounts(|store| {
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
    mutate_accounts(|store| {
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
        account.auth_state = AuthState::Ready;
        Ok(account.clone())
    })
}

pub fn set_account_auth_state(account_id: &str, auth_state: AuthState) -> Result<StoredAccount> {
    mutate_accounts(|store| {
        let account = store
            .accounts
            .iter_mut()
            .find(|account| account.id == account_id)
            .context("Account not found")?;
        account.auth_state = auth_state;
        Ok(account.clone())
    })
}

/// Get the list of masked account IDs
pub fn get_masked_account_ids() -> Result<Vec<String>> {
    let store = load_accounts()?;
    Ok(store.masked_account_ids.clone())
}

/// Set the list of masked account IDs
pub fn set_masked_account_ids(ids: Vec<String>) -> Result<()> {
    mutate_accounts(|store| {
        store.masked_account_ids = ids;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::{load_accounts_from_path, mutate_accounts_at_path, save_accounts_to_path};
    use crate::types::{AccountsStore, AuthData, AuthState, StoredAccount};
    use std::sync::{Arc, Barrier};

    #[test]
    fn concurrent_account_mutations_preserve_both_rotated_tokens() {
        let path = std::env::temp_dir().join(format!(
            "codex-switcher-storage-{}.json",
            uuid::Uuid::new_v4()
        ));
        let first = StoredAccount::new_chatgpt(
            "first".into(),
            Some("first@example.com".into()),
            Some("plus".into()),
            None,
            "id-first".into(),
            "access-first".into(),
            "refresh-first".into(),
            Some("account-first".into()),
        );
        let second = StoredAccount::new_chatgpt(
            "second".into(),
            Some("second@example.com".into()),
            Some("plus".into()),
            None,
            "id-second".into(),
            "access-second".into(),
            "refresh-second".into(),
            Some("account-second".into()),
        );
        let first_id = first.id.clone();
        let second_id = second.id.clone();
        let mut store = AccountsStore::default();
        store.accounts = vec![first, second];
        save_accounts_to_path(&path, &store).unwrap();

        let barrier = Arc::new(Barrier::new(3));
        let handles = [
            (first_id.clone(), "rotated-first".to_string()),
            (second_id.clone(), "rotated-second".to_string()),
        ]
        .into_iter()
        .map(|(account_id, refresh_token)| {
            let path = path.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                mutate_accounts_at_path(&path, |store| {
                    let account = store
                        .accounts
                        .iter_mut()
                        .find(|account| account.id == account_id)
                        .unwrap();
                    let AuthData::ChatGPT {
                        refresh_token: stored_refresh_token,
                        ..
                    } = &mut account.auth_data
                    else {
                        panic!("expected ChatGPT account");
                    };
                    *stored_refresh_token = refresh_token;
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    Ok(())
                })
                .unwrap();
            })
        })
        .collect::<Vec<_>>();

        barrier.wait();
        for handle in handles {
            handle.join().unwrap();
        }

        let final_store = load_accounts_from_path(&path).unwrap();
        let token_for = |account_id: &str| {
            let account = final_store
                .accounts
                .iter()
                .find(|account| account.id == account_id)
                .unwrap();
            match &account.auth_data {
                AuthData::ChatGPT { refresh_token, .. } => refresh_token.clone(),
                AuthData::ApiKey { .. } => panic!("expected ChatGPT account"),
            }
        };
        assert_eq!(token_for(&first_id), "rotated-first");
        assert_eq!(token_for(&second_id), "rotated-second");

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn reauthentication_required_state_survives_reload() {
        let path = std::env::temp_dir().join(format!(
            "codex-switcher-auth-state-{}.json",
            uuid::Uuid::new_v4()
        ));
        let account = StoredAccount::new_chatgpt(
            "expired".into(),
            Some("expired@example.com".into()),
            Some("plus".into()),
            None,
            "id".into(),
            "access".into(),
            "refresh".into(),
            Some("account-expired".into()),
        );
        let account_id = account.id.clone();
        let mut store = AccountsStore::default();
        store.accounts.push(account);
        save_accounts_to_path(&path, &store).unwrap();

        mutate_accounts_at_path(&path, |store| {
            store.accounts[0].auth_state = AuthState::ReauthRequired;
            Ok(())
        })
        .unwrap();

        let restored = load_accounts_from_path(&path).unwrap();
        assert_eq!(restored.accounts[0].id, account_id);
        assert_eq!(restored.accounts[0].auth_state, AuthState::ReauthRequired);

        let _ = std::fs::remove_file(path);
    }
}
