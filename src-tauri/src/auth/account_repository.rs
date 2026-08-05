//! Read-only account repository for the transitional dual-format boundary.

use chrono::{DateTime, Utc};
use zeroize::{Zeroize, Zeroizing};

use crate::auth::metadata_store::{
    AccountMetadataV2, MetadataAuthKind, MetadataFileStore, MetadataStoreError, MetadataStoreV2,
};
use crate::auth::operation_lock::MutationLock;
use crate::auth::paths::AppPaths;
use crate::auth::secure_commit::{verify_consistency, SecureCommitError, SecurePairCommitter};
use crate::auth::vault::{SecretRecord, VaultPayloadV1, VaultStore};
use crate::types::{AccountInfo, AccountsStore, AuthData, AuthMode, StoredAccount};

#[cfg(test)]
use crate::auth::secure_commit::SecureCommitTestOptions;

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

    #[error("Legacy account storage requires secure migration")]
    LegacyMigrationRequired,

    #[error("Account was not found")]
    AccountNotFound,

    #[error("Duplicate account ID")]
    DuplicateAccountId,

    #[error("Duplicate display name")]
    DuplicateDisplayName,

    #[error("Invalid account data")]
    InvalidAccountData,

    #[error("Authentication kind does not match secret")]
    AuthKindMismatch,

    #[error("Secure account mutation commit failed")]
    MutationCommitFailed,

    #[error("Critical secure account mutation rollback failed")]
    CriticalMutationRollbackFailed,
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

pub(crate) struct SecureAccountInsert {
    pub(crate) metadata: AccountMetadataV2,
    pub(crate) secret: SecretRecord,
}

pub(crate) struct AccountExportSnapshot {
    store: AccountsStore,
}

impl AccountExportSnapshot {
    pub(crate) fn store(&self) -> &AccountsStore {
        &self.store
    }

    fn empty() -> Self {
        Self {
            store: AccountsStore {
                version: 1,
                accounts: Vec::new(),
                active_account_id: None,
                masked_account_ids: Vec::new(),
            },
        }
    }

    fn from_legacy(guard: LegacyStoreGuard) -> Self {
        Self {
            store: guard.into_store(),
        }
    }

    fn from_secure(
        metadata: MetadataStoreV2,
        mut vault: VaultPayloadV1,
    ) -> Result<Self, AccountRepositoryError> {
        let mut snapshot = Self {
            store: AccountsStore {
                version: 1,
                accounts: Vec::with_capacity(metadata.accounts.len()),
                active_account_id: metadata.active_account_id,
                masked_account_ids: metadata.masked_account_ids,
            },
        };

        for account in metadata.accounts {
            let secret = vault
                .accounts
                .remove(&account.vault_ref)
                .ok_or(AccountRepositoryError::SecureStateInconsistent)?;
            if !auth_kind_matches(&account.auth_kind, &secret) {
                return Err(AccountRepositoryError::SecureStateInconsistent);
            }
            let auth_data = secret_into_auth_data(secret);

            snapshot.store.accounts.push(StoredAccount {
                id: account.id,
                name: account.display_name,
                email: account.email,
                plan_type: account.plan_type,
                subscription_expires_at: account.subscription_expires_at,
                auth_mode: match account.auth_kind {
                    MetadataAuthKind::ChatGpt => AuthMode::ChatGPT,
                    MetadataAuthKind::ApiKey => AuthMode::ApiKey,
                },
                auth_data,
                created_at: account.created_at,
                last_used_at: account.last_used_at,
            });
        }

        Ok(snapshot)
    }
}

impl Drop for AccountExportSnapshot {
    fn drop(&mut self) {
        zeroize_account_store(&mut self.store);
    }
}

#[derive(Default)]
pub(crate) struct AccountMetadataPatch {
    pub(crate) display_name: Option<String>,
    pub(crate) email: Option<Option<String>>,
    pub(crate) plan_type: Option<Option<String>>,
    pub(crate) subscription_expires_at: Option<Option<DateTime<Utc>>>,
}

pub(crate) struct AccountRepository {
    paths: AppPaths,
    metadata_store: MetadataFileStore,
    vault_store: VaultStore,
    mutation_lock: MutationLock,
    committer: SecurePairCommitter,
}

