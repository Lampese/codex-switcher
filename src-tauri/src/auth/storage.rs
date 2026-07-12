//! Account storage module - manages reading and writing accounts.json
//!
//! Concurrency model: many concurrent readers exist (the tray's 1s file
//! watcher, the 60s usage poller, Tauri commands, and a separate `codex-web`
//! process), so every write must be atomic (`write_file_atomically`), and
//! every read-modify-write cycle must run under the exclusive store lock
//! (`mutate_accounts`) or concurrent writers silently overwrite each other's
//! changes.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::types::{AccountsStore, AppSettings, StoredAccount};

const ACCOUNTS_FILE: &str = "accounts.json";
const SETTINGS_FILE: &str = "settings.json";
const STORE_LOCK_FILE: &str = ".store.lock";

/// Get the path to the codex-switcher config directory
pub fn get_config_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not find home directory")?;
    Ok(home.join(".codex-switcher"))
}

/// Get the path to accounts.json
pub fn get_accounts_file() -> Result<PathBuf> {
    Ok(get_config_dir()?.join(ACCOUNTS_FILE))
}

pub fn get_settings_file() -> Result<PathBuf> {
    Ok(get_config_dir()?.join(SETTINGS_FILE))
}

/// Acquire the exclusive advisory lock that serializes every store write,
/// both across threads in this process and across processes (the desktop app
/// and the `codex-web` LAN server share the same files). Blocks until the
/// lock is available; the OS releases it automatically when the returned
/// `File` drops, including when the holding process crashes.
fn acquire_store_lock(dir: &Path) -> Result<File> {
    fs::create_dir_all(dir)
        .with_context(|| format!("Failed to create config directory: {}", dir.display()))?;

    let lock_path = dir.join(STORE_LOCK_FILE);
    let lock_file = OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(&lock_path)
        .with_context(|| format!("Failed to open store lock file: {}", lock_path.display()))?;

    lock_file
        .lock()
        .with_context(|| format!("Failed to lock store: {}", lock_path.display()))?;

    Ok(lock_file)
}

/// Load the accounts store from disk. Lock-free: atomic writes guarantee a
/// reader always sees a complete file.
pub fn load_accounts() -> Result<AccountsStore> {
    load_accounts_in(&get_config_dir()?)
}

fn load_accounts_in(dir: &Path) -> Result<AccountsStore> {
    let path = dir.join(ACCOUNTS_FILE);

    if !path.exists() {
        return Ok(AccountsStore::default());
    }

    let content = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read accounts file: {}", path.display()))?;

    // An empty file carries no data worth preserving; treat it like a missing
    // file so a past crash can't brick every launch, but keep failing loudly
    // on non-empty garbage — overwriting that would destroy real accounts.
    if content.trim().is_empty() {
        return Ok(AccountsStore::default());
    }

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

    if content.trim().is_empty() {
        return Ok(AppSettings::default());
    }

    let settings: AppSettings = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse settings file: {}", path.display()))?;

    Ok(settings)
}

/// Persist app settings. Settings load-modify-save cycles all run on the main
/// thread (menu and Dock handlers), so only the write itself takes the store
/// lock — enough to stay safe against writers in other processes.
pub fn save_app_settings(settings: &AppSettings) -> Result<()> {
    let dir = get_config_dir()?;
    let _lock = acquire_store_lock(&dir)?;

    let content = serde_json::to_string_pretty(settings).context("Failed to serialize settings")?;
    write_file_atomically(&dir.join(SETTINGS_FILE), &content)
        .with_context(|| format!("Failed to write settings file: {}", dir.display()))
}

/// Run a read-modify-write cycle on the accounts store under the exclusive
/// store lock: load, apply `mutate`, save. If `mutate` returns an error,
/// nothing is written.
///
/// The closure must not call any other locking storage function
/// (`mutate_accounts`, `save_app_settings`, `add_account`, ...) — the lock is
/// not re-entrant and doing so deadlocks. Lock-free helpers like
/// `switch_to_account` and plain access to `store` are fine.
pub fn mutate_accounts<T>(mutate: impl FnOnce(&mut AccountsStore) -> Result<T>) -> Result<T> {
    mutate_accounts_in(&get_config_dir()?, mutate)
}

