//! Account storage module - manages reading and writing accounts.json

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use uuid::Uuid;

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
    write_file_atomically(&path, &content)
        .with_context(|| format!("Failed to write settings file: {}", path.display()))?;

    Ok(())
}

/// Write `content` to `path` by writing a sibling temp file and atomically
/// renaming it into place, instead of writing directly to `path`.
///
/// `fs::write` truncates the target file before writing the new bytes, so any
/// concurrent reader (the tray's 1s file watcher, the 60s usage poller, a
/// separate `codex-web` LAN process, or the official `codex` CLI reading
/// `auth.json`) that opens the file during that window observes a truncated or
/// empty file. `fs::rename` within the same directory is atomic, so readers
/// always see either the fully-old or fully-new content, never a partial
/// write. The temp filename is suffixed with a UUID rather than a PID so two
/// writers on the same process (e.g. a tray-triggered switch racing a
/// main-window switch) never collide on the same temp file. Shared across
/// `auth::storage` and `auth::switcher`, which both persist small JSON/text
/// files that outside readers can observe mid-write.
pub(crate) fn write_file_atomically(path: &std::path::Path, content: &str) -> Result<()> {
    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("tmp");
    let tmp_path = path.with_extension(format!("{extension}.tmp.{}", Uuid::new_v4()));

    let result = write_temp_then_rename(&tmp_path, path, content);
    if result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    result
}

fn write_temp_then_rename(
    tmp_path: &std::path::Path,
    path: &std::path::Path,
    content: &str,
) -> Result<()> {
    fs::write(tmp_path, content)
        .with_context(|| format!("Failed to write temp file: {}", tmp_path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(tmp_path, fs::Permissions::from_mode(0o600))
            .with_context(|| format!("Failed to set permissions: {}", tmp_path.display()))?;
    }

    fs::rename(tmp_path, path)
        .with_context(|| format!("Failed to finalize file: {}", path.display()))?;

    Ok(())
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

    write_file_atomically(&path, &content)
        .with_context(|| format!("Failed to write accounts file: {}", path.display()))?;

    Ok(())
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
        AuthData::ApiKey { .. } | AuthData::CodexAccessToken { .. } => {
            anyhow::bail!("Cannot update OAuth tokens for this account type");
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
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    /// Reproduces the "Failed to parse accounts file" bug: a reader (e.g. the
    /// tray's 1s file watcher) can open the file while a writer is mid-`fs::write`,
    /// since plain `fs::write` truncates before the new bytes land.
    #[test]
    fn concurrent_readers_never_observe_a_torn_write() {
        let dir = std::env::temp_dir().join(format!("codex-switcher-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("accounts.json");

        let payload_a = format!("{{\"marker\":\"A\",\"pad\":\"{}\"}}", "a".repeat(200_000));
        let payload_b = format!("{{\"marker\":\"B\",\"pad\":\"{}\"}}", "b".repeat(200_000));

        write_file_atomically(&path, &payload_a).unwrap();

        let stop = Arc::new(AtomicBool::new(false));
        let writer = {
            let stop = stop.clone();
            let path = path.clone();
            let payload_a = payload_a.clone();
            let payload_b = payload_b.clone();
            std::thread::spawn(move || {
                for i in 0..200 {
                    let payload = if i % 2 == 0 { &payload_b } else { &payload_a };
                    write_file_atomically(&path, payload).unwrap();
                }
                stop.store(true, Ordering::SeqCst);
            })
        };

        let mut reads = 0;
        while !stop.load(Ordering::SeqCst) {
            if let Ok(content) = fs::read_to_string(&path) {
                assert!(
                    content == payload_a || content == payload_b,
                    "torn read detected: got {} bytes (expected {} or {})",
                    content.len(),
                    payload_a.len(),
                    payload_b.len()
                );
                reads += 1;
            }
        }
        writer.join().unwrap();
        assert!(
            reads > 0,
            "test didn't overlap reads with writes — increase iterations"
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