impl AccountRepository {
    pub(crate) fn from_paths(paths: AppPaths) -> Self {
        Self {
            metadata_store: MetadataFileStore::from_paths(&paths),
            vault_store: VaultStore::from_paths(&paths),
            mutation_lock: MutationLock::from_paths(&paths),
            committer: SecurePairCommitter::from_paths(&paths),
            paths,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(paths: AppPaths) -> Self {
        Self::from_paths(paths)
    }

    #[cfg(test)]
    pub(crate) fn for_test_with_commit_options(
        paths: AppPaths,
        options: SecureCommitTestOptions,
    ) -> Self {
        let mut repository = Self::from_paths(paths);
        repository.committer = repository.committer.with_test_options(options);
        repository
    }

    pub(crate) async fn validate_startup_state(
        &self,
    ) -> Result<RepositoryFormat, AccountRepositoryError> {
        self.with_snapshot(|snapshot| Ok(snapshot.format())).await
    }

    pub(crate) async fn export_accounts_snapshot(
        &self,
    ) -> Result<AccountExportSnapshot, AccountRepositoryError> {
        let snapshot = {
            let _guard = self
                .mutation_lock
                .acquire()
                .await
                .map_err(|_| AccountRepositoryError::LockFailed)?;

            match self.detect_locked()? {
                ValidatedSnapshot::Empty => AccountExportSnapshot::empty(),
                ValidatedSnapshot::Legacy(legacy) => AccountExportSnapshot::from_legacy(legacy),
                ValidatedSnapshot::Secure { metadata, vault } => {
                    AccountExportSnapshot::from_secure(metadata, vault)?
                }
            }
        };

        Ok(snapshot)
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
            ValidatedSnapshot::Secure { metadata, .. } => {
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
            ValidatedSnapshot::Secure { metadata, .. } => {
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
            ValidatedSnapshot::Secure { metadata, .. } => Ok(metadata.masked_account_ids.clone()),
        })
        .await
    }

    pub(crate) async fn add_account(
        &self,
        input: SecureAccountInsert,
    ) -> Result<AccountInfo, AccountRepositoryError> {
        let account_id = input.metadata.id.clone();

        self.mutate_secure(move |metadata, vault| {
            let SecureAccountInsert {
                metadata: account,
                secret,
            } = input;

            if metadata
                .accounts
                .iter()
                .any(|existing| existing.id == account.id)
            {
                return Err(AccountRepositoryError::DuplicateAccountId);
            }
            if metadata
                .accounts
                .iter()
                .any(|existing| existing.vault_ref == account.vault_ref)
            {
                return Err(AccountRepositoryError::InvalidAccountData);
            }
            if metadata
                .accounts
                .iter()
                .any(|existing| existing.display_name == account.display_name)
            {
                return Err(AccountRepositoryError::DuplicateDisplayName);
            }
            if !auth_kind_matches(&account.auth_kind, &secret) {
                return Err(AccountRepositoryError::AuthKindMismatch);
            }
            if vault.contains(&account.id) {
                return Err(AccountRepositoryError::DuplicateAccountId);
            }

            vault
                .insert(&account.id, secret)
                .map_err(|_| AccountRepositoryError::InvalidAccountData)?;
            metadata
                .insert(account)
                .map_err(map_metadata_mutation_error)?;

            if metadata.accounts.len() == 1 {
                metadata.active_account_id = Some(account_id.clone());
            }

            let account = metadata
                .get(&account_id)
                .ok_or(AccountRepositoryError::InvalidAccountData)?;
            let active_id = metadata.active_account_id.as_deref();
            Ok(MutationOutcome {
                value: account_info_from_metadata(account, active_id),
                changed: true,
            })
        })
        .await
    }

    pub(crate) async fn remove_account(
        &self,
        account_id: &str,
    ) -> Result<(), AccountRepositoryError> {
        self.mutate_secure(|metadata, vault| {
            if metadata.get(account_id).is_none() {
                return Err(AccountRepositoryError::AccountNotFound);
            }
            if !vault.contains(account_id) {
                return Err(AccountRepositoryError::SecureStateInconsistent);
            }

            let was_active = metadata.active_account_id.as_deref() == Some(account_id);
            vault
                .remove(account_id)
                .ok_or(AccountRepositoryError::SecureStateInconsistent)?;
            metadata
                .remove(account_id)
                .ok_or(AccountRepositoryError::AccountNotFound)?;

            if was_active {
                metadata.active_account_id =
                    metadata.accounts.first().map(|account| account.id.clone());
            }

            Ok(MutationOutcome {
                value: (),
                changed: true,
            })
        })
        .await
    }

    pub(crate) async fn update_account_metadata(
        &self,
        account_id: &str,
        patch: AccountMetadataPatch,
    ) -> Result<AccountInfo, AccountRepositoryError> {
        self.mutate_secure(|metadata, _vault| {
            if metadata.get(account_id).is_none() {
                return Err(AccountRepositoryError::AccountNotFound);
            }

            if let Some(display_name) = patch.display_name.as_deref() {
                if metadata.accounts.iter().any(|other| {
                    other.id != account_id && other.display_name.as_str() == display_name
                }) {
                    return Err(AccountRepositoryError::DuplicateDisplayName);
                }
            }

            let mut changed = false;
            let account = metadata
                .get_mut(account_id)
                .ok_or(AccountRepositoryError::AccountNotFound)?;

            if let Some(display_name) = patch.display_name {
                if account.display_name != display_name {
                    account.display_name = display_name;
                    changed = true;
                }
            }
            if let Some(email) = patch.email {
                if account.email != email {
                    account.email = email;
                    changed = true;
                }
            }
            if let Some(plan_type) = patch.plan_type {
                if account.plan_type != plan_type {
                    account.plan_type = plan_type;
                    changed = true;
                }
            }
            if let Some(subscription_expires_at) = patch.subscription_expires_at {
                if account.subscription_expires_at != subscription_expires_at {
                    account.subscription_expires_at = subscription_expires_at;
                    changed = true;
                }
            }

            metadata
                .validate()
                .map_err(|_| AccountRepositoryError::InvalidAccountData)?;
            let account = metadata
                .get(account_id)
                .ok_or(AccountRepositoryError::AccountNotFound)?;
            let active_id = metadata.active_account_id.as_deref();
            Ok(MutationOutcome {
                value: account_info_from_metadata(account, active_id),
                changed,
            })
        })
        .await
    }

    pub(crate) async fn set_active_account(
        &self,
        account_id: &str,
    ) -> Result<(), AccountRepositoryError> {
        self.mutate_secure(|metadata, _vault| {
            if metadata.get(account_id).is_none() {
                return Err(AccountRepositoryError::AccountNotFound);
            }
            if metadata.active_account_id.as_deref() == Some(account_id) {
                return Ok(MutationOutcome {
                    value: (),
                    changed: false,
                });
            }

            metadata.active_account_id = Some(account_id.to_string());
            Ok(MutationOutcome {
                value: (),
                changed: true,
            })
        })
        .await
    }

    pub(crate) async fn touch_account(
        &self,
        account_id: &str,
        touched_at: DateTime<Utc>,
    ) -> Result<(), AccountRepositoryError> {
        self.mutate_secure(|metadata, _vault| {
            let account = metadata
                .get_mut(account_id)
                .ok_or(AccountRepositoryError::AccountNotFound)?;
            if account.last_used_at == Some(touched_at) {
                return Ok(MutationOutcome {
                    value: (),
                    changed: false,
                });
            }

            account.last_used_at = Some(touched_at);
            Ok(MutationOutcome {
                value: (),
                changed: true,
            })
        })
        .await
    }

    pub(crate) async fn set_masked_account_ids(
        &self,
        ids: Vec<String>,
    ) -> Result<(), AccountRepositoryError> {
        self.mutate_secure(|metadata, _vault| {
            if metadata.masked_account_ids == ids {
                return Ok(MutationOutcome {
                    value: (),
                    changed: false,
                });
            }

            metadata.masked_account_ids = ids;
            Ok(MutationOutcome {
                value: (),
                changed: true,
            })
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

    async fn mutate_secure<T, F>(&self, mutation: F) -> Result<T, AccountRepositoryError>
    where
        F: FnOnce(
            &mut MetadataStoreV2,
            &mut VaultPayloadV1,
        ) -> Result<MutationOutcome<T>, AccountRepositoryError>,
    {
        let guard = self
            .mutation_lock
            .acquire()
            .await
            .map_err(|_| AccountRepositoryError::LockFailed)?;
        let mut state = self.load_mutable_secure_state_locked()?;
        let outcome = mutation(&mut state.metadata, &mut state.vault)?;

        if outcome.changed {
            self.committer
                .commit(&guard, &state.vault, &state.metadata)
                .map_err(map_commit_error)?;
        }

        Ok(outcome.value)
    }

    fn load_mutable_secure_state_locked(
        &self,
    ) -> Result<MutableSecureState, AccountRepositoryError> {
        match self.detect_locked()? {
            ValidatedSnapshot::Empty => Ok(MutableSecureState {
                metadata: MetadataStoreV2::new_empty(),
                vault: VaultPayloadV1::new_empty(),
            }),
            ValidatedSnapshot::Legacy(legacy) => {
                drop(legacy);
                Err(AccountRepositoryError::LegacyMigrationRequired)
            }
            ValidatedSnapshot::Secure { metadata, vault } => {
                Ok(MutableSecureState { metadata, vault })
            }
        }
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
                let vault = self.validate_secure_state(&metadata, vault_exists)?;
                Ok(ValidatedSnapshot::Secure { metadata, vault })
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
    ) -> Result<VaultPayloadV1, AccountRepositoryError> {
        if metadata.is_empty() {
            if !vault_exists {
                return Ok(VaultPayloadV1::new_empty());
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
            .map_err(|_| AccountRepositoryError::SecureStateInconsistent)?;
        Ok(vault)
    }
}

struct MutationOutcome<T> {
    value: T,
    changed: bool,
}

struct MutableSecureState {
    metadata: MetadataStoreV2,
    vault: VaultPayloadV1,
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

fn auth_kind_matches(auth_kind: &MetadataAuthKind, secret: &SecretRecord) -> bool {
    matches!(
        (auth_kind, secret),
        (MetadataAuthKind::ChatGpt, SecretRecord::ChatGpt { .. })
            | (MetadataAuthKind::ApiKey, SecretRecord::ApiKey { .. })
    )
}

fn secret_into_auth_data(mut secret: SecretRecord) -> AuthData {
    match &mut secret {
        SecretRecord::ApiKey { key } => AuthData::ApiKey {
            key: std::mem::take(key),
        },
        SecretRecord::ChatGpt {
            id_token,
            access_token,
            refresh_token,
            account_id,
        } => AuthData::ChatGPT {
            id_token: std::mem::take(id_token),
            access_token: std::mem::take(access_token),
            refresh_token: std::mem::take(refresh_token),
            account_id: account_id.take(),
        },
    }
}

fn map_metadata_mutation_error(error: MetadataStoreError) -> AccountRepositoryError {
    match error {
        MetadataStoreError::DuplicateAccountId => AccountRepositoryError::DuplicateAccountId,
        _ => AccountRepositoryError::InvalidAccountData,
    }
}

fn map_commit_error(error: SecureCommitError) -> AccountRepositoryError {
    match error {
        SecureCommitError::CriticalRollbackFailed => {
            AccountRepositoryError::CriticalMutationRollbackFailed
        }
        _ => AccountRepositoryError::MutationCommitFailed,
    }
}

enum ValidatedSnapshot {
    Empty,
    Legacy(LegacyStoreGuard),
    Secure {
        metadata: MetadataStoreV2,
        vault: VaultPayloadV1,
    },
}

impl ValidatedSnapshot {
    fn format(&self) -> RepositoryFormat {
        match self {
            Self::Empty => RepositoryFormat::Empty,
            Self::Legacy(_) => RepositoryFormat::Legacy,
            Self::Secure { .. } => RepositoryFormat::Secure,
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

    fn into_store(mut self) -> AccountsStore {
        std::mem::take(&mut self.store)
    }
}

impl Drop for LegacyStoreGuard {
    fn drop(&mut self) {
        zeroize_account_store(&mut self.store);
    }
}

fn zeroize_account_store(store: &mut AccountsStore) {
    for account in &mut store.accounts {
        zeroize_auth_data(&mut account.auth_data);
    }
}

fn zeroize_auth_data(auth_data: &mut AuthData) {
    match auth_data {
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
    const ACCOUNT_ID_A: &str = "account-A";
    const ACCOUNT_ID_B: &str = "account-B";
    const ACCOUNT_ID_C: &str = "account-C";
    const DISPLAY_NAME_A: &str = "Account A";
    const DISPLAY_NAME_B: &str = "Account B";

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

    fn load_secure_pair(paths: &AppPaths) -> (MetadataStoreV2, VaultPayloadV1) {
        let metadata = MetadataFileStore::from_paths(paths).load().unwrap();
        let vault = VaultStore::from_paths(paths).load().unwrap();
        (metadata, vault)
    }

    fn read_pair_bytes(paths: &AppPaths) -> (Vec<u8>, Vec<u8>) {
        (
            std::fs::read(&paths.metadata_file).unwrap(),
            std::fs::read(&paths.vault_file).unwrap(),
        )
    }

    fn chatgpt_insert(id: &str, display_name: &str, hour: u32) -> SecureAccountInsert {
        SecureAccountInsert {
            metadata: metadata_account(id, display_name, MetadataAuthKind::ChatGpt, hour),
            secret: SecretRecord::ChatGpt {
                id_token: ID_TOKEN_A.to_string(),
                access_token: ACCESS_TOKEN_A.to_string(),
                refresh_token: REFRESH_TOKEN_A.to_string(),
                account_id: Some(CHATGPT_ACCOUNT_A.to_string()),
            },
        }
    }

    fn api_key_insert(id: &str, display_name: &str, hour: u32) -> SecureAccountInsert {
        SecureAccountInsert {
            metadata: metadata_account(id, display_name, MetadataAuthKind::ApiKey, hour),
            secret: SecretRecord::ApiKey {
                key: API_KEY_A.to_string(),
            },
        }
    }

    fn assert_chatgpt_secret(vault: &VaultPayloadV1, account_id: &str) {
        match vault.get(account_id) {
            Some(SecretRecord::ChatGpt {
                id_token,
                access_token,
                refresh_token,
                account_id: stored_account_id,
            }) => {
                assert_eq!(id_token, ID_TOKEN_A);
                assert_eq!(access_token, ACCESS_TOKEN_A);
                assert_eq!(refresh_token, REFRESH_TOKEN_A);
                assert_eq!(stored_account_id.as_deref(), Some(CHATGPT_ACCOUNT_A));
            }
            _ => panic!("expected ChatGPT secret record"),
        }
    }

    fn assert_api_key_secret(vault: &VaultPayloadV1, account_id: &str) {
        match vault.get(account_id) {
            Some(SecretRecord::ApiKey { key }) => assert_eq!(key, API_KEY_A),
            _ => panic!("expected API key secret record"),
        }
    }

    fn hybrid_legacy_bytes(paths: &AppPaths) -> (Vec<u8>, Vec<u8>) {
        let legacy_bytes = serde_json::to_vec(&legacy_store()).unwrap();
        let mut hybrid: serde_json::Value = serde_json::from_slice(&legacy_bytes).unwrap();
        hybrid
            .as_object_mut()
            .unwrap()
            .insert("schema_version".to_string(), serde_json::json!(2));
        let hybrid_bytes = serde_json::to_vec(&hybrid).unwrap();
        let orphan_bytes = b"opaque hybrid vault bytes".to_vec();

        ensure_switcher_dir(paths);
        std::fs::write(&paths.metadata_file, &hybrid_bytes).unwrap();
        std::fs::write(&paths.vault_file, &orphan_bytes).unwrap();
        (hybrid_bytes, orphan_bytes)
    }

    fn assert_sanitized(message: &str) {
        for secret in [
            ID_TOKEN_A,
            ACCESS_TOKEN_A,
            REFRESH_TOKEN_A,
            API_KEY_A,
            CHATGPT_ACCOUNT_A,
            ACCOUNT_ID_A,
            ACCOUNT_ID_B,
            DISPLAY_NAME_A,
            DISPLAY_NAME_B,
            "account-a@example.test",
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

    #[tokio::test]
    async fn test_add_first_account_to_empty_creates_valid_secure_pair() {
        let (root, paths) = test_paths("add_first_empty");
        let repository = AccountRepository::for_test(paths.clone());

        let info = repository
            .add_account(chatgpt_insert(ACCOUNT_ID_A, DISPLAY_NAME_A, 1))
            .await
            .unwrap();

        assert_eq!(info.id, ACCOUNT_ID_A);
        assert!(info.is_active);
        assert!(paths.metadata_file.exists());
        assert!(paths.vault_file.exists());

        let (metadata, vault) = load_secure_pair(&paths);
        assert_eq!(metadata.accounts.len(), 1);
        assert_eq!(metadata.active_account_id.as_deref(), Some(ACCOUNT_ID_A));
        assert_chatgpt_secret(&vault, ACCOUNT_ID_A);

        drop(info);
        drop(repository);
        drop(metadata);
        drop(vault);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_first_account_becomes_active() {
        let (root, paths) = test_paths("add_first_active");
        let repository = AccountRepository::for_test(paths.clone());

        let info = repository
            .add_account(api_key_insert(ACCOUNT_ID_A, DISPLAY_NAME_A, 1))
            .await
            .unwrap();

        assert!(info.is_active);
        let (metadata, vault) = load_secure_pair(&paths);
        assert_eq!(metadata.active_account_id.as_deref(), Some(ACCOUNT_ID_A));
        assert_api_key_secret(&vault, ACCOUNT_ID_A);

        drop(info);
        drop(repository);
        drop(metadata);
        drop(vault);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_second_account_preserves_first_as_active() {
        let (root, paths) = test_paths("add_second_active");
        let repository = AccountRepository::for_test(paths.clone());

        repository
            .add_account(chatgpt_insert(ACCOUNT_ID_A, DISPLAY_NAME_A, 1))
            .await
            .unwrap();
        let second = repository
            .add_account(api_key_insert(ACCOUNT_ID_B, DISPLAY_NAME_B, 3))
            .await
            .unwrap();

        assert!(!second.is_active);
        let (metadata, vault) = load_secure_pair(&paths);
        assert_eq!(metadata.active_account_id.as_deref(), Some(ACCOUNT_ID_A));
        assert_chatgpt_secret(&vault, ACCOUNT_ID_A);
        assert_api_key_secret(&vault, ACCOUNT_ID_B);

        drop(second);
        drop(repository);
        drop(metadata);
        drop(vault);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_account_ordering_is_preserved() {
        let (root, paths) = test_paths("add_ordering");
        let repository = AccountRepository::for_test(paths.clone());

        repository
            .add_account(chatgpt_insert(ACCOUNT_ID_A, DISPLAY_NAME_A, 1))
            .await
            .unwrap();
        repository
            .add_account(api_key_insert(ACCOUNT_ID_B, DISPLAY_NAME_B, 3))
            .await
            .unwrap();
        repository
            .add_account(chatgpt_insert(ACCOUNT_ID_C, "Account C", 5))
            .await
            .unwrap();

        let (metadata, vault) = load_secure_pair(&paths);
        assert_eq!(
            metadata
                .accounts
                .iter()
                .map(|account| account.id.as_str())
                .collect::<Vec<_>>(),
            vec![ACCOUNT_ID_A, ACCOUNT_ID_B, ACCOUNT_ID_C]
        );
        assert_chatgpt_secret(&vault, ACCOUNT_ID_A);
        assert_api_key_secret(&vault, ACCOUNT_ID_B);
        assert_chatgpt_secret(&vault, ACCOUNT_ID_C);

        drop(repository);
        drop(metadata);
        drop(vault);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_duplicate_account_id_rejected_without_byte_changes() {
        let (root, paths) = test_paths("add_duplicate_id");
        let repository = AccountRepository::for_test(paths.clone());
        repository
            .add_account(chatgpt_insert(ACCOUNT_ID_A, DISPLAY_NAME_A, 1))
            .await
            .unwrap();
        let before = read_pair_bytes(&paths);

        let result = repository
            .add_account(api_key_insert(ACCOUNT_ID_A, DISPLAY_NAME_B, 3))
            .await;

        assert!(matches!(
            result,
            Err(AccountRepositoryError::DuplicateAccountId)
        ));
        assert_eq!(read_pair_bytes(&paths), before);

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_duplicate_display_name_rejected_without_byte_changes() {
        let (root, paths) = test_paths("add_duplicate_name");
        let repository = AccountRepository::for_test(paths.clone());
        repository
            .add_account(chatgpt_insert(ACCOUNT_ID_A, DISPLAY_NAME_A, 1))
            .await
            .unwrap();
        let before = read_pair_bytes(&paths);

        let result = repository
            .add_account(api_key_insert(ACCOUNT_ID_B, DISPLAY_NAME_A, 3))
            .await;

        assert!(matches!(
            result,
            Err(AccountRepositoryError::DuplicateDisplayName)
        ));
        assert_eq!(read_pair_bytes(&paths), before);

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_auth_kind_secret_mismatch_rejected_without_byte_changes() {
        let (root, paths) = test_paths("add_auth_mismatch");
        let repository = AccountRepository::for_test(paths.clone());
        let input = SecureAccountInsert {
            metadata: metadata_account(ACCOUNT_ID_A, DISPLAY_NAME_A, MetadataAuthKind::ChatGpt, 1),
            secret: SecretRecord::ApiKey {
                key: API_KEY_A.to_string(),
            },
        };

        let result = repository.add_account(input).await;

        assert!(matches!(
            result,
            Err(AccountRepositoryError::AuthKindMismatch)
        ));
        assert!(!paths.metadata_file.exists());
        assert!(!paths.vault_file.exists());

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_invalid_vault_ref_rejected_without_byte_changes() {
        let (root, paths) = test_paths("add_invalid_vault_ref");
        let repository = AccountRepository::for_test(paths.clone());
        let mut input = chatgpt_insert(ACCOUNT_ID_A, DISPLAY_NAME_A, 1);
        input.metadata.vault_ref = "different-vault-ref".to_string();

        let result = repository.add_account(input).await;

        assert!(matches!(
            result,
            Err(AccountRepositoryError::InvalidAccountData)
        ));
        assert!(!paths.metadata_file.exists());
        assert!(!paths.vault_file.exists());

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_add_to_legacy_returns_legacy_migration_required() {
        let (root, paths) = test_paths("add_legacy");
        let before = write_legacy(&paths, &legacy_store());
        let repository = AccountRepository::for_test(paths.clone());

        let result = repository
            .add_account(chatgpt_insert(ACCOUNT_ID_A, DISPLAY_NAME_A, 1))
            .await;

        assert!(matches!(
            result,
            Err(AccountRepositoryError::LegacyMigrationRequired)
        ));
        assert_eq!(std::fs::read(&paths.metadata_file).unwrap(), before);
        assert!(!paths.vault_file.exists());

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_add_to_legacy_preserves_legacy_and_orphan_vault_bytes() {
        let (root, paths) = test_paths("add_legacy_orphan");
        let metadata_before = write_legacy(&paths, &legacy_store());
        let vault_before = b"opaque legacy orphan vault".to_vec();
        std::fs::write(&paths.vault_file, &vault_before).unwrap();
        let repository = AccountRepository::for_test(paths.clone());

        let result = repository
            .add_account(api_key_insert(ACCOUNT_ID_A, DISPLAY_NAME_A, 1))
            .await;

        assert!(matches!(
            result,
            Err(AccountRepositoryError::LegacyMigrationRequired)
        ));
        assert_eq!(
            std::fs::read(&paths.metadata_file).unwrap(),
            metadata_before
        );
        assert_eq!(std::fs::read(&paths.vault_file).unwrap(), vault_before);

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_remove_non_active_account_preserves_active_account() {
        let (root, paths) = test_paths("remove_non_active");
        write_secure_pair(&paths);
        let repository = AccountRepository::for_test(paths.clone());

        repository.remove_account("secure-chatgpt-A").await.unwrap();

        let (metadata, vault) = load_secure_pair(&paths);
        assert_eq!(metadata.active_account_id.as_deref(), Some("secure-api-B"));
        assert_eq!(metadata.accounts.len(), 1);
        assert_eq!(metadata.accounts[0].id, "secure-api-B");
        assert!(!vault.contains("secure-chatgpt-A"));
        assert_api_key_secret(&vault, "secure-api-B");

        drop(repository);
        drop(metadata);
        drop(vault);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_remove_active_account_selects_first_remaining_account() {
        let (root, paths) = test_paths("remove_active");
        write_secure_pair(&paths);
        let repository = AccountRepository::for_test(paths.clone());

        repository.remove_account("secure-api-B").await.unwrap();

        let (metadata, vault) = load_secure_pair(&paths);
        assert_eq!(
            metadata.active_account_id.as_deref(),
            Some("secure-chatgpt-A")
        );
        assert_eq!(metadata.accounts.len(), 1);
        assert_chatgpt_secret(&vault, "secure-chatgpt-A");
        assert!(!vault.contains("secure-api-B"));

        drop(repository);
        drop(metadata);
        drop(vault);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_remove_final_account_creates_valid_empty_secure_pair() {
        let (root, paths) = test_paths("remove_final");
        write_secure_pair(&paths);
        let repository = AccountRepository::for_test(paths.clone());

        repository.remove_account("secure-chatgpt-A").await.unwrap();
        repository.remove_account("secure-api-B").await.unwrap();

        let (metadata, vault) = load_secure_pair(&paths);
        assert!(metadata.is_empty());
        assert!(metadata.active_account_id.is_none());
        assert!(vault.is_empty());
        assert!(verify_consistency(&vault, &metadata).is_ok());

        drop(repository);
        drop(metadata);
        drop(vault);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_removing_account_deletes_matching_vault_record() {
        let (root, paths) = test_paths("remove_vault_record");
        write_secure_pair(&paths);
        let repository = AccountRepository::for_test(paths.clone());

        repository.remove_account("secure-chatgpt-A").await.unwrap();

        let (metadata, vault) = load_secure_pair(&paths);
        assert!(!metadata
            .accounts
            .iter()
            .any(|account| account.id == "secure-chatgpt-A"));
        assert!(!vault.contains("secure-chatgpt-A"));
        assert!(vault.contains("secure-api-B"));

        drop(repository);
        drop(metadata);
        drop(vault);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_missing_account_returns_account_not_found_without_byte_changes() {
        let (root, paths) = test_paths("remove_missing");
        write_secure_pair(&paths);
        let repository = AccountRepository::for_test(paths.clone());
        let before = read_pair_bytes(&paths);

        let result = repository.remove_account("missing-account").await;

        assert!(matches!(
            result,
            Err(AccountRepositoryError::AccountNotFound)
        ));
        assert_eq!(read_pair_bytes(&paths), before);

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_removal_preserves_masked_ids_exactly() {
        let (root, paths) = test_paths("remove_masked_ids");
        write_secure_pair(&paths);
        let before = secure_metadata_store().masked_account_ids.clone();
        let repository = AccountRepository::for_test(paths.clone());

        repository.remove_account("secure-chatgpt-A").await.unwrap();

        let (metadata, vault) = load_secure_pair(&paths);
        assert_eq!(metadata.masked_account_ids, before);

        drop(repository);
        drop(metadata);
        drop(vault);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_metadata_rename_succeeds() {
        let (root, paths) = test_paths("patch_rename");
        write_secure_pair(&paths);
        let repository = AccountRepository::for_test(paths.clone());

        let info = repository
            .update_account_metadata(
                "secure-chatgpt-A",
                AccountMetadataPatch {
                    display_name: Some("Renamed ChatGPT".to_string()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        assert_eq!(info.name, "Renamed ChatGPT");
        let (metadata, vault) = load_secure_pair(&paths);
        assert_eq!(
            metadata.get("secure-chatgpt-A").unwrap().display_name,
            "Renamed ChatGPT"
        );
        assert_chatgpt_secret(&vault, "secure-chatgpt-A");

        drop(info);
        drop(repository);
        drop(metadata);
        drop(vault);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_duplicate_rename_rejected_without_byte_changes() {
        let (root, paths) = test_paths("patch_duplicate_name");
        write_secure_pair(&paths);
        let repository = AccountRepository::for_test(paths.clone());
        let before = read_pair_bytes(&paths);

        let result = repository
            .update_account_metadata(
                "secure-chatgpt-A",
                AccountMetadataPatch {
                    display_name: Some("Secure API".to_string()),
                    ..Default::default()
                },
            )
            .await;

        assert!(matches!(
            result,
            Err(AccountRepositoryError::DuplicateDisplayName)
        ));
        assert_eq!(read_pair_bytes(&paths), before);

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_metadata_email_and_plan_can_be_set() {
        let (root, paths) = test_paths("patch_set_email_plan");
        write_secure_pair(&paths);
        let repository = AccountRepository::for_test(paths.clone());

        repository
            .update_account_metadata(
                "secure-api-B",
                AccountMetadataPatch {
                    email: Some(Some("new-account@example.test".to_string())),
                    plan_type: Some(Some("team".to_string())),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let (metadata, vault) = load_secure_pair(&paths);
        let account = metadata.get("secure-api-B").unwrap();
        assert_eq!(account.email.as_deref(), Some("new-account@example.test"));
        assert_eq!(account.plan_type.as_deref(), Some("team"));
        assert_api_key_secret(&vault, "secure-api-B");

        drop(repository);
        drop(metadata);
        drop(vault);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_metadata_email_and_plan_can_be_explicitly_cleared() {
        let (root, paths) = test_paths("patch_clear_email_plan");
        write_secure_pair(&paths);
        let repository = AccountRepository::for_test(paths.clone());

        repository
            .update_account_metadata(
                "secure-chatgpt-A",
                AccountMetadataPatch {
                    email: Some(None),
                    plan_type: Some(None),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let (metadata, vault) = load_secure_pair(&paths);
        let account = metadata.get("secure-chatgpt-A").unwrap();
        assert!(account.email.is_none());
        assert!(account.plan_type.is_none());
        assert_chatgpt_secret(&vault, "secure-chatgpt-A");

        drop(repository);
        drop(metadata);
        drop(vault);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_subscription_expiry_can_be_set_and_cleared() {
        let (root, paths) = test_paths("patch_subscription_expiry");
        write_secure_pair(&paths);
        let repository = AccountRepository::for_test(paths.clone());
        let expiry = timestamp(20);

        repository
            .update_account_metadata(
                "secure-chatgpt-A",
                AccountMetadataPatch {
                    subscription_expires_at: Some(Some(expiry)),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(
            load_secure_pair(&paths)
                .0
                .get("secure-chatgpt-A")
                .unwrap()
                .subscription_expires_at,
            Some(expiry)
        );

        repository
            .update_account_metadata(
                "secure-chatgpt-A",
                AccountMetadataPatch {
                    subscription_expires_at: Some(None),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let (metadata, vault) = load_secure_pair(&paths);
        assert!(metadata
            .get("secure-chatgpt-A")
            .unwrap()
            .subscription_expires_at
            .is_none());

        drop(repository);
        drop(metadata);
        drop(vault);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_metadata_patch_preserves_immutable_fields() {
        let (root, paths) = test_paths("patch_immutable_fields");
        write_secure_pair(&paths);
        let before = load_secure_pair(&paths).0;
        let before_account = before.get("secure-chatgpt-A").unwrap();
        let before_id = before_account.id.clone();
        let before_vault_ref = before_account.vault_ref.clone();
        let before_auth_kind = before_account.auth_kind.clone();
        let before_created_at = before_account.created_at;
        let repository = AccountRepository::for_test(paths.clone());

        repository
            .update_account_metadata(
                "secure-chatgpt-A",
                AccountMetadataPatch {
                    display_name: Some("Immutable Test Rename".to_string()),
                    email: Some(Some("immutable@example.test".to_string())),
                    plan_type: Some(Some("team".to_string())),
                    subscription_expires_at: Some(Some(timestamp(21))),
                },
            )
            .await
            .unwrap();

        let (metadata, vault) = load_secure_pair(&paths);
        let account = metadata.get("secure-chatgpt-A").unwrap();
        assert_eq!(account.id, before_id);
        assert_eq!(account.vault_ref, before_vault_ref);
        assert_eq!(account.auth_kind, before_auth_kind);
        assert_eq!(account.created_at, before_created_at);

        drop(repository);
        drop(before);
        drop(metadata);
        drop(vault);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_metadata_patch_preserves_vault_secret_semantically() {
        let (root, paths) = test_paths("patch_secret_preserved");
        write_secure_pair(&paths);
        let (_, before_vault) = load_secure_pair(&paths);
        let repository = AccountRepository::for_test(paths.clone());

        repository
            .update_account_metadata(
                "secure-chatgpt-A",
                AccountMetadataPatch {
                    display_name: Some("Secret Preservation Rename".to_string()),
                    email: Some(None),
                    plan_type: Some(Some("team".to_string())),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let (_, after_vault) = load_secure_pair(&paths);
        assert_chatgpt_secret(&before_vault, "secure-chatgpt-A");
        assert_api_key_secret(&before_vault, "secure-api-B");
        assert_chatgpt_secret(&after_vault, "secure-chatgpt-A");
        assert_api_key_secret(&after_vault, "secure-api-B");

        drop(repository);
        drop(before_vault);
        drop(after_vault);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_empty_metadata_patch_is_byte_for_byte_no_op() {
        let (root, paths) = test_paths("patch_empty_noop");
        write_secure_pair(&paths);
        let repository = AccountRepository::for_test(paths.clone());
        let before = read_pair_bytes(&paths);

        let info = repository
            .update_account_metadata("secure-chatgpt-A", AccountMetadataPatch::default())
            .await
            .unwrap();

        assert_eq!(info.name, "Secure ChatGPT");
        assert_eq!(read_pair_bytes(&paths), before);

        drop(info);
        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_identical_value_metadata_patch_is_byte_for_byte_no_op() {
        let (root, paths) = test_paths("patch_identical_noop");
        write_secure_pair(&paths);
        let repository = AccountRepository::for_test(paths.clone());
        let before = read_pair_bytes(&paths);

        let info = repository
            .update_account_metadata(
                "secure-chatgpt-A",
                AccountMetadataPatch {
                    display_name: Some("Secure ChatGPT".to_string()),
                    email: Some(Some("secure-chatgpt-A@example.test".to_string())),
                    plan_type: Some(Some("pro".to_string())),
                    subscription_expires_at: Some(None),
                },
            )
            .await
            .unwrap();

        assert_eq!(info.name, "Secure ChatGPT");
        assert_eq!(read_pair_bytes(&paths), before);

        drop(info);
        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_setting_active_account_succeeds() {
        let (root, paths) = test_paths("active_set");
        write_secure_pair(&paths);
        let repository = AccountRepository::for_test(paths.clone());

        repository
            .set_active_account("secure-chatgpt-A")
            .await
            .unwrap();

        let (metadata, vault) = load_secure_pair(&paths);
        assert_eq!(
            metadata.active_account_id.as_deref(),
            Some("secure-chatgpt-A")
        );
        assert_chatgpt_secret(&vault, "secure-chatgpt-A");

        drop(repository);
        drop(metadata);
        drop(vault);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_setting_already_active_account_is_byte_for_byte_no_op() {
        let (root, paths) = test_paths("active_noop");
        write_secure_pair(&paths);
        let repository = AccountRepository::for_test(paths.clone());
        let before = read_pair_bytes(&paths);

        repository.set_active_account("secure-api-B").await.unwrap();

        assert_eq!(read_pair_bytes(&paths), before);

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_unknown_active_account_fails_without_byte_changes() {
        let (root, paths) = test_paths("active_unknown");
        write_secure_pair(&paths);
        let repository = AccountRepository::for_test(paths.clone());
        let before = read_pair_bytes(&paths);

        let result = repository.set_active_account("missing-account").await;

        assert!(matches!(
            result,
            Err(AccountRepositoryError::AccountNotFound)
        ));
        assert_eq!(read_pair_bytes(&paths), before);

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_touch_stores_caller_supplied_timestamp_exactly() {
        let (root, paths) = test_paths("touch_exact");
        write_secure_pair(&paths);
        let repository = AccountRepository::for_test(paths.clone());
        let touched_at = timestamp(22);

        repository
            .touch_account("secure-chatgpt-A", touched_at)
            .await
            .unwrap();

        let (metadata, vault) = load_secure_pair(&paths);
        assert_eq!(
            metadata.get("secure-chatgpt-A").unwrap().last_used_at,
            Some(touched_at)
        );

        drop(repository);
        drop(metadata);
        drop(vault);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_identical_touch_timestamp_is_byte_for_byte_no_op() {
        let (root, paths) = test_paths("touch_noop");
        write_secure_pair(&paths);
        let repository = AccountRepository::for_test(paths.clone());
        let before = read_pair_bytes(&paths);

        repository
            .touch_account("secure-chatgpt-A", timestamp(6))
            .await
            .unwrap();

        assert_eq!(read_pair_bytes(&paths), before);

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_masked_ids_preserve_ordering_stale_ids_and_duplicates() {
        let (root, paths) = test_paths("masked_mutation");
        write_secure_pair(&paths);
        let repository = AccountRepository::for_test(paths.clone());
        let ids = vec![
            "stale-one".to_string(),
            ACCOUNT_ID_A.to_string(),
            "stale-one".to_string(),
            ACCOUNT_ID_A.to_string(),
        ];

        repository
            .set_masked_account_ids(ids.clone())
            .await
            .unwrap();

        let (metadata, vault) = load_secure_pair(&paths);
        assert_eq!(metadata.masked_account_ids, ids);

        drop(repository);
        drop(metadata);
        drop(vault);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_identical_masked_vector_is_byte_for_byte_no_op() {
        let (root, paths) = test_paths("masked_noop");
        write_secure_pair(&paths);
        let repository = AccountRepository::for_test(paths.clone());
        let ids = secure_metadata_store().masked_account_ids.clone();
        let before = read_pair_bytes(&paths);

        repository.set_masked_account_ids(ids).await.unwrap();

        assert_eq!(read_pair_bytes(&paths), before);

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_setting_masked_ids_on_empty_creates_valid_empty_secure_pair() {
        let (root, paths) = test_paths("masked_empty");
        let repository = AccountRepository::for_test(paths.clone());
        let ids = vec!["stale-empty-id".to_string(), "stale-empty-id".to_string()];

        repository
            .set_masked_account_ids(ids.clone())
            .await
            .unwrap();

        let (metadata, vault) = load_secure_pair(&paths);
        assert!(metadata.is_empty());
        assert_eq!(metadata.masked_account_ids, ids);
        assert!(vault.is_empty());
        assert!(verify_consistency(&vault, &metadata).is_ok());

        drop(repository);
        drop(metadata);
        drop(vault);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_fail_before_vault_install_preserves_prior_pair_exactly() {
        let (root, paths) = test_paths("mutation_fail_before_vault");
        write_secure_pair(&paths);
        let before = read_pair_bytes(&paths);
        let repository = AccountRepository::for_test_with_commit_options(
            paths.clone(),
            SecureCommitTestOptions {
                fail_before_vault_install: true,
                ..Default::default()
            },
        );

        let result = repository
            .add_account(chatgpt_insert(ACCOUNT_ID_A, DISPLAY_NAME_A, 1))
            .await;

        assert!(matches!(
            result,
            Err(AccountRepositoryError::MutationCommitFailed)
        ));
        assert_eq!(read_pair_bytes(&paths), before);

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_metadata_install_failure_restores_prior_pair_exactly() {
        let (root, paths) = test_paths("mutation_metadata_failure");
        write_secure_pair(&paths);
        let before = read_pair_bytes(&paths);
        let repository = AccountRepository::for_test_with_commit_options(
            paths.clone(),
            SecureCommitTestOptions {
                fail_metadata_install: true,
                ..Default::default()
            },
        );

        let result = repository
            .add_account(chatgpt_insert(ACCOUNT_ID_A, DISPLAY_NAME_A, 1))
            .await;

        assert!(matches!(
            result,
            Err(AccountRepositoryError::MutationCommitFailed)
        ));
        assert_eq!(read_pair_bytes(&paths), before);

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_metadata_install_failure_on_empty_leaves_no_partial_files() {
        let (root, paths) = test_paths("mutation_metadata_failure_empty");
        let repository = AccountRepository::for_test_with_commit_options(
            paths.clone(),
            SecureCommitTestOptions {
                fail_metadata_install: true,
                ..Default::default()
            },
        );

        let result = repository
            .add_account(api_key_insert(ACCOUNT_ID_A, DISPLAY_NAME_A, 1))
            .await;

        assert!(matches!(
            result,
            Err(AccountRepositoryError::MutationCommitFailed)
        ));
        assert!(!paths.metadata_file.exists());
        assert!(!paths.vault_file.exists());

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_critical_rollback_failure_maps_to_critical_mutation_rollback_failed() {
        let (root, paths) = test_paths("mutation_critical_rollback");
        write_secure_pair(&paths);
        let before_metadata = std::fs::read(&paths.metadata_file).unwrap();
        let before_vault = std::fs::read(&paths.vault_file).unwrap();
        let repository = AccountRepository::for_test_with_commit_options(
            paths.clone(),
            SecureCommitTestOptions {
                fail_metadata_install: true,
                fail_vault_rollback: true,
                ..Default::default()
            },
        );

        let result = repository
            .add_account(chatgpt_insert(ACCOUNT_ID_A, DISPLAY_NAME_A, 1))
            .await;

        assert!(matches!(
            result,
            Err(AccountRepositoryError::CriticalMutationRollbackFailed)
        ));
        assert_eq!(
            std::fs::read(&paths.metadata_file).unwrap(),
            before_metadata
        );
        assert_ne!(std::fs::read(&paths.vault_file).unwrap(), before_vault);

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_two_concurrent_repository_additions_do_not_lose_either_account() {
        let (root, paths) = test_paths("mutation_concurrent_additions");
        let first_repository = AccountRepository::for_test(paths.clone());
        let second_repository = AccountRepository::for_test(paths.clone());

        let (first_result, second_result) = tokio::join!(
            first_repository.add_account(chatgpt_insert(ACCOUNT_ID_A, DISPLAY_NAME_A, 1)),
            second_repository.add_account(api_key_insert(ACCOUNT_ID_B, DISPLAY_NAME_B, 3)),
        );

        assert!(first_result.is_ok());
        assert!(second_result.is_ok());
        let (metadata, vault) = load_secure_pair(&paths);
        assert_eq!(metadata.accounts.len(), 2);
        assert!(metadata.get(ACCOUNT_ID_A).is_some());
        assert!(metadata.get(ACCOUNT_ID_B).is_some());
        assert_chatgpt_secret(&vault, ACCOUNT_ID_A);
        assert_api_key_secret(&vault, ACCOUNT_ID_B);

        drop(first_result);
        drop(second_result);
        drop(first_repository);
        drop(second_repository);
        drop(metadata);
        drop(vault);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_every_mutation_redetects_current_state_after_locking() {
        let (root, paths) = test_paths("mutation_redetect");
        let repository = AccountRepository::for_test(paths.clone());
        let before = write_legacy(&paths, &legacy_store());

        let result = repository
            .add_account(chatgpt_insert(ACCOUNT_ID_A, DISPLAY_NAME_A, 1))
            .await;

        assert!(matches!(
            result,
            Err(AccountRepositoryError::LegacyMigrationRequired)
        ));
        assert_eq!(std::fs::read(&paths.metadata_file).unwrap(), before);

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_corrupt_secure_state_blocks_mutation_without_modification() {
        let (root, paths) = test_paths("mutation_corrupt_secure");
        let metadata_before = write_secure_metadata(&paths, &secure_metadata_store());
        let vault_before = b"corrupt secure vault bytes".to_vec();
        std::fs::write(&paths.vault_file, &vault_before).unwrap();
        let repository = AccountRepository::for_test(paths.clone());

        let result = repository
            .add_account(chatgpt_insert(ACCOUNT_ID_A, DISPLAY_NAME_A, 1))
            .await;

        assert!(matches!(
            result,
            Err(AccountRepositoryError::SecureVaultLoadFailed)
        ));
        assert_eq!(
            std::fs::read(&paths.metadata_file).unwrap(),
            metadata_before
        );
        assert_eq!(std::fs::read(&paths.vault_file).unwrap(), vault_before);

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_hybrid_discriminator_state_blocks_mutation_without_modification() {
        let (root, paths) = test_paths("mutation_hybrid");
        let (metadata_before, vault_before) = hybrid_legacy_bytes(&paths);
        let repository = AccountRepository::for_test(paths.clone());

        let result = repository
            .add_account(api_key_insert(ACCOUNT_ID_A, DISPLAY_NAME_A, 1))
            .await;

        assert!(matches!(
            result,
            Err(AccountRepositoryError::InvalidStoreFormat)
        ));
        assert_eq!(
            std::fs::read(&paths.metadata_file).unwrap(),
            metadata_before
        );
        assert_eq!(std::fs::read(&paths.vault_file).unwrap(), vault_before);

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_mutation_errors_do_not_contain_synthetic_credentials_or_pii() {
        let (root, paths) = test_paths("mutation_error_secrecy");
        let repository = AccountRepository::for_test(paths);
        let input = SecureAccountInsert {
            metadata: metadata_account(ACCOUNT_ID_A, DISPLAY_NAME_A, MetadataAuthKind::ChatGpt, 1),
            secret: SecretRecord::ApiKey {
                key: API_KEY_A.to_string(),
            },
        };

        let error = repository.add_account(input).await.unwrap_err();

        assert!(matches!(error, AccountRepositoryError::AuthKindMismatch));
        assert_sanitized(&error.to_string());

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_export_empty_repository_produces_valid_empty_snapshot() {
        let (root, paths) = test_paths("export_empty_snapshot");
        let repository = AccountRepository::for_test(paths.clone());

        let snapshot = repository.export_accounts_snapshot().await.unwrap();
        let store = snapshot.store();
        assert_eq!(store.version, 1);
        assert!(store.accounts.is_empty());
        assert!(store.active_account_id.is_none());
        assert!(store.masked_account_ids.is_empty());
        let encoded = serde_json::to_vec(store).unwrap();
        let decoded: AccountsStore = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded.version, 1);
        assert!(decoded.accounts.is_empty());
        assert!(decoded.active_account_id.is_none());
        assert!(decoded.masked_account_ids.is_empty());

        drop(snapshot);
        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_export_secure_api_key_account_reconstructs_exactly() {
        let (root, paths) = test_paths("export_api_key");
        write_secure_pair(&paths);
        let repository = AccountRepository::for_test(paths.clone());

        let snapshot = repository.export_accounts_snapshot().await.unwrap();
        let account = snapshot
            .store()
            .accounts
            .iter()
            .find(|account| account.id == "secure-api-B")
            .unwrap();
        assert_eq!(account.name, "Secure API");
        assert_eq!(account.email.as_deref(), Some("secure-api-B@example.test"));
        assert_eq!(account.plan_type.as_deref(), Some("pro"));
        assert_eq!(account.auth_mode, AuthMode::ApiKey);
        assert_eq!(account.created_at, timestamp(7));
        assert_eq!(account.last_used_at, Some(timestamp(8)));
        match &account.auth_data {
            AuthData::ApiKey { key } => assert_eq!(key, API_KEY_A),
            AuthData::ChatGPT { .. } => panic!("expected API key export record"),
        }

        drop(snapshot);
        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_export_secure_chatgpt_account_reconstructs_exactly() {
        let (root, paths) = test_paths("export_chatgpt");
        write_secure_pair(&paths);
        let repository = AccountRepository::for_test(paths.clone());

        let snapshot = repository.export_accounts_snapshot().await.unwrap();
        let account = snapshot
            .store()
            .accounts
            .iter()
            .find(|account| account.id == "secure-chatgpt-A")
            .unwrap();
        assert_eq!(account.name, "Secure ChatGPT");
        assert_eq!(
            account.email.as_deref(),
            Some("secure-chatgpt-A@example.test")
        );
        assert_eq!(account.plan_type.as_deref(), Some("pro"));
        assert_eq!(account.auth_mode, AuthMode::ChatGPT);
        assert_eq!(account.created_at, timestamp(5));
        assert_eq!(account.last_used_at, Some(timestamp(6)));
        match &account.auth_data {
            AuthData::ChatGPT {
                id_token,
                access_token,
                refresh_token,
                account_id,
            } => {
                assert_eq!(id_token, ID_TOKEN_A);
                assert_eq!(access_token, ACCESS_TOKEN_A);
                assert_eq!(refresh_token, REFRESH_TOKEN_A);
                assert_eq!(account_id.as_deref(), Some(CHATGPT_ACCOUNT_A));
            }
            AuthData::ApiKey { .. } => panic!("expected ChatGPT export record"),
        }

        drop(snapshot);
        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_export_secure_metadata_ordering_is_preserved() {
        let (root, paths) = test_paths("export_ordering");
        write_secure_pair(&paths);
        let repository = AccountRepository::for_test(paths.clone());

        let snapshot = repository.export_accounts_snapshot().await.unwrap();
        let ids: Vec<_> = snapshot
            .store()
            .accounts
            .iter()
            .map(|account| account.id.as_str())
            .collect();
        assert_eq!(ids, vec!["secure-chatgpt-A", "secure-api-B"]);

        drop(snapshot);
        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_export_secure_active_account_is_preserved() {
        let (root, paths) = test_paths("export_active");
        write_secure_pair(&paths);
        let repository = AccountRepository::for_test(paths.clone());

        let snapshot = repository.export_accounts_snapshot().await.unwrap();
        assert_eq!(
            snapshot.store().active_account_id.as_deref(),
            Some("secure-api-B")
        );

        drop(snapshot);
        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_export_secure_masked_account_ids_are_preserved() {
        let (root, paths) = test_paths("export_masked");
        write_secure_pair(&paths);
        let repository = AccountRepository::for_test(paths.clone());

        let snapshot = repository.export_accounts_snapshot().await.unwrap();
        assert_eq!(
            snapshot.store().masked_account_ids,
            vec![
                "stale-secure-mask".to_string(),
                "secure-chatgpt-A".to_string(),
                "stale-secure-mask".to_string(),
            ]
        );

        drop(snapshot);
        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_export_secure_optional_metadata_and_timestamps_are_preserved() {
        let (root, paths) = test_paths("export_optional_metadata");
        let expected = secure_metadata_store();
        write_secure_pair(&paths);
        let repository = AccountRepository::for_test(paths.clone());

        let snapshot = repository.export_accounts_snapshot().await.unwrap();
        for expected_account in &expected.accounts {
            let actual = snapshot
                .store()
                .accounts
                .iter()
                .find(|account| account.id == expected_account.id)
                .unwrap();
            assert_eq!(actual.id, expected_account.id);
            assert_eq!(actual.name, expected_account.display_name);
            assert_eq!(actual.email, expected_account.email);
            assert_eq!(actual.plan_type, expected_account.plan_type);
            assert_eq!(
                actual.subscription_expires_at,
                expected_account.subscription_expires_at
            );
            assert_eq!(actual.created_at, expected_account.created_at);
            assert_eq!(actual.last_used_at, expected_account.last_used_at);
        }

        drop(snapshot);
        drop(expected);
        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_export_legacy_repository_is_semantic_and_read_only() {
        let (root, paths) = test_paths("export_legacy_semantic");
        let mut expected = legacy_store();
        let metadata_before = write_legacy(&paths, &expected);
        let repository = AccountRepository::for_test(paths.clone());

        let snapshot = repository.export_accounts_snapshot().await.unwrap();
        assert_eq!(snapshot.store().version, expected.version);
        assert_eq!(
            snapshot.store().active_account_id,
            expected.active_account_id
        );
        assert_eq!(
            snapshot.store().masked_account_ids,
            expected.masked_account_ids
        );
        assert_eq!(snapshot.store().accounts.len(), expected.accounts.len());
        for (actual, expected_account) in snapshot.store().accounts.iter().zip(&expected.accounts) {
            assert_eq!(actual.id, expected_account.id);
            assert_eq!(actual.name, expected_account.name);
            assert_eq!(actual.email, expected_account.email);
            assert_eq!(actual.plan_type, expected_account.plan_type);
            assert_eq!(
                actual.subscription_expires_at,
                expected_account.subscription_expires_at
            );
            assert_eq!(actual.auth_mode, expected_account.auth_mode);
            assert_eq!(actual.created_at, expected_account.created_at);
            assert_eq!(actual.last_used_at, expected_account.last_used_at);
        }
        assert_eq!(
            std::fs::read(&paths.metadata_file).unwrap(),
            metadata_before
        );
        assert!(!paths.vault_file.exists());

        drop(snapshot);
        zeroize_account_store(&mut expected);
        drop(expected);
        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_export_legacy_repository_preserves_metadata_bytes() {
        let (root, paths) = test_paths("export_legacy_metadata_bytes");
        let mut expected = legacy_store();
        let metadata_before = write_legacy(&paths, &expected);
        let repository = AccountRepository::for_test(paths.clone());

        let snapshot = repository.export_accounts_snapshot().await.unwrap();
        assert_eq!(snapshot.store().accounts.len(), expected.accounts.len());
        assert_eq!(
            std::fs::read(&paths.metadata_file).unwrap(),
            metadata_before
        );

        drop(snapshot);
        zeroize_account_store(&mut expected);
        drop(expected);
        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_export_legacy_repository_preserves_orphan_vault_bytes() {
        let (root, paths) = test_paths("export_legacy_orphan_vault");
        let mut expected = legacy_store();
        write_legacy(&paths, &expected);
        let orphan = b"opaque legacy export orphan vault".to_vec();
        std::fs::write(&paths.vault_file, &orphan).unwrap();
        let repository = AccountRepository::for_test(paths.clone());

        let snapshot = repository.export_accounts_snapshot().await.unwrap();
        assert_eq!(snapshot.store().accounts.len(), expected.accounts.len());
        assert_eq!(std::fs::read(&paths.vault_file).unwrap(), orphan);

        drop(snapshot);
        zeroize_account_store(&mut expected);
        drop(expected);
        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_export_inconsistent_secure_pair_fails_closed() {
        let (root, paths) = test_paths("export_inconsistent_secure");
        write_secure_metadata(&paths, &secure_metadata_store());
        let repository = AccountRepository::for_test(paths.clone());

        let result = repository.export_accounts_snapshot().await;
        assert!(matches!(
            result,
            Err(AccountRepositoryError::SecureStateInconsistent)
        ));

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_export_secure_auth_kind_mismatch_fails_closed() {
        let (root, paths) = test_paths("export_auth_kind_mismatch");
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
        let repository = AccountRepository::for_test(paths.clone());

        let result = repository.export_accounts_snapshot().await;
        assert!(matches!(
            result,
            Err(AccountRepositoryError::SecureStateInconsistent)
        ));

        drop(metadata);
        drop(vault);
        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_export_repository_errors_are_sanitized() {
        let (root, paths) = test_paths("export_error_sanitized");
        write_secure_metadata(&paths, &secure_metadata_store());
        std::fs::write(&paths.vault_file, b"corrupt export vault bytes").unwrap();
        let repository = AccountRepository::for_test(paths.clone());

        let error = match repository.export_accounts_snapshot().await {
            Err(error) => error.to_string(),
            Ok(snapshot) => {
                drop(snapshot);
                panic!("corrupt secure vault unexpectedly exported");
            }
        };
        assert_eq!(error, "Secure vault could not be loaded");
        assert_sanitized(&error);
        assert!(!error.contains(root.to_string_lossy().as_ref()));

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_export_partial_secure_snapshot_failure_is_zeroizing_owned() {
        let (root, paths) = test_paths("export_partial_failure");
        let metadata = secure_metadata_store();
        let mut vault = secure_vault_payload();
        vault.remove("secure-api-B");

        let result = AccountExportSnapshot::from_secure(metadata, vault);
        assert!(matches!(
            result,
            Err(AccountRepositoryError::SecureStateInconsistent)
        ));

        drop(paths);
        cleanup(root);
    }
}