fn mutate_accounts_in<T>(
    dir: &Path,
    mutate: impl FnOnce(&mut AccountsStore) -> Result<T>,
) -> Result<T> {
    let _lock = acquire_store_lock(dir)?;

    let mut store = load_accounts_in(dir)?;
    let value = mutate(&mut store)?;

    let content =
        serde_json::to_string_pretty(&store).context("Failed to serialize accounts store")?;
    write_file_atomically(&dir.join(ACCOUNTS_FILE), &content)
        .with_context(|| format!("Failed to write accounts file: {}", dir.display()))?;

    Ok(value)
}

/// Write `content` to `path` by writing a sibling temp file and atomically
/// renaming it into place, instead of writing directly to `path`.
///
/// `fs::write` truncates the target before writing the new bytes, so any
/// concurrent reader (the tray's 1s file watcher, the 60s usage poller, a
/// separate `codex-web` LAN process, or the official `codex` CLI reading
/// `auth.json`) that opens the file during that window observes a truncated
/// or empty file. A same-directory `rename` is atomic, so readers always see
/// either the fully-old or fully-new content. The temp file is created with
/// 0600 from the first byte (these files carry credentials) and fsynced
/// before the rename so a crash can't leave the target pointing at data that
/// never reached disk.
///
/// Callers are expected to hold the store lock; the stale-temp sweep assumes
/// no other writer of ours is mid-flight on the same target.
pub(crate) fn write_file_atomically(path: &Path, content: &str) -> Result<()> {
    sweep_stale_tmp_files(path);

    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("tmp");
    let tmp_path = path.with_extension(format!("{extension}.tmp.{}", Uuid::new_v4()));

    let result = write_temp_then_rename(&tmp_path, path, content);
    if result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    result
}

/// Best-effort removal of `<file>.tmp.<uuid>` leftovers from writes that
/// crashed between creating the temp file and renaming it into place.
fn sweep_stale_tmp_files(path: &Path) {
    let Some(parent) = path.parent() else { return };
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return;
    };
    let prefix = format!("{name}.tmp.");

    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        if entry
            .file_name()
            .to_str()
            .is_some_and(|n| n.starts_with(&prefix))
        {
            let _ = fs::remove_file(entry.path());
        }
    }
}

fn write_temp_then_rename(tmp_path: &Path, path: &Path, content: &str) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let mut file = options
        .open(tmp_path)
        .with_context(|| format!("Failed to create temp file: {}", tmp_path.display()))?;
    file.write_all(content.as_bytes())
        .with_context(|| format!("Failed to write temp file: {}", tmp_path.display()))?;
    file.sync_all()
        .with_context(|| format!("Failed to flush temp file: {}", tmp_path.display()))?;
    drop(file);

    rename_replacing(tmp_path, path)
        .with_context(|| format!("Failed to finalize file: {}", path.display()))?;

    Ok(())
}

#[cfg(not(windows))]
fn rename_replacing(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::rename(from, to)
}

/// On Windows, replacing a file that another process holds open without
/// FILE_SHARE_DELETE (antivirus scanners, non-Rust readers) fails with a
/// sharing violation that clears once the reader closes — retry briefly.
#[cfg(windows)]
fn rename_replacing(from: &Path, to: &Path) -> std::io::Result<()> {
    const ATTEMPTS: u32 = 5;
    let mut last_error = None;
    for attempt in 0..ATTEMPTS {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.expect("rename attempted at least once"))
}

