//! Global mutation lock primitive — Phase 1B-2.
//!
//! Serializes secret-mutating operations using a process-local async mutex
//! combined with a cross-process exclusive file lock (fs2).

use std::fs::{File, OpenOptions};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use tokio::sync::{Mutex, OwnedMutexGuard};

use crate::auth::paths::AppPaths;

static PROCESS_MUTEX: OnceLock<Arc<Mutex<()>>> = OnceLock::new();

fn get_process_mutex() -> Arc<Mutex<()>> {
    PROCESS_MUTEX
        .get_or_init(|| Arc::new(Mutex::new(())))
        .clone()
}

/// Errors returned by operation lock acquisition.
#[derive(Debug, thiserror::Error)]
pub(crate) enum MutationLockError {
    #[error("Failed to create operation lock parent directory")]
    ParentDirectoryCreateFailed,

    #[error("Failed to open operation lock file")]
    LockFileOpenFailed,

    #[error("Failed to acquire cross-process exclusive lock")]
    CrossProcessLockFailed,

    #[error("Blocking lock task failed to complete")]
    BlockingTaskFailed,
}

/// RAII Guard holding process-local async mutex and cross-process file lock.
pub(crate) struct MutationGuard {
    _mutex_guard: OwnedMutexGuard<()>,
    file: Option<File>,
}

impl Drop for MutationGuard {
    fn drop(&mut self) {
        if let Some(file) = self.file.take() {
            let _ = file.unlock();
            drop(file);
        }
        // _mutex_guard drops next, releasing process-local async mutex.
    }
}

pub(crate) struct MutationLock {
    path: PathBuf,
}

impl MutationLock {
    pub(crate) fn from_paths(paths: &AppPaths) -> Self {
        Self {
            path: paths.operation_lock_file.clone(),
        }
    }

    pub(crate) fn for_path(path: PathBuf) -> Self {
        Self { path }
    }

    pub(crate) async fn acquire(&self) -> Result<MutationGuard, MutationLockError> {
        // Step 1: Acquire process-local async mutex guard.
        let mutex = get_process_mutex();
        let mutex_guard = mutex.lock_owned().await;

        // Step 2: Ensure parent directory exists.
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|_| MutationLockError::ParentDirectoryCreateFailed)?;
        }

        // Step 3: Open operation.lock file.
        let lock_path = self.path.clone();

        // Step 4: Perform blocking lock acquisition on spawn_blocking.
        let lock_result =
            tokio::task::spawn_blocking(move || -> Result<File, MutationLockError> {
                let file = OpenOptions::new()
                    .create(true)
                    .read(true)
                    .write(true)
                    .open(&lock_path)
                    .map_err(|_| MutationLockError::LockFileOpenFailed)?;

                use fs2::FileExt;
                file.lock_exclusive()
                    .map_err(|_| MutationLockError::CrossProcessLockFailed)?;

                Ok(file)
            })
            .await;

        match lock_result {
            Ok(Ok(file)) => Ok(MutationGuard {
                _mutex_guard: mutex_guard,
                file: Some(file),
            }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(MutationLockError::BlockingTaskFailed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir(tag: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("codex_lock_test_{}_{}", tag, rand::random::<u32>()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[tokio::test]
    async fn test_lock_first_guard_acquires_successfully() {
        let d = test_dir("first_acquire");
        let lock = MutationLock::for_path(d.join("operation.lock"));

        let guard = lock.acquire().await;
        assert!(guard.is_ok());

        drop(guard);
        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_lock_second_acquisition_waits_and_completes_after_drop() {
        let d = test_dir("wait_drop");
        let lock = MutationLock::for_path(d.join("operation.lock"));

        let guard1 = lock.acquire().await.unwrap();

        let lock_clone = MutationLock::for_path(d.join("operation.lock"));
        let (tx, mut rx) = tokio::sync::oneshot::channel();

        tokio::spawn(async move {
            let guard2 = lock_clone.acquire().await.unwrap();
            drop(guard2);
            let _ = tx.send(());
        });

        // Ensure guard2 is waiting
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(matches!(
            rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));

        drop(guard1);

        // Now guard2 acquires and completes
        let res = tokio::time::timeout(std::time::Duration::from_secs(2), rx).await;
        assert!(res.is_ok());

        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_lock_releases_after_returned_operation_error() {
        let d = test_dir("error_release");
        let lock = MutationLock::for_path(d.join("operation.lock"));

        let res: Result<(), &str> = async {
            let _guard = lock.acquire().await.map_err(|_| "lock_failed")?;
            Err("operation_failed")
        }
        .await;

        assert_eq!(res, Err("operation_failed"));

        // Should be able to acquire again immediately
        let guard2 = lock.acquire().await;
        assert!(guard2.is_ok());

        drop(guard2);
        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_lock_two_instances_same_path_serialize() {
        let d = test_dir("two_instances");
        let lock_file = d.join("operation.lock");

        let lock1 = MutationLock::for_path(lock_file.clone());
        let lock2 = MutationLock::for_path(lock_file);

        let g1 = lock1.acquire().await.unwrap();

        let (tx, mut rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let g2 = lock2.acquire().await.unwrap();
            drop(g2);
            let _ = tx.send(());
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(matches!(
            rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));

        drop(g1);

        let res = tokio::time::timeout(std::time::Duration::from_secs(2), rx).await;
        assert!(res.is_ok());

        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_lock_different_paths_acquire_sequentially() {
        let d = test_dir("different_paths");
        let lock1 = MutationLock::for_path(d.join("op1.lock"));
        let lock2 = MutationLock::for_path(d.join("op2.lock"));

        // This is intentionally a process-local sequential check; it does not claim
        // separate-process concurrency. Each MutationLock uses its own file path.
        let g1 = lock1.acquire().await;
        assert!(g1.is_ok());
        drop(g1);

        let g2 = lock2.acquire().await;
        assert!(g2.is_ok());

        drop(g2);
        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_lock_parent_directory_created_explicitly() {
        let d = test_dir("parent_create");
        let sub_dir = d.join("nested").join("deep");
        let lock_path = sub_dir.join("operation.lock");

        let lock = MutationLock::for_path(lock_path.clone());
        let g = lock.acquire().await;
        assert!(g.is_ok());
        assert!(sub_dir.exists());

        drop(g);
        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_lock_operation_lock_file_remains_after_release() {
        let d = test_dir("file_remains");
        let lock_path = d.join("operation.lock");
        let lock = MutationLock::for_path(lock_path.clone());

        let g = lock.acquire().await.unwrap();
        drop(g);

        assert!(
            lock_path.exists(),
            "operation.lock must remain after release"
        );

        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_lock_error_text_contains_no_synthetic_credentials() {
        let secrets = [
            "synthetic-id-token-A",
            "synthetic-access-token-A",
            "synthetic-refresh-token-A",
            "synthetic-api-key-A",
        ];
        let errors: &[&dyn std::fmt::Display] = &[
            &MutationLockError::ParentDirectoryCreateFailed,
            &MutationLockError::LockFileOpenFailed,
            &MutationLockError::CrossProcessLockFailed,
            &MutationLockError::BlockingTaskFailed,
        ];
        for err in errors {
            let msg = err.to_string();
            for secret in &secrets {
                assert!(!msg.contains(secret));
            }
        }
    }
}
