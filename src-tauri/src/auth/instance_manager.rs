//! Multi-instance manager — isolated Codex config directories with shared symlinks.
//!
//! Inspired by cockpit-tools `codex_instance.rs`.
//! Each instance gets its own `user_data_dir` (a copy of `~/.codex/`),
//! with `skills/` and `rules/` symlinked to a shared global directory.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::atomic_write::{parse_json_with_auto_restore, write_string_atomic};
use super::switcher::get_codex_home;

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// A Codex instance profile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceProfile {
    pub id: String,
    pub name: String,
    /// Absolute path to this instance's data directory.
    pub user_data_dir: String,
    /// Which account ID to bind to this instance (optional).
    pub bind_account_id: Option<String>,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
}

/// Store for all instances.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InstanceStore {
    pub instances: Vec<InstanceProfile>,
    pub active_instance_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Shared resource directories to symlink
// ---------------------------------------------------------------------------

const SHARED_DIRS: &[&str] = &["skills", "rules"];

// ---------------------------------------------------------------------------
// Storage paths
// ---------------------------------------------------------------------------

fn get_instances_file() -> Result<PathBuf> {
    let config_dir = super::storage::get_config_dir()?;
    Ok(config_dir.join("codex_instances.json"))
}

pub fn load_instances() -> Result<InstanceStore> {
    let path = get_instances_file()?;
    if !path.exists() {
        return Ok(InstanceStore::default());
    }
    let content = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read instances file: {}", path.display()))?;
    parse_json_with_auto_restore(&path, &content)
        .with_context(|| format!("Failed to parse instances file: {}", path.display()))
}

fn save_instances(store: &InstanceStore) -> Result<()> {
    let path = get_instances_file()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(store)?;
    write_string_atomic(&path, &content)
        .with_context(|| format!("Failed to write instances file: {}", path.display()))
}

// ---------------------------------------------------------------------------
// CRUD operations
// ---------------------------------------------------------------------------

/// Create a new isolated Codex instance by copying `~/.codex/` to a new directory.
pub fn create_instance(name: String, user_data_dir: String) -> Result<InstanceProfile> {
    let name = name.trim().to_string();
    if name.is_empty() {
        bail!("Instance name cannot be empty");
    }
    let user_data_dir = user_data_dir.trim().to_string();
    if user_data_dir.is_empty() {
        bail!("Instance directory cannot be empty");
    }

    let mut store = load_instances()?;
    ensure_unique(&store, &name, &user_data_dir, None)?;

    let target_path = PathBuf::from(&user_data_dir);
    let codex_home = get_codex_home()?;

    // Copy default codex home to the new instance directory
    if codex_home.exists() {
        copy_dir_recursive(&codex_home, &target_path)?;
    } else {
        fs::create_dir_all(&target_path)
            .with_context(|| format!("Failed to create instance dir: {}", target_path.display()))?;
    }

    // Set up shared symlinks
    setup_shared_symlinks(&target_path, &codex_home)?;

    let instance = InstanceProfile {
        id: Uuid::new_v4().to_string(),
        name,
        user_data_dir,
        bind_account_id: None,
        created_at: Utc::now().timestamp_millis(),
        last_used_at: None,
    };

    store.instances.push(instance.clone());
    save_instances(&store)?;
    Ok(instance)
}

/// Create a new empty instance (no copying from default).
pub fn create_empty_instance(name: String, user_data_dir: String) -> Result<InstanceProfile> {
    let name = name.trim().to_string();
    if name.is_empty() {
        bail!("Instance name cannot be empty");
    }
    let user_data_dir = user_data_dir.trim().to_string();
    if user_data_dir.is_empty() {
        bail!("Instance directory cannot be empty");
    }

    let mut store = load_instances()?;
    ensure_unique(&store, &name, &user_data_dir, None)?;

    let target_path = PathBuf::from(&user_data_dir);
    fs::create_dir_all(&target_path)
        .with_context(|| format!("Failed to create instance dir: {}", target_path.display()))?;

    let codex_home = get_codex_home()?;
    setup_shared_symlinks(&target_path, &codex_home)?;

    let instance = InstanceProfile {
        id: Uuid::new_v4().to_string(),
        name,
        user_data_dir,
        bind_account_id: None,
        created_at: Utc::now().timestamp_millis(),
        last_used_at: None,
    };

    store.instances.push(instance.clone());
    save_instances(&store)?;
    Ok(instance)
}

/// List all instances.
pub fn list_instances() -> Result<Vec<InstanceProfile>> {
    let store = load_instances()?;
    Ok(store.instances)
}

/// Get the currently active instance.
pub fn get_active_instance() -> Result<Option<InstanceProfile>> {
    let store = load_instances()?;
    let active_id = match &store.active_instance_id {
        Some(id) => id,
        None => return Ok(None),
    };
    Ok(store.instances.into_iter().find(|i| i.id == *active_id))
}