/// Add a new account to the store
pub fn add_account(account: StoredAccount) -> Result<StoredAccount> {
    mutate_accounts(|store| {
        if store.accounts.iter().any(|a| a.name == account.name) {
            anyhow::bail!("An account with name '{}' already exists", account.name);
        }

        let account_clone = account.clone();
        store.accounts.push(account);

        // If this is the first account, make it active
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

        // If we removed the active account, clear it or set to first available
        if store.active_account_id.as_deref() == Some(account_id) {
            store.active_account_id = store.accounts.first().map(|a| a.id.clone());
        }

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

/// Update an account's metadata (name, email, plan_type, subscription expiry)
pub fn update_account_metadata(
    account_id: &str,
    name: Option<String>,
    email: Option<String>,
    plan_type: Option<String>,
    subscription_expires_at: Option<Option<DateTime<Utc>>>,
) -> Result<StoredAccount> {
    mutate_accounts(|store| {
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
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    fn temp_store_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("codex-switcher-test-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Reproduces the "Failed to parse accounts file" bug: a reader (e.g. the
    /// tray's 1s file watcher) can open the file while a writer is mid-`fs::write`,
    /// since plain `fs::write` truncates before the new bytes land.
    #[test]
    fn concurrent_readers_never_observe_a_torn_write() {
        let dir = temp_store_dir();
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

    /// Reproduces the lost-update bug: unlocked load-modify-save cycles (e.g.
    /// the 10-way-concurrent metadata updates in `refresh_all_accounts_usage`)
    /// silently drop each other's changes.
    #[test]
    fn concurrent_mutations_never_lose_updates() {
        let dir = temp_store_dir();
        const THREADS: usize = 8;
        const ITERATIONS: usize = 25;

        let handles: Vec<_> = (0..THREADS)
            .map(|thread| {
                let dir = dir.clone();
                std::thread::spawn(move || {
                    for i in 0..ITERATIONS {
                        mutate_accounts_in(&dir, |store| {
                            let account = StoredAccount::new_api_key(
                                format!("account-{thread}-{i}"),
                                "dummy-api-key".to_string(),
                            );
                            store.accounts.push(account);
                            Ok(())
                        })
                        .unwrap();
                    }
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        let store = load_accounts_in(&dir).unwrap();
        assert_eq!(
            store.accounts.len(),
            THREADS * ITERATIONS,
            "lost updates: concurrent mutations overwrote each other"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn treats_empty_accounts_file_as_missing() {
        let dir = temp_store_dir();
        fs::write(dir.join("accounts.json"), "").unwrap();

        let store = load_accounts_in(&dir).unwrap();
        assert!(store.accounts.is_empty());
        assert_eq!(store.version, 1);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_corrupt_accounts_file_instead_of_wiping_it() {
        let dir = temp_store_dir();
        fs::write(dir.join("accounts.json"), "{\"version\":1,").unwrap();

        assert!(load_accounts_in(&dir).is_err());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn failed_mutation_leaves_store_untouched() {
        let dir = temp_store_dir();
        mutate_accounts_in(&dir, |store| {
            store.accounts.push(StoredAccount::new_api_key(
                "keep".into(),
                "dummy-api-key".into(),
            ));
            Ok(())
        })
        .unwrap();

        let result: Result<()> = mutate_accounts_in(&dir, |store| {
            store.accounts.clear();
            anyhow::bail!("boom")
        });
        assert!(result.is_err());

        let store = load_accounts_in(&dir).unwrap();
        assert_eq!(store.accounts.len(), 1, "failed mutation must not be saved");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn sweeps_stale_tmp_files_and_leaves_none_behind() {
        let dir = temp_store_dir();
        let path = dir.join("accounts.json");

        fs::write(dir.join("accounts.json.tmp.stale-crash-leftover"), "junk").unwrap();
        fs::write(dir.join("unrelated.txt"), "keep me").unwrap();

        write_file_atomically(&path, "{\"ok\":true}").unwrap();

        let names: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();

        assert!(names.contains(&"accounts.json".to_string()));
        assert!(names.contains(&"unrelated.txt".to_string()));
        assert!(
            !names.iter().any(|n| n.contains(".tmp.")),
            "stale or leftover temp files remain: {names:?}"
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
