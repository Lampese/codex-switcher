//! Transactional vault + metadata pair committer — Phase 1B-2.

use std::path::PathBuf;

use crate::auth::atomic_file::{atomic_write, FileSensitivity};
use crate::auth::metadata_store::{MetadataAuthKind, MetadataFileStore, MetadataStoreV2};
use crate::auth::operation_lock::MutationGuard;
use crate::auth::paths::AppPaths;
use crate::auth::vault::{SecretRecord, VaultPayloadV1, VaultStore};

/// Errors returned by secure pair commit operations.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SecureCommitError {
    #[error("Failed to encode vault payload")]
    VaultEncodeFailed,

    #[error("Failed to encode metadata store")]
    MetadataEncodeFailed,

    #[error("Metadata account set and vault secret record set do not match")]
    StoreMismatch,

    #[error("Metadata auth_kind does not match vault secret record variant")]
    AuthKindMismatch,

    #[error("Failed to read snapshot of existing vault file")]
    VaultSnapshotReadFailed,

    #[error("Failed to install vault file")]
    VaultInstallFailed,

    #[error("Failed to install metadata file")]
    MetadataInstallFailed,

    #[error("Failed to rollback vault file after metadata install failure")]
    VaultRollbackFailed,

    #[error("Critical rollback failure: metadata install failed and vault rollback failed")]
    CriticalRollbackFailed,

    #[cfg(test)]
    #[error("Simulated process crash after vault installation")]
    SimulatedCrashAfterVaultInstall,
}

/// Independent failure controls for deterministic test verification.
#[cfg(test)]
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct SecureCommitTestOptions {
    pub(crate) fail_before_vault_install: bool,
    pub(crate) simulate_crash_after_vault_install: bool,
    pub(crate) fail_metadata_install: bool,
    pub(crate) fail_vault_rollback: bool,
}

pub(crate) struct SecurePairCommitter {
    vault_store: VaultStore,
    metadata_store: MetadataFileStore,
    vault_path: PathBuf,
    #[cfg(test)]
    test_options: SecureCommitTestOptions,
}

impl SecurePairCommitter {
    pub(crate) fn from_paths(paths: &AppPaths) -> Self {
        Self {
            vault_store: VaultStore::from_paths(paths),
            metadata_store: MetadataFileStore::from_paths(paths),
            vault_path: paths.vault_file.clone(),
            #[cfg(test)]
            test_options: SecureCommitTestOptions::default(),
        }
    }

