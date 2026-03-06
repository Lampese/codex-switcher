use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

pub struct FileLock {
    path: PathBuf,
}

impl FileLock {
    pub fn acquire(target: &Path) -> Result<Self> {
        let lock_path = sibling_with_suffix(target, ".lock");

        if let Some(parent) = lock_path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create lock directory: {}", parent.display())
            })?;
        }

        for _ in 0..100 {
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&lock_path)
            {
                Ok(mut file) => {
                    writeln!(file, "{}", std::process::id()).ok();
                    return Ok(Self { path: lock_path });
                }
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                    thread::sleep(Duration::from_millis(50));
                }
                Err(err) => {
                    return Err(err).with_context(|| {
                        format!("Failed to acquire lock: {}", lock_path.display())
                    });
                }
            }
        }

        anyhow::bail!("Timed out waiting for file lock: {}", lock_path.display());
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub fn sibling_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut file_name: OsString = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_else(|| OsString::from("file"));
    file_name.push(suffix);
    path.with_file_name(file_name)
}

pub fn write_bytes_atomic(path: &Path, bytes: &[u8], backup_existing: bool) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    if backup_existing && path.exists() {
        let backup_path = sibling_with_suffix(path, ".bak");
        fs::copy(path, &backup_path).with_context(|| {
            format!(
                "Failed to create backup {} from {}",
                backup_path.display(),
                path.display()
            )
        })?;
    }

    let temp_path = temp_path_for(path);
    {
        let mut file = File::create(&temp_path)
            .with_context(|| format!("Failed to create temp file: {}", temp_path.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("Failed to write temp file: {}", temp_path.display()))?;
        file.sync_all()
            .with_context(|| format!("Failed to sync temp file: {}", temp_path.display()))?;
    }

    fs::rename(&temp_path, path).with_context(|| {
        format!(
            "Failed to replace {} with {}",
            path.display(),
            temp_path.display()
        )
    })?;

    Ok(())
}

fn temp_path_for(path: &Path) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    sibling_with_suffix(path, &format!(".tmp-{}-{nanos}", std::process::id()))
}
