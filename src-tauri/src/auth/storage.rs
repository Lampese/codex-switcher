//! Account storage module - manages reading and writing accounts.json

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(test)]
use std::{cell::RefCell, thread_local};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};

use crate::types::{
    parse_chatgpt_id_token_claims, AccountsStore, AppSettings, AuthData, AuthDotJson, StoredAccount,
};

#[cfg(test)]
thread_local! {
    static TEST_CONFIG_DIR: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

pub fn sync_active_account_tokens(store: &mut AccountsStore, auth: &AuthDotJson) -> bool {
    let Some(active_id) = store.active_account_id.as_deref() else {
        return false;
    };
    let Some(tokens) = auth.tokens.as_ref() else {
        return false;
    };
    let Some(account) = store
        .accounts
        .iter_mut()
        .find(|account| account.id == active_id)
    else {
        return false;
    };
    let AuthData::ChatGPT {
        id_token,
        access_token,
        refresh_token,
        account_id,
    } = &mut account.auth_data
    else {
        return false;
    };

    let stored_account_id = parse_chatgpt_id_token_claims(id_token)
        .account_id
        .or_else(|| account_id.clone());
    let current_account_id = parse_chatgpt_id_token_claims(&tokens.id_token)
        .account_id
        .or_else(|| tokens.account_id.clone());
    let (Some(stored_account_id), Some(current_account_id)) =
        (stored_account_id, current_account_id)
    else {
        return false;
    };
    if stored_account_id != current_account_id {
        return false;
    }
    if runtime_snapshot_is_older_than_stored_credentials(account.last_refresh_at, auth.last_refresh)
    {
        return false;
    }

    let changed = *id_token != tokens.id_token
        || *access_token != tokens.access_token
        || *refresh_token != tokens.refresh_token
        || account_id.as_ref() != Some(&current_account_id)
        || account.last_refresh_at != auth.last_refresh;
    if !changed {
        return false;
    }

    id_token.clone_from(&tokens.id_token);
    access_token.clone_from(&tokens.access_token);
    refresh_token.clone_from(&tokens.refresh_token);
    *account_id = Some(current_account_id);
    account.last_refresh_at = auth.last_refresh;
    true
}

fn runtime_snapshot_is_older_than_stored_credentials(
    stored_last_refresh: Option<DateTime<Utc>>,
    runtime_last_refresh: Option<DateTime<Utc>>,
) -> bool {
    match (stored_last_refresh, runtime_last_refresh) {
        (Some(stored), Some(runtime)) => runtime < stored,
        // A runtime snapshot without a generation cannot replace credentials
        // that were already persisted by a known refresh or credential update.
        (Some(_), None) => true,
        (None, _) => false,
    }
}

pub fn reconcile_active_projection(store: &mut AccountsStore, auth: Option<&AuthDotJson>) -> bool {
    let previous_active = store.active_account_id.clone();

    let matching_id = auth.and_then(|auth| {
        if let Some(api_key) = auth.openai_api_key.as_ref() {
            return store
                .accounts
                .iter()
                .find_map(|account| match &account.auth_data {
                    AuthData::ApiKey { key } if key == api_key => Some(account.id.clone()),
                    _ => None,
                });
        }

        let tokens = auth.tokens.as_ref()?;
        let runtime_account_id = parse_chatgpt_id_token_claims(&tokens.id_token)
            .account_id
            .or_else(|| tokens.account_id.clone())?;

        store
            .accounts
            .iter()
            .find_map(|account| match &account.auth_data {
                AuthData::ChatGPT {
                    id_token,
                    account_id,
                    ..
                } => {
                    let stored_account_id = parse_chatgpt_id_token_claims(id_token)
                        .account_id
                        .or_else(|| account_id.clone());
                    (stored_account_id.as_deref() == Some(runtime_account_id.as_str()))
                        .then(|| account.id.clone())
                }
                _ => None,
            })
    });

    store.active_account_id = matching_id;

    let mut changed = store.active_account_id != previous_active;
    if let (Some(auth), Some(active_id)) = (auth, store.active_account_id.clone()) {
        let before = store
            .accounts
            .iter()
            .find(|account| account.id == active_id)
            .cloned();
        if sync_active_account_tokens(store, auth) {
            changed = true;
        } else if before.is_none() {
            changed = true;
        }
    }

    changed
}

/// Get the path to the codex-switcher config directory
pub fn get_config_dir() -> Result<PathBuf> {
    #[cfg(test)]
    if let Some(path) = TEST_CONFIG_DIR.with(|dir| dir.borrow().clone()) {
        return Ok(path);
    }

    let home = dirs::home_dir().context("Could not find home directory")?;
    Ok(home.join(".codex-switcher"))
}

#[cfg(test)]
pub(crate) fn with_test_config_dir<T>(path: PathBuf, operation: impl FnOnce() -> T) -> T {
    TEST_CONFIG_DIR.with(|dir| {
        let previous = dir.replace(Some(path));
        let result = operation();
        dir.replace(previous);
        result
    })
}

/// Get the path to accounts.json
pub fn get_accounts_file() -> Result<PathBuf> {
    Ok(get_config_dir()?.join("accounts.json"))
}

pub fn get_settings_file() -> Result<PathBuf> {
    Ok(get_config_dir()?.join("settings.json"))
}

static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(crate) struct MutationLock {
    _file: File,
}

impl Drop for MutationLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            unsafe {
                libc::flock(self._file.as_raw_fd(), libc::LOCK_UN);
            }
        }
    }
}