/// Set the active instance and update CODEX_HOME accordingly.
pub fn set_active_instance(instance_id: &str) -> Result<InstanceProfile> {
    let mut store = load_instances()?;
    let instance = store
        .instances
        .iter()
        .find(|i| i.id == instance_id)
        .context("Instance not found")?
        .clone();

    store.active_instance_id = Some(instance_id.to_string());

    // Touch last_used_at
    if let Some(inst) = store.instances.iter_mut().find(|i| i.id == instance_id) {
        inst.last_used_at = Some(Utc::now().timestamp_millis());
    }

    save_instances(&store)?;

    // Set CODEX_HOME environment variable for this process
    std::env::set_var("CODEX_HOME", &instance.user_data_dir);
    println!(
        "[InstanceManager] Set CODEX_HOME to: {}",
        instance.user_data_dir
    );

    Ok(instance)
}

/// Remove an instance (optionally delete its data directory).
pub fn remove_instance(instance_id: &str, delete_data: bool) -> Result<()> {
    let mut store = load_instances()?;
    let instance = store
        .instances
        .iter()
        .find(|i| i.id == instance_id)
        .context("Instance not found")?
        .clone();

    store.instances.retain(|i| i.id != instance_id);

    // Clear active if we removed it
    if store.active_instance_id.as_deref() == Some(instance_id) {
        store.active_instance_id = store.instances.first().map(|i| i.id.clone());
    }

    save_instances(&store)?;

    if delete_data {
        let path = PathBuf::from(&instance.user_data_dir);
        if path.exists() {
            fs::remove_dir_all(&path).with_context(|| {
                format!("Failed to delete instance data: {}", path.display())
            })?;
        }
    }

    Ok(())
}

/// Bind an account to an instance.
pub fn bind_account(instance_id: &str, account_id: Option<String>) -> Result<InstanceProfile> {
    let mut store = load_instances()?;
    let instance = store
        .instances
        .iter_mut()
        .find(|i| i.id == instance_id)
        .context("Instance not found")?;

    instance.bind_account_id = account_id;
    let updated = instance.clone();
    save_instances(&store)?;
    Ok(updated)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn ensure_unique(
    store: &InstanceStore,
    name: &str,
    user_data_dir: &str,
    exclude_id: Option<&str>,
) -> Result<()> {
    let mut names = HashSet::new();
    let mut dirs = HashSet::new();
    for inst in &store.instances {
        if exclude_id == Some(inst.id.as_str()) {
            continue;
        }
        names.insert(inst.name.to_lowercase());
        dirs.insert(inst.user_data_dir.to_lowercase());
    }
    if names.contains(&name.to_lowercase()) {
        bail!("An instance with name '{}' already exists", name);
    }
    if dirs.contains(&user_data_dir.to_lowercase()) {
        bail!("An instance with directory '{}' already exists", user_data_dir);
    }
    Ok(())
}

/// Create symlinks for shared directories (`skills/`, `rules/`).
fn setup_shared_symlinks(instance_dir: &Path, default_codex_home: &Path) -> Result<()> {
    for dir_name in SHARED_DIRS {
        let global_dir = default_codex_home.join(dir_name);
        let instance_link = instance_dir.join(dir_name);

        // Ensure the global shared directory exists
        fs::create_dir_all(&global_dir).with_context(|| {
            format!("Failed to create shared directory: {}", global_dir.display())
        })?;

        // Remove the copied directory if it exists (we'll replace with symlink)
        if instance_link.exists() {
            let meta = fs::symlink_metadata(&instance_link)?;
            if meta.is_dir() && !meta.file_type().is_symlink() {
                // Real directory (from copying) — remove it
                fs::remove_dir_all(&instance_link).with_context(|| {
                    format!("Failed to remove copied dir: {}", instance_link.display())
                })?;
            } else if meta.file_type().is_symlink() {
                // Already a symlink — skip
                continue;
            }
        }

        // Create the symlink
        create_directory_symlink(&global_dir, &instance_link)?;
    }
    Ok(())
}

/// Create a directory symlink (platform-specific).
fn create_directory_symlink(target: &Path, link: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        // On Windows, try junction first (no admin rights needed), fall back to symlink
        if junction::create(target, link).is_ok() {
            println!(
                "[InstanceManager] Created junction: {} -> {}",
                link.display(),
                target.display()
            );
            return Ok(());
        }
        // Fallback: directory symlink (may need admin/developer mode)
        std::os::windows::fs::symlink_dir(target, link).with_context(|| {
            format!(
                "Failed to create symlink: {} -> {}. Try enabling Developer Mode in Windows Settings.",
                link.display(),
                target.display()
            )
        })?;
    }

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link).with_context(|| {
            format!(
                "Failed to create symlink: {} -> {}",
                link.display(),
                target.display()
            )
        })?;
    }

    println!(
        "[InstanceManager] Created symlink: {} -> {}",
        link.display(),
        target.display()
    );
    Ok(())
}

/// Recursively copy a directory.
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    if !src.exists() {
        bail!("Source directory does not exist: {}", src.display());
    }

    fs::create_dir_all(dst)
        .with_context(|| format!("Failed to create target directory: {}", dst.display()))?;

    for entry in fs::read_dir(src)
        .with_context(|| format!("Failed to read source directory: {}", src.display()))?
    {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let target = dst.join(entry.file_name());

        if file_type.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
        } else if file_type.is_file() {
            fs::copy(entry.path(), &target).with_context(|| {
                format!("Failed to copy file: {}", entry.path().display())
            })?;
        }
        // Skip symlinks during copy — they'll be recreated
    }

    Ok(())
}
