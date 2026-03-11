use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
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
                    maybe_clear_stale_lock(&lock_path);
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

fn maybe_clear_stale_lock(lock_path: &Path) {
    let Ok(Some(pid)) = read_lock_pid(lock_path) else {
        return;
    };

    if !process_is_alive(pid) {
        let _ = fs::remove_file(lock_path);
    }
}

fn read_lock_pid(lock_path: &Path) -> Result<Option<u32>> {
    let mut content = String::new();
    File::open(lock_path)
        .with_context(|| format!("Failed to open lock file: {}", lock_path.display()))?
        .read_to_string(&mut content)
        .with_context(|| format!("Failed to read lock file: {}", lock_path.display()))?;

    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    let pid = trimmed
        .parse::<u32>()
        .with_context(|| format!("Invalid PID in lock file: {}", lock_path.display()))?;
    Ok(Some(pid))
}

#[cfg(unix)]
fn process_is_alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(windows)]
fn process_is_alive(pid: u32) -> bool {
    Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}")])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map(|output| {
            let stdout = String::from_utf8_lossy(&output.stdout);
            stdout.lines().any(|line| line.contains(&pid.to_string()))
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "codex-switcher-fs-utils-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn acquire_reclaims_stale_lock_file() {
        let dir = test_dir();
        let target = dir.join("auth.json");
        let lock_path = sibling_with_suffix(&target, ".lock");
        fs::write(&lock_path, format!("{}\n", u32::MAX)).unwrap();

        let lock = FileLock::acquire(&target).expect("stale lock should be reclaimed");
        let content = fs::read_to_string(&lock_path).unwrap();

        assert_eq!(content.trim(), std::process::id().to_string());

        drop(lock);
        assert!(!lock_path.exists());
        fs::remove_dir_all(dir).unwrap();
    }
}
