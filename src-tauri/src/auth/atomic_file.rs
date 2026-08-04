use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Sensitivity categorization of target files for future ACL hardening.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileSensitivity {
    Metadata,
    Secret,
}

/// Errors returned by atomic write operations.
#[derive(Debug, thiserror::Error)]
pub(crate) enum AtomicWriteError {
    #[error("I/O error during atomic write: {0}")]
    Io(#[from] io::Error),

    #[error("Atomic file replacement failed with HRESULT 0x{0:08X}")]
    ReplaceFailed(u32),

    #[cfg(test)]
    #[error("Simulated failure at step: {0}")]
    SimulatedFailure(&'static str),

    #[error("Invalid target path: parent directory missing")]
    MissingParentDir,
}

/// Injected options for atomic file operations.
/// Crate-internal; not exposed in the public API.
#[derive(Debug, Clone, Default)]
pub(crate) struct AtomicWriteOptions {
    pub(crate) ensure_parent_dir: bool,
    #[cfg(test)]
    pub(crate) fail_point: FailPoint,
}

impl AtomicWriteOptions {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn with_ensure_parent(mut self, ensure: bool) -> Self {
        self.ensure_parent_dir = ensure;
        self
    }

    #[cfg(test)]
    pub(crate) fn with_fail_point(mut self, fail_point: FailPoint) -> Self {
        self.fail_point = fail_point;
        self
    }
}

/// Simulated failure injection points for deterministic test verification.
/// Exists only in test builds; never compiled into production.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum FailPoint {
    #[default]
    None,
    BeforeWrite,
    BeforeFlush,
    BeforeSync,
    BeforeReplace,
    SimulateReplaceFailure,
}

/// RAII Guard ensuring temporary file cleanup on failure.
struct TempFileGuard<'a> {
    path: &'a Path,
    active: bool,
}

impl<'a> TempFileGuard<'a> {
    fn new(path: &'a Path) -> Self {
        Self { path, active: true }
    }

    fn disarm(mut self) {
        self.active = false;
    }
}

impl<'a> Drop for TempFileGuard<'a> {
    fn drop(&mut self) {
        if self.active && self.path.exists() {
            let _ = std::fs::remove_file(self.path);
        }
    }
}

/// Performs an atomic write of `bytes` to `target`.
pub(crate) fn atomic_write(
    target: &Path,
    bytes: &[u8],
    sensitivity: FileSensitivity,
) -> Result<(), AtomicWriteError> {
    atomic_write_with_options(target, bytes, sensitivity, &AtomicWriteOptions::default())
}