pub(crate) fn acquire_mutation_lock(lock_name: &str) -> Result<MutationLock> {
    let config_dir = get_config_dir()?;
    fs::create_dir_all(&config_dir).with_context(|| {
        format!(
            "Failed to create config directory: {}",
            config_dir.display()
        )
    })?;
    acquire_mutation_lock_at(&config_dir.join(lock_name))
}

pub(crate) async fn acquire_auth_operation_lock() -> Result<MutationLock> {
    tokio::task::spawn_blocking(|| acquire_mutation_lock("auth-operation.lock"))
        .await
        .context("Auth operation lock task failed")?
}

fn acquire_mutation_lock_at(path: &Path) -> Result<MutationLock> {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::OpenOptionsExt;

        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(path)
            .with_context(|| format!("Failed to open mutation lock: {}", path.display()))?;

        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
        if result != 0 {
            return Err(std::io::Error::last_os_error())
                .with_context(|| format!("Failed to acquire mutation lock: {}", path.display()));
        }
        return Ok(MutationLock { _file: file });
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            match OpenOptions::new()
                .create(true)
                .read(true)
                .write(true)
                .share_mode(0)
                .open(path)
            {
                Ok(file) => return Ok(MutationLock { _file: file }),
                Err(error) if std::time::Instant::now() < deadline => {
                    let _ = error;
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("Timed out waiting for mutation lock: {}", path.display())
                    });
                }
            }
        }
    }

    #[cfg(not(any(unix, windows)))]
    {
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(path)
            .with_context(|| format!("Failed to open mutation lock: {}", path.display()))?;
        Ok(MutationLock { _file: file })
    }
}

pub(crate) fn write_file_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    write_file_atomic_with_pre_replace(path, bytes, || Ok(()))
}

