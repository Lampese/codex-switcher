//! Read-only account repository for the transitional dual-format boundary.

use zeroize::{Zeroize, Zeroizing};

use crate::auth::metadata_store::{
    AccountMetadataV2, MetadataAuthKind, MetadataFileStore, MetadataStoreV2,
};
use crate::auth::operation_lock::MutationLock;
use crate::auth::paths::AppPaths;
use crate::auth::secure_commit::verify_consistency;
use crate::auth::vault::VaultStore;
use crate::types::{AccountInfo, AccountsStore, AuthData, AuthMode};

#[derive(Debug, thiserror::Error)]
pub(crate) enum AccountRepositoryError {
    #[error("Failed to read account store")]
    AccountsReadFailed,

    #[error("Account store format is invalid")]
    InvalidStoreFormat,

    #[error("Secure metadata could not be loaded")]
    SecureMetadataLoadFailed,

    #[error("Secure vault could not be loaded")]
    SecureVaultLoadFailed,

    #[error("Secure account state is inconsistent")]
    SecureStateInconsistent,

    #[error("Failed to acquire account store lock")]
    LockFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RepositoryFormat {
    Empty,
    Legacy,
    Secure,
}

#[derive(serde::Deserialize)]
struct StoreDiscriminator {
    #[serde(default, deserialize_with = "deserialize_discriminator_u32")]
    schema_version: Option<u32>,
    #[serde(default, deserialize_with = "deserialize_discriminator_u32")]
    version: Option<u32>,
}

fn deserialize_discriminator_u32<'de, D>(deserializer: D) -> Result<Option<u32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    <u32 as serde::Deserialize<'de>>::deserialize(deserializer).map(Some)
}

pub(crate) struct AccountRepository {
    paths: AppPaths,
    metadata_store: MetadataFileStore,
    vault_store: VaultStore,
    mutation_lock: MutationLock,
}