/// Performs an atomic write with injected options/fail-points.
pub(crate) fn atomic_write_with_options(
    target: &Path,
    bytes: &[u8],
    _sensitivity: FileSensitivity,
    options: &AtomicWriteOptions,
) -> Result<(), AtomicWriteError> {
    let parent = target.parent().ok_or(AtomicWriteError::MissingParentDir)?;

    if options.ensure_parent_dir {
        std::fs::create_dir_all(parent)?;
    } else if !parent.exists() {
        return Err(AtomicWriteError::MissingParentDir);
    }

    let file_name_str = target
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("target");

    let mut nonce: u64 = 0;
    let mut temp_path: PathBuf;

    loop {
        let unique_suffix = format!("{}_{}_{}", std::process::id(), nonce, rand::random::<u32>());
        temp_path = parent.join(format!(".tmp_{}_{}", file_name_str, unique_suffix));
        nonce += 1;

        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
        {
            Ok(file) => {
                let guard = TempFileGuard::new(&temp_path);
                write_and_replace(file, &temp_path, target, bytes, options, guard)?;
                return Ok(());
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(AtomicWriteError::Io(e)),
        }
    }
}

fn write_and_replace(
    mut file: File,
    temp_path: &Path,
    target: &Path,
    bytes: &[u8],
    options: &AtomicWriteOptions,
    guard: TempFileGuard,
) -> Result<(), AtomicWriteError> {
    #[cfg(test)]
    if options.fail_point == FailPoint::BeforeWrite {
        return Err(AtomicWriteError::SimulatedFailure("BeforeWrite"));
    }

    file.write_all(bytes)?;

    #[cfg(test)]
    if options.fail_point == FailPoint::BeforeFlush {
        return Err(AtomicWriteError::SimulatedFailure("BeforeFlush"));
    }

    file.flush()?;

    #[cfg(test)]
    if options.fail_point == FailPoint::BeforeSync {
        return Err(AtomicWriteError::SimulatedFailure("BeforeSync"));
    }

    file.sync_all()?;

    // Explicitly drop file handle before replacement on Windows.
    drop(file);

    #[cfg(test)]
    if options.fail_point == FailPoint::BeforeReplace {
        return Err(AtomicWriteError::SimulatedFailure("BeforeReplace"));
    }

    #[cfg(test)]
    if options.fail_point == FailPoint::SimulateReplaceFailure {
        return Err(AtomicWriteError::SimulatedFailure("SimulateReplaceFailure"));
    }

    replace_file(temp_path, target)?;

    guard.disarm();
    Ok(())
}

#[cfg(windows)]
fn replace_file(temp_path: &Path, target_path: &Path) -> Result<(), AtomicWriteError> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let temp_wide: Vec<u16> = temp_path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let target_wide: Vec<u16> = target_path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let res = unsafe {
        MoveFileExW(
            PCWSTR(temp_wide.as_ptr()),
            PCWSTR(target_wide.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };

    if let Err(err) = res {
        let hresult_code = err.code().0 as u32;
        Err(AtomicWriteError::ReplaceFailed(hresult_code))
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn replace_file(temp_path: &Path, target_path: &Path) -> Result<(), AtomicWriteError> {
    std::fs::rename(temp_path, target_path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "codex_switcher_test_{}_{}",
            name,
            rand::random::<u32>()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_atomic_write_replacement_error_formatting_sanitized() {
        let err = AtomicWriteError::ReplaceFailed(0x80070005);
        let formatted = err.to_string();

        assert_eq!(
            formatted,
            "Atomic file replacement failed with HRESULT 0x80070005"
        );
        assert!(!formatted.contains("accounts.json"));
        assert!(!formatted.contains("vault.dat"));
    }

    #[test]
    fn test_atomic_write_new_file_succeeds() {
        let test_dir = create_test_dir("new_file");
        let target = test_dir.join("test.json");
        let data = b"{\"hello\":\"world\"}";

        atomic_write(&target, data, FileSensitivity::Metadata).expect("Write failed");

        assert!(target.exists());
        let read_back = std::fs::read(&target).unwrap();
        assert_eq!(read_back, data);

        let _ = std::fs::remove_dir_all(test_dir);
    }

    #[test]
    fn test_atomic_write_replaces_existing_file_succeeds() {
        let test_dir = create_test_dir("replace_existing");
        let target = test_dir.join("data.dat");

        std::fs::write(&target, b"old content").unwrap();
        let new_data = b"new atomic content";

        atomic_write(&target, new_data, FileSensitivity::Secret).expect("Replace failed");

        let read_back = std::fs::read(&target).unwrap();
        assert_eq!(read_back, new_data);

        let _ = std::fs::remove_dir_all(test_dir);
    }

    #[test]
    fn test_atomic_write_final_contents_exact() {
        let test_dir = create_test_dir("exact_contents");
        let target = test_dir.join("exact.txt");
        let payload = vec![0x00, 0xFF, 0x42, 0x13, 0x37];

        atomic_write(&target, &payload, FileSensitivity::Metadata).unwrap();

        assert_eq!(std::fs::read(&target).unwrap(), payload);
        let _ = std::fs::remove_dir_all(test_dir);
    }

    #[test]
    fn test_atomic_write_simulated_failure_before_replace_preserves_old_contents() {
        let test_dir = create_test_dir("fail_before_replace");
        let target = test_dir.join("preserve.json");
        let initial_data = b"original content";

        std::fs::write(&target, initial_data).unwrap();

        let options = AtomicWriteOptions::new().with_fail_point(FailPoint::BeforeReplace);
        let res =
            atomic_write_with_options(&target, b"new content", FileSensitivity::Metadata, &options);

        assert!(res.is_err());
        assert_eq!(std::fs::read(&target).unwrap(), initial_data);

        let _ = std::fs::remove_dir_all(test_dir);
    }

    #[test]
    fn test_atomic_write_simulated_replacement_failure_preserves_old_contents() {
        let test_dir = create_test_dir("fail_replace");
        let target = test_dir.join("preserve_replace.json");
        let initial_data = b"original data";

        std::fs::write(&target, initial_data).unwrap();

        let options = AtomicWriteOptions::new().with_fail_point(FailPoint::SimulateReplaceFailure);
        let res =
            atomic_write_with_options(&target, b"new data", FileSensitivity::Metadata, &options);

        assert!(res.is_err());
        assert_eq!(std::fs::read(&target).unwrap(), initial_data);

        let _ = std::fs::remove_dir_all(test_dir);
    }

    #[test]
    fn test_atomic_write_handled_failure_removes_temp_file() {
        let test_dir = create_test_dir("clean_temp");
        let target = test_dir.join("clean.json");

        let options = AtomicWriteOptions::new().with_fail_point(FailPoint::BeforeReplace);
        let _ = atomic_write_with_options(&target, b"test", FileSensitivity::Metadata, &options);

        let temp_files: Vec<_> = std::fs::read_dir(&test_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(".tmp_"))
            .collect();

        assert!(
            temp_files.is_empty(),
            "Temporary file was not cleaned up on failure"
        );

        let _ = std::fs::remove_dir_all(test_dir);
    }

    #[test]
    fn test_atomic_write_successful_operation_removes_temp_file() {
        let test_dir = create_test_dir("success_temp");
        let target = test_dir.join("success.json");

        atomic_write(&target, b"data", FileSensitivity::Metadata).unwrap();

        let entries: Vec<_> = std::fs::read_dir(&test_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(".tmp_"))
            .collect();

        assert!(
            entries.is_empty(),
            "Temporary file remained after successful write"
        );

        let _ = std::fs::remove_dir_all(test_dir);
    }

    #[test]
    fn test_atomic_write_empty_payload_creates_valid_zero_length_file() {
        let test_dir = create_test_dir("empty_payload");
        let target = test_dir.join("empty.json");

        atomic_write(&target, b"", FileSensitivity::Metadata).unwrap();

        assert!(target.exists());
        let meta = std::fs::metadata(&target).unwrap();
        assert_eq!(meta.len(), 0);

        let _ = std::fs::remove_dir_all(test_dir);
    }

    #[test]
    fn test_atomic_write_sibling_unrelated_files_untouched() {
        let test_dir = create_test_dir("sibling_test");
        let sibling = test_dir.join("sibling.txt");
        let target = test_dir.join("target.txt");

        std::fs::write(&sibling, b"sibling_data").unwrap();
        atomic_write(&target, b"target_data", FileSensitivity::Metadata).unwrap();

        assert_eq!(std::fs::read(&sibling).unwrap(), b"sibling_data");
        assert_eq!(std::fs::read(&target).unwrap(), b"target_data");

        let _ = std::fs::remove_dir_all(test_dir);
    }
}