fn write_file_atomic_with_pre_replace(
    path: &Path,
    bytes: &[u8],
    before_replace: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let parent = path
        .parent()
        .context("Atomic write target has no parent directory")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("Failed to create directory: {}", parent.display()))?;

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("Atomic write target has an invalid file name")?;
    let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temp_path = parent.join(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        sequence
    ));

    let mut options = OpenOptions::new();
    options.create_new(true).write(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let mut file = options
        .open(&temp_path)
        .with_context(|| format!("Failed to create temporary file: {}", temp_path.display()))?;

    let write_result = (|| -> Result<()> {
        file.write_all(bytes)
            .with_context(|| format!("Failed to write temporary file: {}", temp_path.display()))?;
        file.sync_all()
            .with_context(|| format!("Failed to sync temporary file: {}", temp_path.display()))?;
        drop(file);

        before_replace()?;
        replace_file(&temp_path, path)?;

        #[cfg(unix)]
        {
            let directory = File::open(parent)
                .with_context(|| format!("Failed to open directory: {}", parent.display()))?;
            directory
                .sync_all()
                .with_context(|| format!("Failed to sync directory: {}", parent.display()))?;
        }

        Ok(())
    })();

    if write_result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }

    write_result
}

#[cfg(not(windows))]
fn replace_file(from: &Path, to: &Path) -> Result<()> {
    fs::rename(from, to).with_context(|| {
        format!(
            "Failed to atomically replace {} with {}",
            to.display(),
            from.display()
        )
    })
}