impl AccountRepository {
    pub(crate) fn from_paths(paths: AppPaths) -> Self {
        Self {
            metadata_store: MetadataFileStore::from_paths(&paths),
            vault_store: VaultStore::from_paths(&paths),
            mutation_lock: MutationLock::from_paths(&paths),
            paths,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(paths: AppPaths) -> Self {
        Self::from_paths(paths)
    }

    pub(crate) async fn validate_startup_state(
        &self,
    ) -> Result<RepositoryFormat, AccountRepositoryError> {
        self.with_snapshot(|snapshot| Ok(snapshot.format())).await
    }

    pub(crate) async fn list_accounts(&self) -> Result<Vec<AccountInfo>, AccountRepositoryError> {
        self.with_snapshot(|snapshot| match snapshot {
            ValidatedSnapshot::Empty => Ok(Vec::new()),
            ValidatedSnapshot::Legacy(store) => {
                let active_id = store.store.active_account_id.as_deref();
                Ok(store
                    .store
                    .accounts
                    .iter()
                    .map(|account| AccountInfo::from_stored(account, active_id))
                    .collect())
            }
            ValidatedSnapshot::Secure(metadata) => {
                let active_id = metadata.active_account_id.as_deref();
                Ok(metadata
                    .accounts
                    .iter()
                    .map(|account| account_info_from_metadata(account, active_id))
                    .collect())
            }
        })
        .await
    }

    pub(crate) async fn get_active_account(
        &self,
    ) -> Result<Option<AccountInfo>, AccountRepositoryError> {
        self.with_snapshot(|snapshot| match snapshot {
            ValidatedSnapshot::Empty => Ok(None),
            ValidatedSnapshot::Legacy(store) => {
                let Some(active_id) = store.store.active_account_id.as_deref() else {
                    return Ok(None);
                };

                Ok(store
                    .store
                    .accounts
                    .iter()
                    .find(|account| account.id == active_id)
                    .map(|account| AccountInfo::from_stored(account, Some(active_id))))
            }
            ValidatedSnapshot::Secure(metadata) => {
                let Some(active_id) = metadata.active_account_id.as_deref() else {
                    return Ok(None);
                };

                Ok(metadata
                    .accounts
                    .iter()
                    .find(|account| account.id == active_id)
                    .map(|account| account_info_from_metadata(account, Some(active_id))))
            }
        })
        .await
    }

    pub(crate) async fn get_masked_account_ids(
        &self,
    ) -> Result<Vec<String>, AccountRepositoryError> {
        self.with_snapshot(|snapshot| match snapshot {
            ValidatedSnapshot::Empty => Ok(Vec::new()),
            ValidatedSnapshot::Legacy(store) => Ok(store.store.masked_account_ids.clone()),
            ValidatedSnapshot::Secure(metadata) => Ok(metadata.masked_account_ids.clone()),
        })
        .await
    }

    async fn with_snapshot<T, F>(&self, operation: F) -> Result<T, AccountRepositoryError>
    where
        F: FnOnce(ValidatedSnapshot) -> Result<T, AccountRepositoryError>,
    {
        let _guard = self
            .mutation_lock
            .acquire()
            .await
            .map_err(|_| AccountRepositoryError::LockFailed)?;
        let snapshot = self.detect_locked()?;
        operation(snapshot)
    }

    fn detect_locked(&self) -> Result<ValidatedSnapshot, AccountRepositoryError> {
        let metadata_exists = self.metadata_store.exists();
        let vault_exists = self.vault_store.exists();

        if !metadata_exists {
            return if vault_exists {
                Err(AccountRepositoryError::SecureStateInconsistent)
            } else {
                Ok(ValidatedSnapshot::Empty)
            };
        }

        let raw_bytes = Zeroizing::new(
            std::fs::read(&self.paths.metadata_file)
                .map_err(|_| AccountRepositoryError::AccountsReadFailed)?,
        );

        let discriminator = serde_json::from_slice::<StoreDiscriminator>(&raw_bytes)
            .map_err(|_| AccountRepositoryError::InvalidStoreFormat)?;

        match (discriminator.schema_version, discriminator.version) {
            (Some(_), None) => {
                let metadata = serde_json::from_slice::<MetadataStoreV2>(&raw_bytes)
                    .map_err(|_| AccountRepositoryError::SecureMetadataLoadFailed)?;
                metadata
                    .validate()
                    .map_err(|_| AccountRepositoryError::SecureMetadataLoadFailed)?;
                self.validate_secure_state(&metadata, vault_exists)?;
                Ok(ValidatedSnapshot::Secure(metadata))
            }
            (None, Some(_)) => {
                let legacy_store = serde_json::from_slice::<AccountsStore>(&raw_bytes)
                    .map_err(|_| AccountRepositoryError::InvalidStoreFormat)?;
                Ok(ValidatedSnapshot::Legacy(LegacyStoreGuard::new(
                    legacy_store,
                )))
            }
            (Some(_), Some(_)) | (None, None) => Err(AccountRepositoryError::InvalidStoreFormat),
        }
    }

    fn validate_secure_state(
        &self,
        metadata: &MetadataStoreV2,
        vault_exists: bool,
    ) -> Result<(), AccountRepositoryError> {
        if metadata.is_empty() {
            if !vault_exists {
                return Ok(());
            }
        } else if !vault_exists {
            return Err(AccountRepositoryError::SecureStateInconsistent);
        }

        let vault = self
            .vault_store
            .load()
            .map_err(|_| AccountRepositoryError::SecureVaultLoadFailed)?;

        if metadata.is_empty() && !vault.is_empty() {
            return Err(AccountRepositoryError::SecureStateInconsistent);
        }

        verify_consistency(&vault, metadata)
            .map_err(|_| AccountRepositoryError::SecureStateInconsistent)
    }
}

fn account_info_from_metadata(account: &AccountMetadataV2, active_id: Option<&str>) -> AccountInfo {
    AccountInfo {
        id: account.id.clone(),
        name: account.display_name.clone(),
        email: account.email.clone(),
        plan_type: account.plan_type.clone(),
        subscription_expires_at: account.subscription_expires_at.clone(),
        auth_mode: match account.auth_kind {
            MetadataAuthKind::ChatGpt => AuthMode::ChatGPT,
            MetadataAuthKind::ApiKey => AuthMode::ApiKey,
        },
        is_active: active_id == Some(account.id.as_str()),
        created_at: account.created_at,
        last_used_at: account.last_used_at,
    }
}

enum ValidatedSnapshot {
    Empty,
    Legacy(LegacyStoreGuard),
    Secure(MetadataStoreV2),
}

impl ValidatedSnapshot {
    fn format(&self) -> RepositoryFormat {
        match self {
            Self::Empty => RepositoryFormat::Empty,
            Self::Legacy(_) => RepositoryFormat::Legacy,
            Self::Secure(_) => RepositoryFormat::Secure,
        }
    }
}

struct LegacyStoreGuard {
    store: AccountsStore,
}

impl LegacyStoreGuard {
    fn new(store: AccountsStore) -> Self {
        Self { store }
    }
}

impl Drop for LegacyStoreGuard {
    fn drop(&mut self) {
        for account in &mut self.store.accounts {
            match &mut account.auth_data {
                AuthData::ApiKey { key } => key.zeroize(),
                AuthData::ChatGPT {
                    id_token,
                    access_token,
                    refresh_token,
                    account_id,
                } => {
                    id_token.zeroize();
                    access_token.zeroize();
                    refresh_token.zeroize();
                    if let Some(account_id) = account_id {
                        account_id.zeroize();
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::metadata_store::{AccountMetadataV2, MetadataAuthKind};
    use crate::auth::vault::{SecretRecord, VaultPayloadV1, VaultStore};
    use crate::types::{AuthData, StoredAccount};
    use chrono::{DateTime, TimeZone, Utc};
    use std::path::PathBuf;

    const ID_TOKEN_A: &str = "synthetic-id-token-A";
    const ACCESS_TOKEN_A: &str = "synthetic-access-token-A";
    const REFRESH_TOKEN_A: &str = "synthetic-refresh-token-A";
    const API_KEY_A: &str = "synthetic-api-key-A";
    const CHATGPT_ACCOUNT_A: &str = "synthetic-chatgpt-account-A";

    fn test_paths(tag: &str) -> (PathBuf, AppPaths) {
        let root = std::env::temp_dir().join(format!(
            "codex_repository_test_{}_{}",
            tag,
            rand::random::<u32>()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let paths = AppPaths::for_test(&root);
        (root, paths)
    }

    fn ensure_switcher_dir(paths: &AppPaths) {
        std::fs::create_dir_all(&paths.switcher_dir).unwrap();
    }

    fn cleanup(root: PathBuf) {
        let _ = std::fs::remove_dir_all(root);
    }

    fn timestamp(hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 5, hour, 0, 0)
            .single()
            .unwrap()
    }

    fn legacy_chatgpt(id: &str, name: &str, hour: u32) -> StoredAccount {
        StoredAccount {
            id: id.to_string(),
            name: name.to_string(),
            email: Some(format!("{id}@example.test")),
            plan_type: Some("pro".to_string()),
            subscription_expires_at: None,
            auth_mode: AuthMode::ChatGPT,
            auth_data: AuthData::ChatGPT {
                id_token: ID_TOKEN_A.to_string(),
                access_token: ACCESS_TOKEN_A.to_string(),
                refresh_token: REFRESH_TOKEN_A.to_string(),
                account_id: Some(CHATGPT_ACCOUNT_A.to_string()),
            },
            created_at: timestamp(hour),
            last_used_at: Some(timestamp(hour + 1)),
        }
    }

    fn legacy_api_key(id: &str, name: &str, hour: u32) -> StoredAccount {
        StoredAccount {
            id: id.to_string(),
            name: name.to_string(),
            email: None,
            plan_type: None,
            subscription_expires_at: None,
            auth_mode: AuthMode::ApiKey,
            auth_data: AuthData::ApiKey {
                key: API_KEY_A.to_string(),
            },
            created_at: timestamp(hour),
            last_used_at: None,
        }
    }

    fn legacy_store() -> AccountsStore {
        AccountsStore {
            version: 1,
            accounts: vec![
                legacy_chatgpt("legacy-chatgpt-A", "Legacy ChatGPT", 1),
                legacy_api_key("legacy-api-B", "Legacy API", 3),
            ],
            active_account_id: Some("legacy-api-B".to_string()),
            masked_account_ids: vec![
                "stale-mask".to_string(),
                "legacy-chatgpt-A".to_string(),
                "stale-mask".to_string(),
            ],
        }
    }

    fn write_legacy(paths: &AppPaths, store: &AccountsStore) -> Vec<u8> {
        ensure_switcher_dir(paths);
        let bytes = serde_json::to_vec(store).unwrap();
        std::fs::write(&paths.metadata_file, &bytes).unwrap();
        bytes
    }

    fn metadata_account(
        id: &str,
        display_name: &str,
        auth_kind: MetadataAuthKind,
        hour: u32,
    ) -> AccountMetadataV2 {
        AccountMetadataV2 {
            id: id.to_string(),
            display_name: display_name.to_string(),
            email: Some(format!("{id}@example.test")),
            plan_type: Some("pro".to_string()),
            subscription_expires_at: None,
            created_at: timestamp(hour),
            last_used_at: Some(timestamp(hour + 1)),
            auth_kind,
            vault_ref: id.to_string(),
        }
    }

    fn secure_metadata_store() -> MetadataStoreV2 {
        let mut metadata = MetadataStoreV2::new_empty();
        metadata
            .insert(metadata_account(
                "secure-chatgpt-A",
                "Secure ChatGPT",
                MetadataAuthKind::ChatGpt,
                5,
            ))
            .unwrap();
        metadata
            .insert(metadata_account(
                "secure-api-B",
                "Secure API",
                MetadataAuthKind::ApiKey,
                7,
            ))
            .unwrap();
        metadata.set_active(Some("secure-api-B")).unwrap();
        metadata.set_masked_account_ids(vec![
            "stale-secure-mask".to_string(),
            "secure-chatgpt-A".to_string(),
            "stale-secure-mask".to_string(),
        ]);
        metadata
    }

    fn secure_vault_payload() -> VaultPayloadV1 {
        let mut vault = VaultPayloadV1::new_empty();
        vault
            .insert(
                "secure-chatgpt-A",
                SecretRecord::ChatGpt {
                    id_token: ID_TOKEN_A.to_string(),
                    access_token: ACCESS_TOKEN_A.to_string(),
                    refresh_token: REFRESH_TOKEN_A.to_string(),
                    account_id: Some(CHATGPT_ACCOUNT_A.to_string()),
                },
            )
            .unwrap();
        vault
            .insert(
                "secure-api-B",
                SecretRecord::ApiKey {
                    key: API_KEY_A.to_string(),
                },
            )
            .unwrap();
        vault
    }

    fn write_secure_pair(paths: &AppPaths) -> (Vec<u8>, Vec<u8>) {
        ensure_switcher_dir(paths);
        let metadata = secure_metadata_store();
        let vault = secure_vault_payload();
        MetadataFileStore::from_paths(paths)
            .save(&metadata)
            .unwrap();
        VaultStore::from_paths(paths).save(&vault).unwrap();
        (
            std::fs::read(&paths.metadata_file).unwrap(),
            std::fs::read(&paths.vault_file).unwrap(),
        )
    }

    fn write_secure_metadata(paths: &AppPaths, metadata: &MetadataStoreV2) -> Vec<u8> {
        ensure_switcher_dir(paths);
        MetadataFileStore::from_paths(paths).save(metadata).unwrap();
        std::fs::read(&paths.metadata_file).unwrap()
    }

    fn write_secure_vault(paths: &AppPaths, vault: &VaultPayloadV1) -> Vec<u8> {
        ensure_switcher_dir(paths);
        VaultStore::from_paths(paths).save(vault).unwrap();
        std::fs::read(&paths.vault_file).unwrap()
    }

    fn assert_sanitized(message: &str) {
        for secret in [
            ID_TOKEN_A,
            ACCESS_TOKEN_A,
            REFRESH_TOKEN_A,
            API_KEY_A,
            CHATGPT_ACCOUNT_A,
        ] {
            assert!(!message.contains(secret), "error leaked secret: {message}");
        }
    }

    #[tokio::test]
    async fn test_validate_empty_without_files_returns_empty() {
        let (root, paths) = test_paths("empty");
        let repository = AccountRepository::for_test(paths.clone());

        assert_eq!(
            repository.validate_startup_state().await.unwrap(),
            RepositoryFormat::Empty
        );
        assert!(paths.operation_lock_file.exists());
        assert!(!paths.metadata_file.exists());
        assert!(!paths.vault_file.exists());

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_validate_empty_does_not_create_accounts_file() {
        let (root, paths) = test_paths("empty_no_accounts");
        let repository = AccountRepository::for_test(paths.clone());

        repository.validate_startup_state().await.unwrap();

        assert!(!paths.metadata_file.exists());
        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_validate_empty_does_not_create_vault_file() {
        let (root, paths) = test_paths("empty_no_vault");
        let repository = AccountRepository::for_test(paths.clone());

        repository.validate_startup_state().await.unwrap();

        assert!(!paths.vault_file.exists());
        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_validate_legacy_store_classifies_as_legacy() {
        let (root, paths) = test_paths("legacy_format");
        write_legacy(&paths, &legacy_store());
        let repository = AccountRepository::for_test(paths);

        assert_eq!(
            repository.validate_startup_state().await.unwrap(),
            RepositoryFormat::Legacy
        );

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_validate_legacy_accounts_bytes_remain_unchanged() {
        let (root, paths) = test_paths("legacy_unchanged");
        let original = write_legacy(&paths, &legacy_store());
        let repository = AccountRepository::for_test(paths.clone());

        repository.validate_startup_state().await.unwrap();

        assert_eq!(std::fs::read(&paths.metadata_file).unwrap(), original);
        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_validate_legacy_with_orphan_vault_classifies_as_legacy() {
        let (root, paths) = test_paths("legacy_orphan_vault");
        write_legacy(&paths, &legacy_store());
        let orphan_bytes = b"opaque legacy orphan vault".to_vec();
        std::fs::write(&paths.vault_file, &orphan_bytes).unwrap();
        let repository = AccountRepository::for_test(paths.clone());

        assert_eq!(
            repository.validate_startup_state().await.unwrap(),
            RepositoryFormat::Legacy
        );
        assert_eq!(std::fs::read(&paths.vault_file).unwrap(), orphan_bytes);

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_hybrid_secure_and_legacy_discriminators_are_rejected() {
        let (root, paths) = test_paths("hybrid_discriminators");
        let legacy_bytes = serde_json::to_vec(&legacy_store()).unwrap();
        let mut hybrid: serde_json::Value = serde_json::from_slice(&legacy_bytes).unwrap();
        let object = hybrid.as_object_mut().unwrap();
        object.insert("schema_version".to_string(), serde_json::json!(2));
        assert_eq!(
            object.get("version").and_then(serde_json::Value::as_u64),
            Some(1)
        );
        let hybrid_bytes = serde_json::to_vec(&hybrid).unwrap();

        ensure_switcher_dir(&paths);
        std::fs::write(&paths.metadata_file, &hybrid_bytes).unwrap();
        let orphan_bytes = b"opaque hybrid orphan vault".to_vec();
        std::fs::write(&paths.vault_file, &orphan_bytes).unwrap();
        let repository = AccountRepository::for_test(paths.clone());

        assert!(matches!(
            repository.validate_startup_state().await,
            Err(AccountRepositoryError::InvalidStoreFormat)
        ));
        assert_eq!(std::fs::read(&paths.metadata_file).unwrap(), hybrid_bytes);
        assert_eq!(std::fs::read(&paths.vault_file).unwrap(), orphan_bytes);

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_secure_candidate_never_falls_back_to_legacy() {
        let (root, paths) = test_paths("secure_candidate_no_legacy_fallback");
        let legacy_bytes = serde_json::to_vec(&legacy_store()).unwrap();
        let mut candidate: serde_json::Value = serde_json::from_slice(&legacy_bytes).unwrap();
        let object = candidate.as_object_mut().unwrap();
        object.remove("version").unwrap();
        object.insert("schema_version".to_string(), serde_json::json!(2));
        let candidate_bytes = serde_json::to_vec(&candidate).unwrap();

        ensure_switcher_dir(&paths);
        std::fs::write(&paths.metadata_file, &candidate_bytes).unwrap();
        let repository = AccountRepository::for_test(paths.clone());

        assert!(matches!(
            repository.validate_startup_state().await,
            Err(AccountRepositoryError::SecureMetadataLoadFailed)
        ));
        assert_eq!(
            std::fs::read(&paths.metadata_file).unwrap(),
            candidate_bytes
        );

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_validate_secure_matching_pair_classifies_as_secure() {
        let (root, paths) = test_paths("secure_pair");
        write_secure_pair(&paths);
        let repository = AccountRepository::for_test(paths);

        assert_eq!(
            repository.validate_startup_state().await.unwrap(),
            RepositoryFormat::Secure
        );

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_validate_empty_secure_metadata_without_vault_is_valid() {
        let (root, paths) = test_paths("secure_empty");
        write_secure_metadata(&paths, &MetadataStoreV2::new_empty());
        let repository = AccountRepository::for_test(paths);

        assert_eq!(
            repository.validate_startup_state().await.unwrap(),
            RepositoryFormat::Secure
        );

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_validate_non_empty_secure_metadata_without_vault_fails_closed() {
        let (root, paths) = test_paths("secure_missing_vault");
        let metadata = secure_metadata_store();
        write_secure_metadata(&paths, &metadata);
        let repository = AccountRepository::for_test(paths);

        assert!(matches!(
            repository.validate_startup_state().await,
            Err(AccountRepositoryError::SecureStateInconsistent)
        ));

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_validate_secure_metadata_with_corrupt_vault_fails_closed() {
        let (root, paths) = test_paths("secure_corrupt_vault");
        write_secure_metadata(&paths, &secure_metadata_store());
        let corrupt_bytes = b"not a vault envelope".to_vec();
        std::fs::write(&paths.vault_file, &corrupt_bytes).unwrap();
        let repository = AccountRepository::for_test(paths);

        assert!(matches!(
            repository.validate_startup_state().await,
            Err(AccountRepositoryError::SecureVaultLoadFailed)
        ));

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_validate_secure_metadata_vault_account_mismatch_fails_closed() {
        let (root, paths) = test_paths("secure_account_mismatch");
        let mut metadata = MetadataStoreV2::new_empty();
        metadata
            .insert(metadata_account(
                "secure-chatgpt-A",
                "Secure ChatGPT",
                MetadataAuthKind::ChatGpt,
                5,
            ))
            .unwrap();
        write_secure_metadata(&paths, &metadata);

        let mut vault = VaultPayloadV1::new_empty();
        vault
            .insert(
                "secure-other-B",
                SecretRecord::ChatGpt {
                    id_token: ID_TOKEN_A.to_string(),
                    access_token: ACCESS_TOKEN_A.to_string(),
                    refresh_token: REFRESH_TOKEN_A.to_string(),
                    account_id: Some(CHATGPT_ACCOUNT_A.to_string()),
                },
            )
            .unwrap();
        write_secure_vault(&paths, &vault);
        let repository = AccountRepository::for_test(paths);

        assert!(matches!(
            repository.validate_startup_state().await,
            Err(AccountRepositoryError::SecureStateInconsistent)
        ));

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_validate_secure_metadata_vault_auth_kind_mismatch_fails_closed() {
        let (root, paths) = test_paths("secure_auth_kind_mismatch");
        let mut metadata = MetadataStoreV2::new_empty();
        metadata
            .insert(metadata_account(
                "secure-chatgpt-A",
                "Secure ChatGPT",
                MetadataAuthKind::ChatGpt,
                5,
            ))
            .unwrap();
        write_secure_metadata(&paths, &metadata);

        let mut vault = VaultPayloadV1::new_empty();
        vault
            .insert(
                "secure-chatgpt-A",
                SecretRecord::ApiKey {
                    key: API_KEY_A.to_string(),
                },
            )
            .unwrap();
        write_secure_vault(&paths, &vault);
        let repository = AccountRepository::for_test(paths);

        assert!(matches!(
            repository.validate_startup_state().await,
            Err(AccountRepositoryError::SecureStateInconsistent)
        ));

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_validate_malformed_accounts_fails_without_modification() {
        let (root, paths) = test_paths("malformed_accounts");
        ensure_switcher_dir(&paths);
        let original = b"{ malformed accounts json".to_vec();
        std::fs::write(&paths.metadata_file, &original).unwrap();
        let repository = AccountRepository::for_test(paths.clone());

        assert!(matches!(
            repository.validate_startup_state().await,
            Err(AccountRepositoryError::InvalidStoreFormat)
        ));
        assert_eq!(std::fs::read(&paths.metadata_file).unwrap(), original);
        assert!(!paths.vault_file.exists());

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_list_legacy_preserves_ordering_and_active_account() {
        let (root, paths) = test_paths("list_legacy");
        write_legacy(&paths, &legacy_store());
        let repository = AccountRepository::for_test(paths);

        let accounts = repository.list_accounts().await.unwrap();

        assert_eq!(
            accounts
                .iter()
                .map(|account| account.id.as_str())
                .collect::<Vec<_>>(),
            vec!["legacy-chatgpt-A", "legacy-api-B"]
        );
        assert_eq!(accounts[0].auth_mode, AuthMode::ChatGPT);
        assert!(!accounts[0].is_active);
        assert_eq!(accounts[1].auth_mode, AuthMode::ApiKey);
        assert!(accounts[1].is_active);

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_list_secure_preserves_ordering_and_active_account() {
        let (root, paths) = test_paths("list_secure");
        write_secure_pair(&paths);
        let repository = AccountRepository::for_test(paths);

        let accounts = repository.list_accounts().await.unwrap();

        assert_eq!(
            accounts
                .iter()
                .map(|account| account.id.as_str())
                .collect::<Vec<_>>(),
            vec!["secure-chatgpt-A", "secure-api-B"]
        );
        assert_eq!(accounts[0].auth_mode, AuthMode::ChatGPT);
        assert!(!accounts[0].is_active);
        assert_eq!(accounts[1].auth_mode, AuthMode::ApiKey);
        assert!(accounts[1].is_active);

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_get_active_legacy_account() {
        let (root, paths) = test_paths("active_legacy");
        write_legacy(&paths, &legacy_store());
        let repository = AccountRepository::for_test(paths);

        let active = repository.get_active_account().await.unwrap().unwrap();

        assert_eq!(active.id, "legacy-api-B");
        assert!(active.is_active);
        assert_eq!(active.auth_mode, AuthMode::ApiKey);

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_get_active_secure_account() {
        let (root, paths) = test_paths("active_secure");
        write_secure_pair(&paths);
        let repository = AccountRepository::for_test(paths);

        let active = repository.get_active_account().await.unwrap().unwrap();

        assert_eq!(active.id, "secure-api-B");
        assert!(active.is_active);
        assert_eq!(active.auth_mode, AuthMode::ApiKey);

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_get_active_without_active_account_returns_none() {
        let (root, paths) = test_paths("no_active");
        let mut store = legacy_store();
        store.active_account_id = None;
        write_legacy(&paths, &store);
        let repository = AccountRepository::for_test(paths);

        assert!(repository.get_active_account().await.unwrap().is_none());

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_get_legacy_masked_ids_preserves_ordering_stale_ids_and_duplicates() {
        let (root, paths) = test_paths("masked_legacy");
        let store = legacy_store();
        let expected = store.masked_account_ids.clone();
        write_legacy(&paths, &store);
        let repository = AccountRepository::for_test(paths);

        assert_eq!(repository.get_masked_account_ids().await.unwrap(), expected);

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_get_secure_masked_ids_preserves_ordering_stale_ids_and_duplicates() {
        let (root, paths) = test_paths("masked_secure");
        let metadata = secure_metadata_store();
        let expected = metadata.masked_account_ids.clone();
        write_secure_metadata(&paths, &metadata);
        write_secure_vault(&paths, &secure_vault_payload());
        let repository = AccountRepository::for_test(paths);

        assert_eq!(repository.get_masked_account_ids().await.unwrap(), expected);

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_read_operations_leave_legacy_accounts_bytes_unchanged() {
        let (root, paths) = test_paths("read_accounts_unchanged");
        let original = write_legacy(&paths, &legacy_store());
        let repository = AccountRepository::for_test(paths.clone());

        repository.list_accounts().await.unwrap();
        repository.get_active_account().await.unwrap();
        repository.get_masked_account_ids().await.unwrap();

        assert_eq!(std::fs::read(&paths.metadata_file).unwrap(), original);
        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_read_operations_leave_legacy_vault_bytes_unchanged() {
        let (root, paths) = test_paths("read_vault_unchanged");
        write_legacy(&paths, &legacy_store());
        let original_vault = b"opaque legacy vault bytes".to_vec();
        std::fs::write(&paths.vault_file, &original_vault).unwrap();
        let repository = AccountRepository::for_test(paths.clone());

        repository.list_accounts().await.unwrap();
        repository.get_active_account().await.unwrap();
        repository.get_masked_account_ids().await.unwrap();

        assert_eq!(std::fs::read(&paths.vault_file).unwrap(), original_vault);
        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_read_operations_leave_secure_accounts_bytes_unchanged() {
        let (root, paths) = test_paths("read_secure_accounts_unchanged");
        let (original_metadata, _) = write_secure_pair(&paths);
        let repository = AccountRepository::for_test(paths.clone());

        repository.list_accounts().await.unwrap();
        repository.get_active_account().await.unwrap();
        repository.get_masked_account_ids().await.unwrap();

        assert_eq!(
            std::fs::read(&paths.metadata_file).unwrap(),
            original_metadata
        );
        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_read_operations_leave_secure_vault_bytes_unchanged() {
        let (root, paths) = test_paths("read_secure_vault_unchanged");
        let (_, original_vault) = write_secure_pair(&paths);
        let repository = AccountRepository::for_test(paths.clone());

        repository.list_accounts().await.unwrap();
        repository.get_active_account().await.unwrap();
        repository.get_masked_account_ids().await.unwrap();

        assert_eq!(std::fs::read(&paths.vault_file).unwrap(), original_vault);
        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_legacy_credentials_do_not_appear_in_repository_errors() {
        let (root, paths) = test_paths("legacy_error_secrecy");
        let store_bytes = serde_json::to_vec(&legacy_store()).unwrap();
        ensure_switcher_dir(&paths);
        std::fs::write(&paths.metadata_file, &store_bytes[..store_bytes.len() - 1]).unwrap();
        let repository = AccountRepository::for_test(paths);

        let error = repository.validate_startup_state().await.unwrap_err();
        assert_sanitized(&error.to_string());

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_secure_credentials_do_not_appear_in_repository_errors() {
        let (root, paths) = test_paths("secure_error_secrecy");
        write_secure_metadata(&paths, &secure_metadata_store());
        std::fs::write(&paths.vault_file, b"corrupt secure vault").unwrap();
        let repository = AccountRepository::for_test(paths);

        let error = repository.validate_startup_state().await.unwrap_err();
        assert_sanitized(&error.to_string());

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_validate_startup_does_not_migrate_or_replace_legacy_data() {
        let (root, paths) = test_paths("startup_no_migration");
        let original = write_legacy(&paths, &legacy_store());
        let repository = AccountRepository::for_test(paths.clone());

        assert_eq!(
            repository.validate_startup_state().await.unwrap(),
            RepositoryFormat::Legacy
        );
        assert_eq!(std::fs::read(&paths.metadata_file).unwrap(), original);
        assert!(serde_json::from_slice::<AccountsStore>(&original).is_ok());
        assert!(serde_json::from_slice::<MetadataStoreV2>(&original).is_err());
        assert!(!paths.vault_file.exists());

        drop(repository);
        cleanup(root);
    }
}