    pub(crate) fn for_paths(vault_path: PathBuf, metadata_path: PathBuf) -> Self {
        Self {
            vault_store: VaultStore::for_path(vault_path.clone()),
            metadata_store: MetadataFileStore::for_path(metadata_path),
            vault_path,
            #[cfg(test)]
            test_options: SecureCommitTestOptions::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_test_options(mut self, test_options: SecureCommitTestOptions) -> Self {
        self.test_options = test_options;
        self
    }

    /// Perform transactional pair commit of vault and metadata.
    /// Requires caller to pass a `&MutationGuard` proving lock ownership.
    pub(crate) fn commit(
        &self,
        _guard: &MutationGuard,
        vault: &VaultPayloadV1,
        metadata: &MetadataStoreV2,
    ) -> Result<(), SecureCommitError> {
        // Step 1: Encode and validate vault in memory.
        let encoded_vault = self
            .vault_store
            .encode(vault)
            .map_err(|_| SecureCommitError::VaultEncodeFailed)?;

        // Step 2: Encode and validate metadata in memory.
        let encoded_metadata = self
            .metadata_store
            .encode(metadata)
            .map_err(|_| SecureCommitError::MetadataEncodeFailed)?;

        // Step 3 & 4: Consistency checks between metadata and vault.
        verify_consistency(vault, metadata)?;

        #[cfg(test)]
        if self.test_options.fail_before_vault_install {
            return Err(SecureCommitError::VaultInstallFailed);
        }

        // Step 5: Snapshot existing vault.dat if it exists.
        let vault_snapshot = if self.vault_path.exists() {
            Some(
                std::fs::read(&self.vault_path)
                    .map_err(|_| SecureCommitError::VaultSnapshotReadFailed)?,
            )
        } else {
            None
        };

        // Step 6: Install the new encrypted vault.
        self.vault_store
            .install_encoded(&encoded_vault)
            .map_err(|_| SecureCommitError::VaultInstallFailed)?;

        #[cfg(test)]
        if self.test_options.simulate_crash_after_vault_install {
            return Err(SecureCommitError::SimulatedCrashAfterVaultInstall);
        }

        // Step 7: Install the new metadata.
        #[cfg(test)]
        let metadata_res = if self.test_options.fail_metadata_install {
            Err(SecureCommitError::MetadataInstallFailed)
        } else {
            self.metadata_store
                .install_encoded(&encoded_metadata)
                .map_err(|_| SecureCommitError::MetadataInstallFailed)
        };

        #[cfg(not(test))]
        let metadata_res = self
            .metadata_store
            .install_encoded(&encoded_metadata)
            .map_err(|_| SecureCommitError::MetadataInstallFailed);

        // Step 8: Return success if metadata installed cleanly.
        if metadata_res.is_ok() {
            return Ok(());
        }

        // Step 9: Rollback vault if metadata install failed.
        let rollback_res = self.rollback_vault(vault_snapshot.as_deref());

        match rollback_res {
            Ok(()) => Err(SecureCommitError::MetadataInstallFailed),
            Err(_) => Err(SecureCommitError::CriticalRollbackFailed),
        }
    }

    fn rollback_vault(&self, snapshot: Option<&[u8]>) -> Result<(), SecureCommitError> {
        #[cfg(test)]
        if self.test_options.fail_vault_rollback {
            return Err(SecureCommitError::VaultRollbackFailed);
        }

        match snapshot {
            Some(old_bytes) => atomic_write(&self.vault_path, old_bytes, FileSensitivity::Secret)
                .map_err(|_| SecureCommitError::VaultRollbackFailed),
            None => {
                if self.vault_path.exists() {
                    std::fs::remove_file(&self.vault_path)
                        .map_err(|_| SecureCommitError::VaultRollbackFailed)?;
                }
                Ok(())
            }
        }
    }
}

fn verify_consistency(
    vault: &VaultPayloadV1,
    metadata: &MetadataStoreV2,
) -> Result<(), SecureCommitError> {
    if metadata.len() != vault.len() {
        return Err(SecureCommitError::StoreMismatch);
    }

    for acc in &metadata.accounts {
        let record = vault
            .get(&acc.vault_ref)
            .ok_or(SecureCommitError::StoreMismatch)?;

        match (&acc.auth_kind, record) {
            (MetadataAuthKind::ChatGpt, SecretRecord::ChatGpt { .. }) => {}
            (MetadataAuthKind::ApiKey, SecretRecord::ApiKey { .. }) => {}
            _ => return Err(SecureCommitError::AuthKindMismatch),
        }
    }

    // Check no unreferenced vault records exist
    for (vault_id, _) in &vault.accounts {
        if !metadata.accounts.iter().any(|a| &a.vault_ref == vault_id) {
            return Err(SecureCommitError::StoreMismatch);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::metadata_store::AccountMetadataV2;
    use crate::auth::operation_lock::MutationLock;
    use chrono::Utc;
    use std::path::PathBuf;

    const ID_TOKEN_A: &str = "synthetic-id-token-A";
    const ACCESS_TOKEN_A: &str = "synthetic-access-token-A";
    const REFRESH_TOKEN_A: &str = "synthetic-refresh-token-A";

    fn test_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "codex_commit_test_{}_{}",
            tag,
            rand::random::<u32>()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn sample_pair() -> (VaultPayloadV1, MetadataStoreV2) {
        let mut vault = VaultPayloadV1::new_empty();
        let rec = SecretRecord::ChatGpt {
            id_token: ID_TOKEN_A.to_string(),
            access_token: ACCESS_TOKEN_A.to_string(),
            refresh_token: REFRESH_TOKEN_A.to_string(),
            account_id: Some("acc-1".to_string()),
        };
        vault.insert("acc-1", rec).unwrap();

        let mut meta = MetadataStoreV2::new_empty();
        let acc = AccountMetadataV2 {
            id: "acc-1".to_string(),
            display_name: "User acc-1".to_string(),
            email: Some("acc1@example.com".to_string()),
            plan_type: Some("pro".to_string()),
            subscription_expires_at: None,
            created_at: Utc::now(),
            last_used_at: None,
            auth_kind: MetadataAuthKind::ChatGpt,
            vault_ref: "acc-1".to_string(),
        };
        meta.insert(acc).unwrap();

        (vault, meta)
    }

    #[tokio::test]
    async fn test_commit_successful_pair_writes_both_stores() {
        let d = test_dir("success_pair");
        let lock = MutationLock::for_path(d.join("operation.lock"));
        let committer =
            SecurePairCommitter::for_paths(d.join("vault.dat"), d.join("accounts.json"));

        let guard = lock.acquire().await.unwrap();
        let (vault, meta) = sample_pair();

        let res = committer.commit(&guard, &vault, &meta);
        assert!(res.is_ok());
        assert!(d.join("vault.dat").exists());
        assert!(d.join("accounts.json").exists());

        drop(guard);
        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_commit_vault_failure_leaves_metadata_untouched() {
        let d = test_dir("vault_fail");
        let lock = MutationLock::for_path(d.join("operation.lock"));
        let committer =
            SecurePairCommitter::for_paths(d.join("vault.dat"), d.join("accounts.json"))
                .with_test_options(SecureCommitTestOptions {
                    fail_before_vault_install: true,
                    ..Default::default()
                });

        let guard = lock.acquire().await.unwrap();
        let (vault, meta) = sample_pair();

        let res = committer.commit(&guard, &vault, &meta);
        assert!(matches!(res, Err(SecureCommitError::VaultInstallFailed)));
        assert!(!d.join("vault.dat").exists());
        assert!(!d.join("accounts.json").exists());

        drop(guard);
        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_commit_metadata_failure_restores_previous_vault() {
        let d = test_dir("meta_fail_restore");
        let lock = MutationLock::for_path(d.join("operation.lock"));
        let committer_good =
            SecurePairCommitter::for_paths(d.join("vault.dat"), d.join("accounts.json"));

        let guard = lock.acquire().await.unwrap();
        let (vault1, meta1) = sample_pair();

        committer_good.commit(&guard, &vault1, &meta1).unwrap();
        let old_vault_bytes = std::fs::read(d.join("vault.dat")).unwrap();
        let old_metadata_bytes = std::fs::read(d.join("accounts.json")).unwrap();

        let committer_fail =
            SecurePairCommitter::for_paths(d.join("vault.dat"), d.join("accounts.json"))
                .with_test_options(SecureCommitTestOptions {
                    fail_metadata_install: true,
                    ..Default::default()
                });

        let (vault2, meta2) = sample_pair();
        let res = committer_fail.commit(&guard, &vault2, &meta2);
        assert!(matches!(res, Err(SecureCommitError::MetadataInstallFailed)));

        // Previous vault must be restored exactly
        let current_vault_bytes = std::fs::read(d.join("vault.dat")).unwrap();
        assert_eq!(current_vault_bytes, old_vault_bytes);
        let current_metadata_bytes = std::fs::read(d.join("accounts.json")).unwrap();
        assert_eq!(current_metadata_bytes, old_metadata_bytes);

        drop(guard);
        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_commit_metadata_failure_restores_corrupt_prior_vault_exactly() {
        let d = test_dir("meta_fail_corrupt_prior_vault");
        let vault_path = d.join("vault.dat");
        let metadata_path = d.join("accounts.json");
        let lock = MutationLock::for_path(d.join("operation.lock"));
        let old_metadata_bytes = b"known previous metadata bytes\n".to_vec();
        let old_vault_bytes = b"opaque corrupt prior vault snapshot\x00\xff".to_vec();

        std::fs::write(&metadata_path, &old_metadata_bytes).unwrap();
        std::fs::write(&vault_path, &old_vault_bytes).unwrap();

        let committer_fail =
            SecurePairCommitter::for_paths(vault_path.clone(), metadata_path.clone())
                .with_test_options(SecureCommitTestOptions {
                    fail_metadata_install: true,
                    ..Default::default()
                });
        let guard = lock.acquire().await.unwrap();
        let (vault, meta) = sample_pair();

        let res = committer_fail.commit(&guard, &vault, &meta);
        assert!(matches!(res, Err(SecureCommitError::MetadataInstallFailed)));
        assert_eq!(std::fs::read(&metadata_path).unwrap(), old_metadata_bytes);
        assert_eq!(std::fs::read(&vault_path).unwrap(), old_vault_bytes);

        drop(guard);
        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_commit_metadata_failure_with_no_previous_vault_removes_new_vault() {
        let d = test_dir("meta_fail_no_prev");
        let lock = MutationLock::for_path(d.join("operation.lock"));
        let committer_fail =
            SecurePairCommitter::for_paths(d.join("vault.dat"), d.join("accounts.json"))
                .with_test_options(SecureCommitTestOptions {
                    fail_metadata_install: true,
                    ..Default::default()
                });

        let guard = lock.acquire().await.unwrap();
        let (vault, meta) = sample_pair();

        let res = committer_fail.commit(&guard, &vault, &meta);
        assert!(matches!(res, Err(SecureCommitError::MetadataInstallFailed)));

        assert!(!d.join("vault.dat").exists());
        assert!(!d.join("accounts.json").exists());

        drop(guard);
        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_commit_rollback_failure_returns_critical_rollback_failed() {
        let d = test_dir("critical_rollback");
        let vault_path = d.join("vault.dat");
        let metadata_path = d.join("accounts.json");
        let lock = MutationLock::for_path(d.join("operation.lock"));

        let old_vault = VaultPayloadV1::new_empty();
        VaultStore::for_path(vault_path.clone())
            .save(&old_vault)
            .unwrap();
        let old_vault_bytes = std::fs::read(&vault_path).unwrap();

        let old_metadata = MetadataStoreV2::new_empty();
        MetadataFileStore::for_path(metadata_path.clone())
            .save(&old_metadata)
            .unwrap();
        let old_metadata_bytes = std::fs::read(&metadata_path).unwrap();

        let test_options = SecureCommitTestOptions {
            fail_metadata_install: true,
            fail_vault_rollback: true,
            ..Default::default()
        };
        assert!(test_options.fail_metadata_install);
        assert!(test_options.fail_vault_rollback);

        let committer_fail =
            SecurePairCommitter::for_paths(vault_path.clone(), metadata_path.clone())
                .with_test_options(test_options);
        let guard = lock.acquire().await.unwrap();
        let (vault, meta) = sample_pair();

        let res = committer_fail.commit(&guard, &vault, &meta);
        assert!(matches!(
            res,
            Err(SecureCommitError::CriticalRollbackFailed)
        ));
        assert_eq!(std::fs::read(&metadata_path).unwrap(), old_metadata_bytes);

        let current_vault_bytes = std::fs::read(&vault_path).unwrap();
        assert_ne!(current_vault_bytes, old_vault_bytes);
        let current_vault = VaultStore::for_path(vault_path).load().unwrap();
        assert_eq!(current_vault.len(), 1);

        drop(guard);
        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_commit_mismatched_account_sets_rejected_before_disk_mutation() {
        let d = test_dir("mismatched_sets");
        let lock = MutationLock::for_path(d.join("operation.lock"));
        let committer =
            SecurePairCommitter::for_paths(d.join("vault.dat"), d.join("accounts.json"));

        let guard = lock.acquire().await.unwrap();
        let (vault, mut meta) = sample_pair();

        // Add extra account to metadata that doesn't exist in vault
        let acc2 = AccountMetadataV2 {
            id: "acc-2".to_string(),
            display_name: "User acc-2".to_string(),
            email: Some("acc2@example.com".to_string()),
            plan_type: Some("pro".to_string()),
            subscription_expires_at: None,
            created_at: Utc::now(),
            last_used_at: None,
            auth_kind: MetadataAuthKind::ApiKey,
            vault_ref: "acc-2".to_string(),
        };
        meta.insert(acc2).unwrap();

        let res = committer.commit(&guard, &vault, &meta);
        assert!(matches!(res, Err(SecureCommitError::StoreMismatch)));
        assert!(!d.join("vault.dat").exists());
        assert!(!d.join("accounts.json").exists());

        drop(guard);
        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_commit_auth_kind_mismatch_rejected_before_disk_mutation() {
        let d = test_dir("auth_kind_mismatch");
        let lock = MutationLock::for_path(d.join("operation.lock"));
        let committer =
            SecurePairCommitter::for_paths(d.join("vault.dat"), d.join("accounts.json"));

        let guard = lock.acquire().await.unwrap();
        let (vault, mut meta) = sample_pair();

        // Change metadata auth_kind to ApiKey while vault secret is ChatGpt
        meta.accounts[0].auth_kind = MetadataAuthKind::ApiKey;

        let res = committer.commit(&guard, &vault, &meta);
        assert!(matches!(res, Err(SecureCommitError::AuthKindMismatch)));
        assert!(!d.join("vault.dat").exists());
        assert!(!d.join("accounts.json").exists());

        drop(guard);
        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_commit_active_account_does_not_affect_vault_matching() {
        let d = test_dir("active_account_matching");
        let lock = MutationLock::for_path(d.join("operation.lock"));
        let committer =
            SecurePairCommitter::for_paths(d.join("vault.dat"), d.join("accounts.json"));

        let guard = lock.acquire().await.unwrap();
        let (vault, mut meta) = sample_pair();

        meta.set_active(Some("acc-1")).unwrap();

        let res = committer.commit(&guard, &vault, &meta);
        assert!(res.is_ok());
        assert!(d.join("vault.dat").exists());
        assert!(d.join("accounts.json").exists());

        drop(guard);
        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_commit_stale_masked_ids_do_not_affect_matching() {
        let d = test_dir("stale_masked_matching");
        let lock = MutationLock::for_path(d.join("operation.lock"));
        let committer =
            SecurePairCommitter::for_paths(d.join("vault.dat"), d.join("accounts.json"));

        let guard = lock.acquire().await.unwrap();
        let (vault, mut meta) = sample_pair();

        meta.set_masked_account_ids(vec!["stale-ghost-id".to_string()]);

        let res = committer.commit(&guard, &vault, &meta);
        assert!(res.is_ok());
        assert!(d.join("vault.dat").exists());
        assert!(d.join("accounts.json").exists());

        drop(guard);
        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_commit_temp_files_cleaned_after_handled_failures() {
        let d = test_dir("temp_cleaned");
        let lock = MutationLock::for_path(d.join("operation.lock"));
        let committer =
            SecurePairCommitter::for_paths(d.join("vault.dat"), d.join("accounts.json"))
                .with_test_options(SecureCommitTestOptions {
                    fail_metadata_install: true,
                    ..Default::default()
                });

        let guard = lock.acquire().await.unwrap();
        let (vault, meta) = sample_pair();

        let res = committer.commit(&guard, &vault, &meta);
        assert!(matches!(res, Err(SecureCommitError::MetadataInstallFailed)));
        assert!(!d.join("vault.dat").exists());
        assert!(!d.join("accounts.json").exists());

        let entries: Vec<_> = std::fs::read_dir(&d)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();

        for name in entries {
            assert!(
                !name.starts_with(".tmp_"),
                "Temporary file remained after failure: {name}"
            );
        }

        drop(guard);
        let _ = std::fs::remove_dir_all(d);
    }
}