#[cfg(windows)]
fn replace_file(from: &Path, to: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let from_wide: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to_wide: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    let result = unsafe {
        MoveFileExW(
            from_wide.as_ptr(),
            to_wide.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        return Err(std::io::Error::last_os_error()).with_context(|| {
            format!(
                "Failed to atomically replace {} with {}",
                to.display(),
                from.display()
            )
        });
    }
    Ok(())
}

/// Load the accounts store from disk
pub fn load_accounts() -> Result<AccountsStore> {
    load_accounts_from_path(&get_accounts_file()?)
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
    let raw: serde_json::Value = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse settings file: {}", path.display()))?;
    let had_language_preference = raw.get("ui_language_preference").is_some()
        || raw.get("language").is_some();

    let mut settings: AppSettings = serde_json::from_value(raw)
        .with_context(|| format!("Failed to parse settings file: {}", path.display()))?;

    // Existing installations predate localization. Keep their observable
    // English UI on upgrade instead of silently switching to the OS language.
    if !had_language_preference {
        settings.ui_language_preference = crate::types::UiLanguagePreference::English;
    }

    Ok(settings)
}

pub fn save_app_settings(settings: &AppSettings) -> Result<()> {
    let _lock = acquire_mutation_lock("settings.lock")?;
    save_app_settings_unlocked(settings)
}

fn save_app_settings_unlocked(settings: &AppSettings) -> Result<()> {
    let path = get_settings_file()?;
    let content = serde_json::to_vec_pretty(settings).context("Failed to serialize settings")?;
    write_file_atomic(&path, &content)
}

pub fn mutate_app_settings<T>(mutate: impl FnOnce(&mut AppSettings) -> Result<T>) -> Result<T> {
    let _lock = acquire_mutation_lock("settings.lock")?;
    let mut settings = load_app_settings()?;
    let result = mutate(&mut settings)?;
    save_app_settings_unlocked(&settings)?;
    Ok(result)
}

/// Save the accounts store to disk
pub fn save_accounts(store: &AccountsStore) -> Result<()> {
    let _lock = acquire_mutation_lock("accounts.lock")?;
    save_accounts_unlocked(store)
}

fn save_accounts_unlocked(store: &AccountsStore) -> Result<()> {
    save_accounts_unlocked_at(&get_accounts_file()?, store)
}

fn save_accounts_unlocked_at(path: &Path, store: &AccountsStore) -> Result<()> {
    let content = serde_json::to_vec_pretty(store).context("Failed to serialize accounts store")?;
    write_file_atomic(&path, &content)
}

/// Apply one logical account-store mutation and persist its resulting snapshot.
/// This boundary owns the complete read-modify-write transaction.
pub fn mutate_accounts<T>(mutate: impl FnOnce(&mut AccountsStore) -> Result<T>) -> Result<T> {
    mutate_accounts_in_dir(&get_config_dir()?, mutate)
}

fn mutate_accounts_in_dir<T>(
    config_dir: &Path,
    mutate: impl FnOnce(&mut AccountsStore) -> Result<T>,
) -> Result<T> {
    fs::create_dir_all(config_dir).with_context(|| {
        format!(
            "Failed to create config directory: {}",
            config_dir.display()
        )
    })?;
    let _lock = acquire_mutation_lock_at(&config_dir.join("accounts.lock"))?;
    let accounts_path = config_dir.join("accounts.json");
    let mut store = load_accounts_from_path(&accounts_path)?;
    let result = mutate(&mut store)?;
    save_accounts_unlocked_at(&accounts_path, &store)?;
    Ok(result)
}

#[cfg(test)]
pub(crate) fn mutate_accounts_at<T>(
    config_dir: &Path,
    mutate: impl FnOnce(&mut AccountsStore) -> Result<T>,
) -> Result<T> {
    mutate_accounts_in_dir(config_dir, mutate)
}

/// Add a new account to the store
pub fn add_account(account: StoredAccount) -> Result<StoredAccount> {
    mutate_accounts(|store| add_account_to_store(store, account))
}

fn add_account_to_store(
    store: &mut AccountsStore,
    account: StoredAccount,
) -> Result<StoredAccount> {
    if store.accounts.iter().any(|a| a.name == account.name) {
        anyhow::bail!("An account with name '{}' already exists", account.name);
    }

    store.accounts.push(account.clone());
    Ok(account)
}

/// Remove an account by ID
pub fn remove_account(account_id: &str) -> Result<()> {
    mutate_accounts(|store| {
        if store.active_account_id.as_deref() == Some(account_id) {
            anyhow::bail!("Cannot delete the active account; switch to another account first");
        }

        let initial_len = store.accounts.len();
        store.accounts.retain(|a| a.id != account_id);

        if store.accounts.len() == initial_len {
            anyhow::bail!("Account not found: {account_id}");
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
        if let Some(new_email) = email {
            account.email = Some(new_email);
        }
        if let Some(new_plan_type) = plan_type {
            account.plan_type = Some(new_plan_type);
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
    let refreshed_at = Utc::now();
    mutate_accounts(|store| {
        update_account_chatgpt_tokens_in_store(
            store,
            account_id,
            id_token,
            access_token,
            refresh_token,
            chatgpt_account_id,
            email,
            plan_type,
            subscription_expires_at,
            refreshed_at,
        )
    })
}

fn update_account_chatgpt_tokens_in_store(
    store: &mut AccountsStore,
    account_id: &str,
    id_token: String,
    access_token: String,
    refresh_token: String,
    chatgpt_account_id: Option<String>,
    email: Option<String>,
    plan_type: Option<String>,
    subscription_expires_at: Option<DateTime<Utc>>,
    refreshed_at: DateTime<Utc>,
) -> Result<StoredAccount> {
    let account = store
        .accounts
        .iter_mut()
        .find(|account| account.id == account_id)
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

    account.last_refresh_at = Some(refreshed_at);

    Ok(account.clone())
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
    use super::{
        acquire_mutation_lock_at, add_account_to_store, reconcile_active_projection,
        sync_active_account_tokens, update_account_chatgpt_tokens_in_store, write_file_atomic,
        write_file_atomic_with_pre_replace,
    };
    use crate::types::{AccountsStore, AuthData, AuthDotJson, StoredAccount, TokenData};
    use base64::Engine;
    use chrono::{Duration, TimeZone, Utc};

    #[test]
    fn mutation_lock_serializes_competing_writers() {
        let dir = std::env::temp_dir().join(format!(
            "codex-switcher-mutation-lock-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.lock");

        let first = acquire_mutation_lock_at(&path).unwrap();
        let second_path = path.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let second = acquire_mutation_lock_at(&second_path).unwrap();
            tx.send(()).unwrap();
            drop(second);
        });

        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(rx.try_recv().is_err());

        drop(first);
        rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        worker.join().unwrap();

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn atomic_write_preserves_old_file_when_pre_replace_step_fails() {
        let dir = std::env::temp_dir().join(format!(
            "codex-switcher-atomic-failure-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");
        std::fs::write(&path, b"old").unwrap();

        let result = write_file_atomic_with_pre_replace(&path, b"new", || {
            anyhow::bail!("injected failure before replacement")
        });

        assert!(result.is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"old");
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn adding_first_account_does_not_claim_runtime_activation() {
        let mut store = AccountsStore::default();
        let profile = account("A", "workspace-a", "a1");
        let profile_id = profile.id.clone();

        let added = add_account_to_store(&mut store, profile).unwrap();

        assert_eq!(added.id, profile_id);
        assert!(store
            .accounts
            .iter()
            .any(|account| account.id == profile_id));
        assert_eq!(store.active_account_id, None);
    }

    #[test]
    fn atomic_write_replaces_existing_file() {
        let dir = std::env::temp_dir().join(format!(
            "codex-switcher-atomic-write-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");
        std::fs::write(&path, b"old").unwrap();

        write_file_atomic(&path, b"new").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn atomic_secret_write_has_restrictive_permissions_from_creation() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!(
            "codex-switcher-atomic-permissions-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("auth.json");

        write_file_atomic(&path, br#"{"tokens":{}}"#).unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);

        std::fs::remove_dir_all(dir).unwrap();
    }

    fn account(name: &str, account_id: &str, suffix: &str) -> StoredAccount {
        StoredAccount::new_chatgpt(
            name.into(),
            None,
            None,
            None,
            format!("id-{suffix}"),
            format!("access-{suffix}"),
            format!("refresh-{suffix}"),
            Some(account_id.into()),
        )
    }

    fn auth(account_id: &str, suffix: &str) -> AuthDotJson {
        AuthDotJson {
            openai_api_key: None,
            tokens: Some(TokenData {
                id_token: format!("id-{suffix}"),
                access_token: format!("access-{suffix}"),
                refresh_token: format!("refresh-{suffix}"),
                account_id: Some(account_id.into()),
            }),
            last_refresh: None,
        }
    }

    fn auth_with_refresh_at(
        account_id: &str,
        suffix: &str,
        last_refresh: chrono::DateTime<Utc>,
    ) -> AuthDotJson {
        let mut auth = auth(account_id, suffix);
        auth.last_refresh = Some(last_refresh);
        auth
    }

    fn refresh_token(account: &StoredAccount) -> &str {
        match &account.auth_data {
            AuthData::ChatGPT { refresh_token, .. } => refresh_token,
            AuthData::ApiKey { .. } => panic!("expected ChatGPT account"),
        }
    }

    fn id_token_with_account_id(account_id: &str, suffix: &str) -> String {
        let payload =
            format!(r#"{{"https://api.openai.com/auth":{{"chatgpt_account_id":"{account_id}"}}}}"#);
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload);
        format!("header.{encoded}.{suffix}")
    }

    #[test]
    fn projection_follows_matching_runtime_chatgpt_account_and_tokens() {
        let mut account_a = account("A", "workspace-a", "a1");
        let account_a_id = account_a.id.clone();
        let runtime_refresh_at = account_a.last_refresh_at.unwrap() + Duration::seconds(1);
        let account_b = account("B", "workspace-b", "b1");
        let account_b_id = account_b.id.clone();
        let mut store = AccountsStore {
            accounts: vec![account_a.clone(), account_b],
            active_account_id: Some(account_b_id),
            ..AccountsStore::default()
        };

        let live = auth_with_refresh_at("workspace-a", "a2", runtime_refresh_at);
        assert!(reconcile_active_projection(&mut store, Some(&live)));
        assert_eq!(
            store.active_account_id.as_deref(),
            Some(account_a_id.as_str())
        );
        let AuthData::ChatGPT {
            refresh_token: stored_refresh_token,
            ..
        } = &store.accounts[0].auth_data
        else {
            panic!("expected ChatGPT account");
        };
        assert_eq!(stored_refresh_token, "refresh-a2");

        account_a = store.accounts.remove(0);
        assert_eq!(refresh_token(&account_a), "refresh-a2");
    }

    #[test]
    fn projection_clears_when_runtime_is_logged_out_or_unknown() {
        let account_a = account("A", "workspace-a", "a1");
        let account_a_id = account_a.id.clone();
        let mut store = AccountsStore {
            accounts: vec![account_a],
            active_account_id: Some(account_a_id),
            ..AccountsStore::default()
        };

        assert!(reconcile_active_projection(&mut store, None));
        assert!(store.active_account_id.is_none());

        let unknown = auth("workspace-unknown", "x");
        assert!(!reconcile_active_projection(&mut store, Some(&unknown)));
        assert!(store.active_account_id.is_none());
    }

    #[test]
    fn preserves_rotated_tokens_before_switching_away_and_back() {
        let account_a = account("A", "workspace-a", "a1");
        let account_a_id = account_a.id.clone();
        let runtime_refresh_at = account_a.last_refresh_at.unwrap() + Duration::seconds(1);
        let account_b = account("B", "workspace-b", "b1");
        let account_b_id = account_b.id.clone();
        let mut store = AccountsStore {
            accounts: vec![account_a, account_b],
            active_account_id: Some(account_a_id.clone()),
            ..AccountsStore::default()
        };

        assert!(!sync_active_account_tokens(
            &mut store,
            &auth("workspace-b", "wrong-account")
        ));
        assert_eq!(refresh_token(&store.accounts[0]), "refresh-a1");

        let mut auth_without_top_level_id = auth("workspace-b", "missing-id");
        let tokens = auth_without_top_level_id.tokens.as_mut().unwrap();
        tokens.id_token = id_token_with_account_id("workspace-b", "signature");
        tokens.account_id = None;
        assert!(!sync_active_account_tokens(
            &mut store,
            &auth_without_top_level_id
        ));
        assert_eq!(refresh_token(&store.accounts[0]), "refresh-a1");

        let mut auth_without_identity = auth("workspace-a", "unknown");
        auth_without_identity.tokens.as_mut().unwrap().account_id = None;
        assert!(!sync_active_account_tokens(
            &mut store,
            &auth_without_identity
        ));
        assert_eq!(refresh_token(&store.accounts[0]), "refresh-a1");

        assert!(sync_active_account_tokens(
            &mut store,
            &auth_with_refresh_at("workspace-a", "a2", runtime_refresh_at)
        ));
        store.active_account_id = Some(account_b_id);

        let restored_a = store
            .accounts
            .iter()
            .find(|account| account.id == account_a_id)
            .unwrap();
        let AuthData::ChatGPT { refresh_token, .. } = &restored_a.auth_data else {
            panic!("expected ChatGPT account");
        };
        assert_eq!(refresh_token, "refresh-a2");
    }

    #[test]
    fn rejects_live_tokens_when_stored_account_identity_is_unknown() {
        let mut account = account("A", "workspace-a", "a1");
        let account_id = account.id.clone();
        let AuthData::ChatGPT {
            id_token,
            account_id: chatgpt_account_id,
            ..
        } = &mut account.auth_data
        else {
            panic!("expected ChatGPT account");
        };
        *id_token = "opaque-id-token".into();
        *chatgpt_account_id = None;

        let mut store = AccountsStore {
            accounts: vec![account],
            active_account_id: Some(account_id),
            ..AccountsStore::default()
        };

        assert!(!sync_active_account_tokens(
            &mut store,
            &auth("workspace-a", "a2")
        ));
        assert_eq!(refresh_token(&store.accounts[0]), "refresh-a1");
    }

    #[test]
    fn derives_stored_identity_from_id_token_and_backfills_account_id() {
        let mut account = account("A", "workspace-a", "a1");
        let account_id = account.id.clone();
        let runtime_refresh_at = account.last_refresh_at.unwrap() + Duration::seconds(1);
        let AuthData::ChatGPT {
            id_token,
            account_id: chatgpt_account_id,
            ..
        } = &mut account.auth_data
        else {
            panic!("expected ChatGPT account");
        };
        *id_token = id_token_with_account_id("workspace-a", "stored");
        *chatgpt_account_id = None;

        let mut store = AccountsStore {
            accounts: vec![account],
            active_account_id: Some(account_id),
            ..AccountsStore::default()
        };

        assert!(sync_active_account_tokens(
            &mut store,
            &auth_with_refresh_at("workspace-a", "a2", runtime_refresh_at)
        ));
        let AuthData::ChatGPT { account_id, .. } = &store.accounts[0].auth_data else {
            panic!("expected ChatGPT account");
        };
        assert_eq!(account_id.as_deref(), Some("workspace-a"));
        assert_eq!(refresh_token(&store.accounts[0]), "refresh-a2");
    }

    #[test]
    fn rotated_refresh_token_survives_stale_runtime_reconciliation() {
        let old_generation = Utc.timestamp_opt(1_800_000_000, 0).single().unwrap();
        let rotated_generation = old_generation + Duration::seconds(1);
        let mut account = account("A", "workspace-a", "old");
        let account_id = account.id.clone();
        account.last_refresh_at = Some(old_generation);
        let mut store = AccountsStore {
            accounts: vec![account],
            active_account_id: Some(account_id.clone()),
            ..AccountsStore::default()
        };

        let updated = update_account_chatgpt_tokens_in_store(
            &mut store,
            &account_id,
            "id-rotated".into(),
            "access-rotated".into(),
            "refresh-rotated".into(),
            Some("workspace-a".into()),
            None,
            None,
            None,
            rotated_generation,
        )
        .unwrap();
        assert_eq!(refresh_token(&updated), "refresh-rotated");

        assert!(!sync_active_account_tokens(
            &mut store,
            &auth_with_refresh_at("workspace-a", "old", old_generation)
        ));
        assert_eq!(refresh_token(&store.accounts[0]), "refresh-rotated");
        assert_eq!(store.accounts[0].last_refresh_at, Some(rotated_generation));
    }

    #[test]
    fn newer_runtime_generation_projects_tokens_to_the_active_store() {
        let stored_generation = Utc.timestamp_opt(1_800_000_000, 0).single().unwrap();
        let runtime_generation = stored_generation + Duration::seconds(1);
        let mut account = account("A", "workspace-a", "old");
        let account_id = account.id.clone();
        account.last_refresh_at = Some(stored_generation);
        let mut store = AccountsStore {
            accounts: vec![account],
            active_account_id: Some(account_id),
            ..AccountsStore::default()
        };

        assert!(sync_active_account_tokens(
            &mut store,
            &auth_with_refresh_at("workspace-a", "runtime", runtime_generation)
        ));
        assert_eq!(refresh_token(&store.accounts[0]), "refresh-runtime");
        assert_eq!(store.accounts[0].last_refresh_at, Some(runtime_generation));
    }

    #[test]
    fn unversioned_runtime_snapshot_does_not_replace_known_store_credentials() {
        let stored_generation = Utc.timestamp_opt(1_800_000_000, 0).single().unwrap();
        let mut account = account("A", "workspace-a", "stored");
        let account_id = account.id.clone();
        account.last_refresh_at = Some(stored_generation);
        let mut store = AccountsStore {
            accounts: vec![account],
            active_account_id: Some(account_id),
            ..AccountsStore::default()
        };

        assert!(!sync_active_account_tokens(
            &mut store,
            &auth("workspace-a", "runtime")
        ));
        assert_eq!(refresh_token(&store.accounts[0]), "refresh-stored");
    }
}
