//! Atomic file write helpers — crash-safe writes with automatic backup.
//!
//! Adapted from cockpit-tools `atomic_write.rs`.
//! Key patterns:
//!   1. Write to temp file → `fs::rename()` (atomic on same filesystem)
//!   2. Auto-create `.bak` before overwriting existing files
//!   3. `parse_json_with_auto_restore()` — auto-recover from corrupt JSON

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde::de::DeserializeOwned;

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn build_backup_path(path: &Path) -> Result<PathBuf> {
    let parent = path.parent().context("Cannot determine parent directory")?;
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .context("Cannot determine file name")?;
    Ok(parent.join(format!("{file_name}.bak")))
}

fn build_temp_path(parent: &Path, target: &Path, suffix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let base = target
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file");
    parent.join(format!(
        ".{base}.tmp.{}.{nanos}.{suffix}",
        std::process::id()
    ))
}

fn is_json_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("json"))
        .unwrap_or(false)
}

/// Returns `true` if `content` looks valid enough to use as a backup source.
fn content_is_safe_backup(path: &Path, content: &str) -> bool {
    if content.trim().is_empty() || content.as_bytes().contains(&0) {
        return false;
    }
    if !is_json_path(path) {
        return true;
    }
    // For JSON files, verify the content actually parses
    serde_json::from_str::<serde_json::Value>(content).is_ok()
}

/// Write bytes to a temp file and `sync_all` before returning.
fn write_synced_temp(temp_path: &Path, content: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temp_path)
        .with_context(|| format!("Failed to create temp file: {}", temp_path.display()))?;
    file.write_all(content)
        .with_context(|| format!("Failed to write temp file: {}", temp_path.display()))?;
    file.sync_all()
        .with_context(|| format!("Failed to sync temp file: {}", temp_path.display()))?;
    Ok(())
}

fn write_atomic_internal(path: &Path, content: &str, create_backup: bool) -> Result<()> {
    let parent = path.parent().context("Cannot determine parent directory")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("Failed to create directory: {}", parent.display()))?;

    // Backup existing file (only if it's valid content)
    if create_backup && path.exists() {
        let backup_path = build_backup_path(path)?;
        if let Ok(existing) = fs::read_to_string(path) {
            if content_is_safe_backup(path, &existing) {
                // Write backup atomically too (but without creating a backup of the backup)
                write_atomic_internal(&backup_path, &existing, false)?;
            }
        }
    }

    // Write new content via temp → rename
    let temp_path = build_temp_path(parent, path, "atomic");
    if let Err(err) = write_synced_temp(&temp_path, content.as_bytes()) {
        let _ = fs::remove_file(&temp_path);
        return Err(err);
    }
    if let Err(err) = fs::rename(&temp_path, path) {
        let _ = fs::remove_file(&temp_path);
        bail!(
            "Failed to rename temp file to target: {} → {} ({})",
            temp_path.display(),
            path.display(),
            err
        );
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Atomically write a string to `path`, creating a `.bak` backup of the previous content.
pub fn write_string_atomic(path: &Path, content: &str) -> Result<()> {
    write_atomic_internal(path, content, true)
}

/// Restore the target file from its `.bak` backup.
/// Returns `Ok(true)` if restore succeeded, `Ok(false)` if no valid backup exists.
pub fn restore_from_backup(path: &Path) -> Result<bool> {
    let backup_path = build_backup_path(path)?;
    if !backup_path.exists() {
        return Ok(false);
    }

    let backup_content = fs::read_to_string(&backup_path)
        .with_context(|| format!("Failed to read backup: {}", backup_path.display()))?;
    if !content_is_safe_backup(path, &backup_content) {
        return Ok(false);
    }
    write_atomic_internal(path, &backup_content, false)?;
    Ok(true)
}

/// Parse JSON from `content`. If parsing fails and a `.bak` file exists,
/// automatically restore from backup and retry.
pub fn parse_json_with_auto_restore<T: DeserializeOwned>(path: &Path, content: &str) -> Result<T> {
    match serde_json::from_str::<T>(content) {
        Ok(value) => Ok(value),
        Err(parse_err) => {
            println!(
                "[AtomicWrite] JSON parse failed for {}: {parse_err}. Attempting backup restore...",
                path.display()
            );

            if restore_from_backup(path)? {
                let restored = fs::read_to_string(path)
                    .with_context(|| format!("Failed to read restored file: {}", path.display()))?;
                serde_json::from_str::<T>(&restored).with_context(|| {
                    format!(
                        "Original parse failed: {parse_err}; auto-restored from .bak but still failed"
                    )
                })
            } else {
                bail!(
                    "JSON parse failed for {} and no valid backup: {parse_err}",
                    path.display()
                )
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("{prefix}_{nanos}"));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn atomic_write_creates_backup() {
        let dir = make_temp_dir("aw_backup");
        let path = dir.join("data.json");
        let backup = build_backup_path(&path).unwrap();

        write_string_atomic(&path, r#"{"v":1}"#).unwrap();
        assert!(!backup.exists(), "no backup on first write");

        write_string_atomic(&path, r#"{"v":2}"#).unwrap();
        assert_eq!(fs::read_to_string(&backup).unwrap(), r#"{"v":1}"#);
        assert_eq!(fs::read_to_string(&path).unwrap(), r#"{"v":2}"#);
    }

    #[test]
    fn corrupt_file_preserves_good_backup() {
        let dir = make_temp_dir("aw_corrupt");
        let path = dir.join("data.json");
        let backup = build_backup_path(&path).unwrap();

        write_string_atomic(&path, r#"{"v":1}"#).unwrap();
        write_string_atomic(&path, r#"{"v":2}"#).unwrap();
        // backup should be v1
        assert_eq!(fs::read_to_string(&backup).unwrap(), r#"{"v":1}"#);

        // Corrupt the file with null bytes
        fs::write(&path, vec![0u8; 32]).unwrap();
        write_string_atomic(&path, r#"{"v":3}"#).unwrap();

        // Backup should still be v1 (corrupt file was not a safe backup source)
        assert_eq!(fs::read_to_string(&backup).unwrap(), r#"{"v":1}"#);
        assert_eq!(fs::read_to_string(&path).unwrap(), r#"{"v":3}"#);
    }

    #[test]
    fn restore_rejects_invalid_backup() {
        let dir = make_temp_dir("aw_restore");
        let path = dir.join("state.json");
        let backup = build_backup_path(&path).unwrap();

        fs::write(&path, r#"{"v":1}"#).unwrap();
        fs::write(&backup, vec![0u8; 16]).unwrap();

        assert!(!restore_from_backup(&path).unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), r#"{"v":1}"#);
    }

    #[test]
    fn parse_json_auto_restore_works() {
        let dir = make_temp_dir("aw_autorestore");
        let path = dir.join("cfg.json");

        // Write a valid first version, then a second (creating backup of first)
        write_string_atomic(&path, r#"{"version":1}"#).unwrap();
        write_string_atomic(&path, r#"{"version":2}"#).unwrap();

        // Now corrupt the current file
        fs::write(&path, "NOT VALID JSON!!!").unwrap();

        // parse_json_with_auto_restore should auto-restore v1 from backup
        let val: serde_json::Value =
            parse_json_with_auto_restore(&path, &fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(val["version"], 1);
    }
}
