//! One-time legacy accounts.json migration engine — Phase 1B-2.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use zeroize::{Zeroize, Zeroizing};

use crate::auth::metadata_store::{
    AccountMetadataV2, MetadataAuthKind, MetadataFileStore, MetadataStoreV2,
};
use crate::auth::operation_lock::MutationLock;
use crate::auth::paths::AppPaths;
use crate::auth::secure_commit::{SecureCommitError, SecurePairCommitter};
use crate::auth::vault::{SecretRecord, VaultPayloadV1, VaultStore};

// ----- Legacy deserialization types (No Debug derive on secret types) -----------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyAccountsStore {
    version: u32,
    accounts: Vec<LegacyStoredAccount>,
    active_account_id: Option<String>,
    #[serde(default)]
    masked_account_ids: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyStoredAccount {
    id: String,
    name: String,
    email: Option<String>,
    plan_type: Option<String>,
    #[serde(default)]
    subscription_expires_at: Option<DateTime<Utc>>,
    auth_mode: LegacyAuthMode,
    auth_data: LegacyAuthData,
    created_at: DateTime<Utc>,
    last_used_at: Option<DateTime<Utc>>,
}

#[derive(Deserialize, PartialEq, Eq)]
enum LegacyAuthMode {
    #[serde(rename = "api_key")]
    ApiKey,
    #[serde(rename = "chat_g_p_t", alias = "chat_gpt")]
    ChatGpt,
}

#[derive(Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
enum LegacyAuthData {
    #[serde(rename = "api_key")]
    ApiKey { key: String },
    #[serde(rename = "chat_g_p_t", alias = "chat_gpt")]
    ChatGpt {
        id_token: String,
        access_token: String,
        refresh_token: String,
        account_id: Option<String>,
    },
}

impl LegacyAuthData {
    fn into_secret_record(mut self) -> SecretRecord {
        match &mut self {
            LegacyAuthData::ApiKey { key } => SecretRecord::ApiKey {
                key: std::mem::take(key),
            },
            LegacyAuthData::ChatGpt {
                id_token,
                access_token,
                refresh_token,
                account_id,
            } => SecretRecord::ChatGpt {
                id_token: std::mem::take(id_token),
                access_token: std::mem::take(access_token),
                refresh_token: std::mem::take(refresh_token),
                account_id: std::mem::take(account_id),
            },
        }
    }
}

impl Zeroize for LegacyAuthData {
    fn zeroize(&mut self) {
        match self {
            LegacyAuthData::ApiKey { key } => key.zeroize(),
            LegacyAuthData::ChatGpt {
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

impl Drop for LegacyAuthData {
    fn drop(&mut self) {
        self.zeroize();
    }
}

// ----- Migration outcome and errors ---------------------------------------------

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum MigrationOutcome {
    NoStore,
    AlreadySecure,
    Migrated { account_count: usize },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum MigrationError {
    #[error("Failed to acquire mutation lock")]
    LockFailed,

    #[error("Failed to read accounts.json file")]
    AccountsReadFailed,

    #[error("Target store format is invalid (neither valid legacy nor valid secure schema)")]
    InvalidStoreFormat,

    #[error("Legacy store version {0} is not supported (expected 1)")]
    UnsupportedLegacyVersion(u32),

    #[error("Legacy account ID is invalid")]
    InvalidLegacyAccountId,

    #[error("Duplicate account ID found in legacy store")]
    DuplicateLegacyAccountId,

    #[error("Legacy active_account_id is invalid or references unknown account")]
    InvalidLegacyActiveAccount,

    #[error("Legacy auth_mode and auth_data type mismatch")]
    LegacyAuthModeMismatch,

    #[error("Legacy metadata field is invalid")]
    InvalidLegacyMetadata,

    #[error("Legacy secret contains empty required fields")]
    InvalidLegacySecret,

    #[error("Encoded metadata contained legacy secret value")]
    MetadataContainsSecret,

    #[error("Secure commit transaction failed: {0}")]
    SecureCommitFailed(#[from] SecureCommitError),

    #[error("Secure state is inconsistent between metadata and vault")]
    SecureStateInconsistent,

    #[error("Final post-migration verification failed")]
    FinalVerificationFailed,
}

// ----- Migration engine ---------------------------------------------------------

pub(crate) struct LegacyMigration {
    paths: AppPaths,
    mutation_lock: MutationLock,
    committer: SecurePairCommitter,
}

impl LegacyMigration {
    pub(crate) fn from_paths(paths: AppPaths) -> Self {
        let mutation_lock = MutationLock::from_paths(&paths);
        let committer = SecurePairCommitter::from_paths(&paths);
        Self {
            paths,
            mutation_lock,
            committer,
        }
    }

    pub(crate) fn for_test(paths: AppPaths) -> Self {
        Self::from_paths(paths)
    }

    pub(crate) async fn migrate_if_needed(&self) -> Result<MigrationOutcome, MigrationError> {
        // Step 1: Acquire MutationGuard.
        let guard = self
            .mutation_lock
            .acquire()
            .await
            .map_err(|_| MigrationError::LockFailed)?;

        // Step 2 & 3: Re-read accounts.json under lock.
        let accounts_path = &self.paths.metadata_file;
        if !accounts_path.exists() {
            return Ok(MigrationOutcome::NoStore);
        }

        let raw_bytes = Zeroizing::new(
            std::fs::read(accounts_path).map_err(|_| MigrationError::AccountsReadFailed)?,
        );

        // Step 4 & 5: Attempt strict MetadataStoreV2 deserialization first.
        if let Ok(metadata_v2) = serde_json::from_slice::<MetadataStoreV2>(&raw_bytes) {
            if metadata_v2.validate().is_ok() {
                let vault_store = VaultStore::from_paths(&self.paths);
                if metadata_v2.is_empty() {
                    if vault_store.exists() {
                        let vault = vault_store
                            .load()
                            .map_err(|_| MigrationError::SecureStateInconsistent)?;
                        if !vault.is_empty() {
                            return Err(MigrationError::SecureStateInconsistent);
                        }
                    }
                } else {
                    if !vault_store.exists() {
                        return Err(MigrationError::SecureStateInconsistent);
                    }
                    let vault = vault_store
                        .load()
                        .map_err(|_| MigrationError::SecureStateInconsistent)?;
                    verify_secure_state_match(&metadata_v2, &vault)?;
                }
                return Ok(MigrationOutcome::AlreadySecure);
            }
        }

        // Step 6: Parse as exact LegacyAccountsStore.
        let legacy_store: LegacyAccountsStore =
            serde_json::from_slice(&raw_bytes).map_err(|_| MigrationError::InvalidStoreFormat)?;

        // Step 8: Validate complete legacy store.
        validate_legacy_store(&legacy_store)?;

        // Extract secret values for secrecy assertion checks
        let legacy_secrets = extract_legacy_secrets(&legacy_store);

        // Step 9 & 10 & 11: Build VaultPayloadV1 and MetadataStoreV2 in memory.
        let (vault_payload, metadata_v2) = convert_legacy_store(legacy_store)?;

        // Step 12 & 13: Encode metadata in memory and verify no legacy secrets exist.
        let metadata_file_store = MetadataFileStore::from_paths(&self.paths);
        let encoded_metadata = metadata_file_store
            .encode(&metadata_v2)
            .map_err(|_| MigrationError::InvalidLegacyMetadata)?;

        for secret_bytes in &legacy_secrets {
            if !secret_bytes.is_empty()
                && encoded_metadata
                    .windows(secret_bytes.len())
                    .any(|w| w == secret_bytes.as_slice())
            {
                return Err(MigrationError::MetadataContainsSecret);
            }
        }

        // Step 14: Commit through SecurePairCommitter while holding guard.
        self.committer
            .commit(&guard, &vault_payload, &metadata_v2)?;

        // Step 15 & 16 & 17 & 18 & 19: Final post-migration verification.
        let final_metadata_store = MetadataFileStore::from_paths(&self.paths);
        let final_vault_store = VaultStore::from_paths(&self.paths);

        let final_metadata = final_metadata_store
            .load()
            .map_err(|_| MigrationError::FinalVerificationFailed)?;
        let final_vault = final_vault_store
            .load()
            .map_err(|_| MigrationError::FinalVerificationFailed)?;

        verify_secure_state_match(&final_metadata, &final_vault)?;

        let final_raw_metadata = Zeroizing::new(
            std::fs::read(&self.paths.metadata_file)
                .map_err(|_| MigrationError::FinalVerificationFailed)?,
        );

        for secret_bytes in &legacy_secrets {
            if !secret_bytes.is_empty()
                && final_raw_metadata
                    .windows(secret_bytes.len())
                    .any(|w| w == secret_bytes.as_slice())
            {
                return Err(MigrationError::FinalVerificationFailed);
            }
        }

        Ok(MigrationOutcome::Migrated {
            account_count: final_metadata.len(),
        })
    }
}

// ----- Helpers ------------------------------------------------------------------

fn validate_legacy_store(store: &LegacyAccountsStore) -> Result<(), MigrationError> {
    if store.version != 1 {
        return Err(MigrationError::UnsupportedLegacyVersion(store.version));
    }

    let mut ids: BTreeMap<&str, ()> = BTreeMap::new();

    for acc in &store.accounts {
        if acc.id.is_empty() || acc.id.trim() != acc.id {
            return Err(MigrationError::InvalidLegacyAccountId);
        }
        if ids.insert(acc.id.as_str(), ()).is_some() {
            return Err(MigrationError::DuplicateLegacyAccountId);
        }

        if acc.name.is_empty() || acc.name.trim().is_empty() {
            return Err(MigrationError::InvalidLegacyMetadata);
        }

        if let Some(ref email) = acc.email {
            if email.is_empty() || email.trim() != email {
                return Err(MigrationError::InvalidLegacyMetadata);
            }
        }

        if let Some(ref plan) = acc.plan_type {
            if plan.is_empty() || plan.trim() != plan {
                return Err(MigrationError::InvalidLegacyMetadata);
            }
        }

        match (&acc.auth_mode, &acc.auth_data) {
            (LegacyAuthMode::ApiKey, LegacyAuthData::ApiKey { key }) => {
                if key.is_empty() {
                    return Err(MigrationError::InvalidLegacySecret);
                }
            }
            (
                LegacyAuthMode::ChatGpt,
                LegacyAuthData::ChatGpt {
                    id_token,
                    access_token,
                    refresh_token,
                    account_id,
                },
            ) => {
                if id_token.is_empty() || access_token.is_empty() || refresh_token.is_empty() {
                    return Err(MigrationError::InvalidLegacySecret);
                }
                if let Some(ref act_id) = account_id {
                    if act_id.is_empty() || act_id.trim() != act_id {
                        return Err(MigrationError::InvalidLegacySecret);
                    }
                }
            }
            _ => return Err(MigrationError::LegacyAuthModeMismatch),
        }
    }

    if let Some(ref active) = store.active_account_id {
        if active.is_empty() || active.trim() != active {
            return Err(MigrationError::InvalidLegacyActiveAccount);
        }
        if !store.accounts.iter().any(|a| a.id == *active) {
            return Err(MigrationError::InvalidLegacyActiveAccount);
        }
    }

    Ok(())
}

fn convert_legacy_store(
    legacy: LegacyAccountsStore,
) -> Result<(VaultPayloadV1, MetadataStoreV2), MigrationError> {
    let mut vault = VaultPayloadV1::new_empty();
    let mut metadata = MetadataStoreV2::new_empty();

    metadata.set_masked_account_ids(legacy.masked_account_ids);

    for acc in legacy.accounts {
        let auth_kind = match &acc.auth_data {
            LegacyAuthData::ApiKey { .. } => MetadataAuthKind::ApiKey,
            LegacyAuthData::ChatGpt { .. } => MetadataAuthKind::ChatGpt,
        };
        let secret_record = acc.auth_data.into_secret_record();

        vault
            .insert(&acc.id, secret_record)
            .map_err(|_| MigrationError::InvalidLegacySecret)?;

        let meta_acc = AccountMetadataV2 {
            id: acc.id.clone(),
            display_name: acc.name,
            email: acc.email,
            plan_type: acc.plan_type,
            subscription_expires_at: acc.subscription_expires_at,
            created_at: acc.created_at,
            last_used_at: acc.last_used_at,
            auth_kind,
            vault_ref: acc.id,
        };

        metadata
            .insert(meta_acc)
            .map_err(|_| MigrationError::InvalidLegacyMetadata)?;
    }

    if let Some(active) = legacy.active_account_id {
        metadata
            .set_active(Some(&active))
            .map_err(|_| MigrationError::InvalidLegacyActiveAccount)?;
    }

    Ok((vault, metadata))
}

fn extract_legacy_secrets(store: &LegacyAccountsStore) -> Vec<Zeroizing<Vec<u8>>> {
    let mut secrets = Vec::new();
    for acc in &store.accounts {
        match &acc.auth_data {
            LegacyAuthData::ApiKey { key } => {
                secrets.push(Zeroizing::new(key.as_bytes().to_vec()));
            }
            LegacyAuthData::ChatGpt {
                id_token,
                access_token,
                refresh_token,
                account_id,
            } => {
                secrets.push(Zeroizing::new(id_token.as_bytes().to_vec()));
                secrets.push(Zeroizing::new(access_token.as_bytes().to_vec()));
                secrets.push(Zeroizing::new(refresh_token.as_bytes().to_vec()));
                if let Some(act_id) = account_id {
                    secrets.push(Zeroizing::new(act_id.as_bytes().to_vec()));
                }
            }
        }
    }
    secrets
}

fn verify_secure_state_match(
    metadata: &MetadataStoreV2,
    vault: &VaultPayloadV1,
) -> Result<(), MigrationError> {
    if metadata.len() != vault.len() {
        return Err(MigrationError::SecureStateInconsistent);
    }
    for acc in &metadata.accounts {
        let secret = vault
            .get(&acc.vault_ref)
            .ok_or(MigrationError::SecureStateInconsistent)?;
        match (&acc.auth_kind, secret) {
            (MetadataAuthKind::ChatGpt, SecretRecord::ChatGpt { .. }) => {}
            (MetadataAuthKind::ApiKey, SecretRecord::ApiKey { .. }) => {}
            _ => return Err(MigrationError::SecureStateInconsistent),
        }
    }
    Ok(())
}

// ----- Tests --------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::secure_commit::SecureCommitTestOptions;
    use crate::types::{AccountsStore, StoredAccount};
    use std::path::PathBuf;

    const ID_TOKEN_A: &str = "synthetic-id-token-A";
    const ACCESS_TOKEN_A: &str = "synthetic-access-token-A";
    const REFRESH_TOKEN_A: &str = "synthetic-refresh-token-A";
    const API_KEY_A: &str = "synthetic-api-key-A";
    const CHATGPT_ACCOUNT_A: &str = "synthetic-chatgpt-account-A";
    const OLD_VAULT_API_KEY: &str = "synthetic-unrelated-old-vault-key-A";

    fn test_paths(tag: &str) -> (PathBuf, AppPaths) {
        let d =
            std::env::temp_dir().join(format!("codex_migr_test_{}_{}", tag, rand::random::<u32>()));
        std::fs::create_dir_all(&d).unwrap();
        let paths = AppPaths::for_test(&d);
        std::fs::create_dir_all(&paths.switcher_dir).unwrap();
        (d, paths)
    }

    fn sample_legacy_json() -> String {
        serde_json::json!({
            "version": 1,
            "active_account_id": "acc-1",
            "masked_account_ids": ["stale-id"],
            "accounts": [
                {
                    "id": "acc-1",
                    "name": "  Padded User  ",
                    "email": "user1@example.com",
                    "plan_type": "pro",
                    "subscription_expires_at": null,
                    "auth_mode": "chat_g_p_t",
                    "auth_data": {
                        "type": "chat_g_p_t",
                        "id_token": ID_TOKEN_A,
                        "access_token": ACCESS_TOKEN_A,
                        "refresh_token": REFRESH_TOKEN_A,
                        "account_id": CHATGPT_ACCOUNT_A
                    },
                    "created_at": "2026-08-01T00:00:00Z",
                    "last_used_at": null
                },
                {
                    "id": "acc-2",
                    "name": "API Key User",
                    "email": null,
                    "plan_type": null,
                    "subscription_expires_at": null,
                    "auth_mode": "api_key",
                    "auth_data": {
                        "type": "api_key",
                        "key": API_KEY_A
                    },
                    "created_at": "2026-08-01T00:00:00Z",
                    "last_used_at": null
                }
            ]
        })
        .to_string()
    }

    #[tokio::test]
    async fn test_migration_no_accounts_json_returns_no_store() {
        let (d, paths) = test_paths("no_store");
        let mig = LegacyMigration::for_test(paths);
        let res = mig.migrate_if_needed().await.unwrap();
        assert_eq!(res, MigrationOutcome::NoStore);
        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_migration_valid_empty_legacy_store_migrates() {
        let (d, paths) = test_paths("empty_legacy");
        let empty_legacy = serde_json::json!({
            "version": 1,
            "active_account_id": null,
            "masked_account_ids": [],
            "accounts": []
        })
        .to_string();

        std::fs::write(&paths.metadata_file, empty_legacy.as_bytes()).unwrap();

        let mig = LegacyMigration::for_test(paths);
        let res = mig.migrate_if_needed().await.unwrap();
        assert_eq!(res, MigrationOutcome::Migrated { account_count: 0 });

        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_migration_one_chatgpt_account_migrates() {
        let (d, paths) = test_paths("one_chatgpt");
        let legacy_json = serde_json::json!({
            "version": 1,
            "active_account_id": "acc-1",
            "accounts": [{
                "id": "acc-1",
                "name": "ChatGPT User",
                "email": "chatgpt@example.com",
                "plan_type": "pro",
                "auth_mode": "chat_g_p_t",
                "auth_data": {
                    "type": "chat_g_p_t",
                    "id_token": ID_TOKEN_A,
                    "access_token": ACCESS_TOKEN_A,
                    "refresh_token": REFRESH_TOKEN_A,
                    "account_id": CHATGPT_ACCOUNT_A
                },
                "created_at": "2026-08-01T00:00:00Z"
            }]
        })
        .to_string();

        std::fs::write(&paths.metadata_file, legacy_json.as_bytes()).unwrap();

        let mig = LegacyMigration::for_test(paths);
        let res = mig.migrate_if_needed().await.unwrap();
        assert_eq!(res, MigrationOutcome::Migrated { account_count: 1 });

        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_migration_accepts_production_chatgpt_wire_format() {
        let (d, paths) = test_paths("production_chatgpt_wire_format");
        let stored_account = StoredAccount::new_chatgpt(
            "Production ChatGPT".to_string(),
            Some("production@example.com".to_string()),
            Some("pro".to_string()),
            None,
            ID_TOKEN_A.to_string(),
            ACCESS_TOKEN_A.to_string(),
            REFRESH_TOKEN_A.to_string(),
            Some(CHATGPT_ACCOUNT_A.to_string()),
        );
        let account_id = stored_account.id.clone();
        let production_store = AccountsStore {
            version: 1,
            accounts: vec![stored_account],
            active_account_id: Some(account_id.clone()),
            masked_account_ids: Vec::new(),
        };
        let serialized = serde_json::to_vec(&production_store).unwrap();
        let serialized_value: serde_json::Value = serde_json::from_slice(&serialized).unwrap();
        assert_eq!(serialized_value["accounts"][0]["auth_mode"], "chat_g_p_t");
        assert_eq!(
            serialized_value["accounts"][0]["auth_data"]["type"],
            "chat_g_p_t"
        );
        std::fs::write(&paths.metadata_file, &serialized).unwrap();

        let migration = LegacyMigration::for_test(paths.clone());
        let result = migration.migrate_if_needed().await.unwrap();
        assert_eq!(result, MigrationOutcome::Migrated { account_count: 1 });

        let metadata = MetadataFileStore::from_paths(&paths).load().unwrap();
        let metadata_account = metadata.get(&account_id).unwrap();
        assert_eq!(metadata_account.display_name, "Production ChatGPT");
        assert_eq!(
            metadata_account.email.as_deref(),
            Some("production@example.com")
        );
        assert_eq!(metadata_account.plan_type.as_deref(), Some("pro"));
        assert_eq!(metadata_account.auth_kind, MetadataAuthKind::ChatGpt);
        assert_eq!(
            metadata.active_account_id.as_deref(),
            Some(account_id.as_str())
        );

        let vault = VaultStore::from_paths(&paths).load().unwrap();
        match vault.get(&account_id).unwrap() {
            SecretRecord::ChatGpt {
                id_token,
                access_token,
                refresh_token,
                account_id: vault_account_id,
            } => {
                assert_eq!(id_token, ID_TOKEN_A);
                assert_eq!(access_token, ACCESS_TOKEN_A);
                assert_eq!(refresh_token, REFRESH_TOKEN_A);
                assert_eq!(vault_account_id.as_deref(), Some(CHATGPT_ACCOUNT_A));
            }
            SecretRecord::ApiKey { .. } => panic!("Expected ChatGPT secret record"),
        }
        verify_secure_state_match(&metadata, &vault).unwrap();

        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_migration_accepts_chatgpt_compatibility_alias() {
        let (d, paths) = test_paths("chatgpt_alias");
        let alias_json = serde_json::json!({
            "version": 1,
            "active_account_id": "alias-account",
            "accounts": [{
                "id": "alias-account",
                "name": "Alias User",
                "email": "alias@example.com",
                "plan_type": "pro",
                "auth_mode": "chat_gpt",
                "auth_data": {
                    "type": "chat_gpt",
                    "id_token": ID_TOKEN_A,
                    "access_token": ACCESS_TOKEN_A,
                    "refresh_token": REFRESH_TOKEN_A,
                    "account_id": CHATGPT_ACCOUNT_A
                },
                "created_at": "2026-08-01T00:00:00Z"
            }]
        })
        .to_string();
        std::fs::write(&paths.metadata_file, alias_json.as_bytes()).unwrap();

        let migration = LegacyMigration::for_test(paths.clone());
        let result = migration.migrate_if_needed().await.unwrap();
        assert_eq!(result, MigrationOutcome::Migrated { account_count: 1 });

        let metadata = MetadataFileStore::from_paths(&paths).load().unwrap();
        assert_eq!(
            metadata.get("alias-account").unwrap().auth_kind,
            MetadataAuthKind::ChatGpt
        );
        let vault = VaultStore::from_paths(&paths).load().unwrap();
        assert!(matches!(
            vault.get("alias-account"),
            Some(SecretRecord::ChatGpt { .. })
        ));
        verify_secure_state_match(&metadata, &vault).unwrap();

        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn test_legacy_unrelated_chatgpt_spelling_is_rejected() {
        assert!(serde_json::from_str::<LegacyAuthMode>(r#""chatgpt""#).is_err());
        assert!(serde_json::from_value::<LegacyAuthData>(serde_json::json!({
            "type": "chatgpt",
            "id_token": ID_TOKEN_A,
            "access_token": ACCESS_TOKEN_A,
            "refresh_token": REFRESH_TOKEN_A,
            "account_id": CHATGPT_ACCOUNT_A
        }))
        .is_err());
    }

    #[tokio::test]
    async fn test_migration_chatgpt_account_id_none_is_preserved() {
        let (d, paths) = test_paths("chatgpt_none_id");
        let legacy_json = serde_json::json!({
            "version": 1,
            "active_account_id": "acc-1",
            "accounts": [{
                "id": "acc-1",
                "name": "User",
                "auth_mode": "chat_g_p_t",
                "auth_data": {
                    "type": "chat_g_p_t",
                    "id_token": ID_TOKEN_A,
                    "access_token": ACCESS_TOKEN_A,
                    "refresh_token": REFRESH_TOKEN_A,
                    "account_id": null
                },
                "created_at": "2026-08-01T00:00:00Z"
            }]
        })
        .to_string();

        std::fs::write(&paths.metadata_file, legacy_json.as_bytes()).unwrap();

        let mig = LegacyMigration::for_test(paths.clone());
        let res = mig.migrate_if_needed().await.unwrap();
        assert_eq!(res, MigrationOutcome::Migrated { account_count: 1 });

        let vault_store = VaultStore::from_paths(&paths);
        let vault = vault_store.load().unwrap();
        let rec = vault.get("acc-1").unwrap();
        if let SecretRecord::ChatGpt { account_id, .. } = rec {
            assert_eq!(account_id, &None);
        } else {
            panic!("Expected ChatGpt variant");
        }

        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_migration_one_api_key_account_migrates() {
        let (d, paths) = test_paths("one_api_key");
        let legacy_json = serde_json::json!({
            "version": 1,
            "active_account_id": "acc-1",
            "accounts": [{
                "id": "acc-1",
                "name": "API Key User",
                "auth_mode": "api_key",
                "auth_data": {
                    "type": "api_key",
                    "key": API_KEY_A
                },
                "created_at": "2026-08-01T00:00:00Z"
            }]
        })
        .to_string();

        std::fs::write(&paths.metadata_file, legacy_json.as_bytes()).unwrap();

        let mig = LegacyMigration::for_test(paths);
        let res = mig.migrate_if_needed().await.unwrap();
        assert_eq!(res, MigrationOutcome::Migrated { account_count: 1 });

        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_migration_mixed_account_types_migrate() {
        let (d, paths) = test_paths("mixed_types");
        std::fs::write(&paths.metadata_file, sample_legacy_json().as_bytes()).unwrap();

        let mig = LegacyMigration::for_test(paths);
        let res = mig.migrate_if_needed().await.unwrap();
        assert_eq!(res, MigrationOutcome::Migrated { account_count: 2 });

        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_migration_active_account_id_preserved() {
        let (d, paths) = test_paths("active_id_preserved");
        std::fs::write(&paths.metadata_file, sample_legacy_json().as_bytes()).unwrap();

        let mig = LegacyMigration::for_test(paths.clone());
        mig.migrate_if_needed().await.unwrap();

        let meta_store = MetadataFileStore::from_paths(&paths);
        let meta = meta_store.load().unwrap();
        assert_eq!(meta.active_account_id.as_deref(), Some("acc-1"));

        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_migration_masked_account_ids_preserved_exactly() {
        let (d, paths) = test_paths("masked_ids_preserved");
        std::fs::write(&paths.metadata_file, sample_legacy_json().as_bytes()).unwrap();

        let mig = LegacyMigration::for_test(paths.clone());
        mig.migrate_if_needed().await.unwrap();

        let meta_store = MetadataFileStore::from_paths(&paths);
        let meta = meta_store.load().unwrap();
        assert_eq!(meta.masked_account_ids(), &["stale-id"]);

        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_migration_stale_masked_id_preserved() {
        let (d, paths) = test_paths("stale_masked");
        let legacy_json = serde_json::json!({
            "version": 1,
            "masked_account_ids": ["stale-ghost-account"],
            "accounts": []
        })
        .to_string();
        std::fs::write(&paths.metadata_file, legacy_json.as_bytes()).unwrap();

        let mig = LegacyMigration::for_test(paths.clone());
        mig.migrate_if_needed().await.unwrap();

        let meta_store = MetadataFileStore::from_paths(&paths);
        let meta = meta_store.load().unwrap();
        assert_eq!(meta.masked_account_ids(), &["stale-ghost-account"]);

        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_migration_duplicate_masked_ids_preserved() {
        let (d, paths) = test_paths("dup_masked");
        let legacy_json = serde_json::json!({
            "version": 1,
            "masked_account_ids": ["stale-id", "stale-id"],
            "accounts": []
        })
        .to_string();
        std::fs::write(&paths.metadata_file, legacy_json.as_bytes()).unwrap();

        let mig = LegacyMigration::for_test(paths.clone());
        mig.migrate_if_needed().await.unwrap();

        let meta_store = MetadataFileStore::from_paths(&paths);
        let meta = meta_store.load().unwrap();
        assert_eq!(meta.masked_account_ids(), &["stale-id", "stale-id"]);

        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_migration_padded_valid_display_name_preserved_exactly() {
        let (d, paths) = test_paths("padded_display_name");
        std::fs::write(&paths.metadata_file, sample_legacy_json().as_bytes()).unwrap();

        let mig = LegacyMigration::for_test(paths.clone());
        mig.migrate_if_needed().await.unwrap();

        let meta_store = MetadataFileStore::from_paths(&paths);
        let meta = meta_store.load().unwrap();
        let acc1 = meta.get("acc-1").unwrap();
        assert_eq!(acc1.display_name, "  Padded User  ");

        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_migration_legacy_version_other_than_1_rejected() {
        let (d, paths) = test_paths("bad_version");
        let legacy_json = serde_json::json!({
            "version": 99,
            "accounts": []
        })
        .to_string();
        std::fs::write(&paths.metadata_file, legacy_json.as_bytes()).unwrap();

        let mig = LegacyMigration::for_test(paths);
        let res = mig.migrate_if_needed().await;
        assert!(matches!(
            res,
            Err(MigrationError::UnsupportedLegacyVersion(99))
        ));

        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_migration_duplicate_account_ids_rejected() {
        let (d, paths) = test_paths("dup_acc_ids");
        let legacy_json = serde_json::json!({
            "version": 1,
            "accounts": [
                {
                    "id": "acc-1",
                    "name": "User 1",
                    "auth_mode": "api_key",
                    "auth_data": { "type": "api_key", "key": API_KEY_A },
                    "created_at": "2026-08-01T00:00:00Z"
                },
                {
                    "id": "acc-1",
                    "name": "User 2",
                    "auth_mode": "api_key",
                    "auth_data": { "type": "api_key", "key": API_KEY_A },
                    "created_at": "2026-08-01T00:00:00Z"
                }
            ]
        })
        .to_string();
        std::fs::write(&paths.metadata_file, legacy_json.as_bytes()).unwrap();

        let mig = LegacyMigration::for_test(paths);
        let res = mig.migrate_if_needed().await;
        assert!(matches!(res, Err(MigrationError::DuplicateLegacyAccountId)));

        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_migration_missing_active_account_rejected() {
        let (d, paths) = test_paths("missing_active");
        let legacy_json = serde_json::json!({
            "version": 1,
            "active_account_id": "ghost-acc",
            "accounts": []
        })
        .to_string();
        std::fs::write(&paths.metadata_file, legacy_json.as_bytes()).unwrap();

        let mig = LegacyMigration::for_test(paths);
        let res = mig.migrate_if_needed().await;
        assert!(matches!(
            res,
            Err(MigrationError::InvalidLegacyActiveAccount)
        ));

        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_migration_auth_mode_data_mismatch_rejected() {
        let (d, paths) = test_paths("mode_mismatch");
        let legacy_json = serde_json::json!({
            "version": 1,
            "accounts": [{
                "id": "acc-1",
                "name": "User 1",
                "auth_mode": "api_key",
                "auth_data": {
                    "type": "chat_g_p_t",
                    "id_token": ID_TOKEN_A,
                    "access_token": ACCESS_TOKEN_A,
                    "refresh_token": REFRESH_TOKEN_A
                },
                "created_at": "2026-08-01T00:00:00Z"
            }]
        })
        .to_string();
        std::fs::write(&paths.metadata_file, legacy_json.as_bytes()).unwrap();

        let mig = LegacyMigration::for_test(paths);
        let res = mig.migrate_if_needed().await;
        assert!(matches!(res, Err(MigrationError::LegacyAuthModeMismatch)));

        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_migration_empty_chatgpt_token_rejected() {
        let (d, paths) = test_paths("empty_token");
        let legacy_json = serde_json::json!({
            "version": 1,
            "accounts": [{
                "id": "acc-1",
                "name": "User 1",
                "auth_mode": "chat_g_p_t",
                "auth_data": {
                    "type": "chat_g_p_t",
                    "id_token": "",
                    "access_token": ACCESS_TOKEN_A,
                    "refresh_token": REFRESH_TOKEN_A
                },
                "created_at": "2026-08-01T00:00:00Z"
            }]
        })
        .to_string();
        std::fs::write(&paths.metadata_file, legacy_json.as_bytes()).unwrap();

        let mig = LegacyMigration::for_test(paths);
        let res = mig.migrate_if_needed().await;
        assert!(matches!(res, Err(MigrationError::InvalidLegacySecret)));

        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_migration_empty_api_key_rejected() {
        let (d, paths) = test_paths("empty_key");
        let legacy_json = serde_json::json!({
            "version": 1,
            "accounts": [{
                "id": "acc-1",
                "name": "User 1",
                "auth_mode": "api_key",
                "auth_data": {
                    "type": "api_key",
                    "key": ""
                },
                "created_at": "2026-08-01T00:00:00Z"
            }]
        })
        .to_string();
        std::fs::write(&paths.metadata_file, legacy_json.as_bytes()).unwrap();

        let mig = LegacyMigration::for_test(paths);
        let res = mig.migrate_if_needed().await;
        assert!(matches!(res, Err(MigrationError::InvalidLegacySecret)));

        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_migration_malformed_json_leaves_original_bytes_unchanged() {
        let (d, paths) = test_paths("malformed_json");
        let bad_json = b"{ not valid json }";
        std::fs::write(&paths.metadata_file, bad_json).unwrap();

        let mig = LegacyMigration::for_test(paths.clone());
        let res = mig.migrate_if_needed().await;
        assert!(matches!(res, Err(MigrationError::InvalidStoreFormat)));

        assert_eq!(
            std::fs::read(&paths.metadata_file).unwrap(),
            bad_json.as_slice()
        );

        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_migration_unknown_legacy_field_rejected() {
        let (d, paths) = test_paths("unknown_field");
        let legacy_json = serde_json::json!({
            "version": 1,
            "accounts": [],
            "unknown_extra_field": true
        })
        .to_string();
        std::fs::write(&paths.metadata_file, legacy_json.as_bytes()).unwrap();

        let mig = LegacyMigration::for_test(paths);
        let res = mig.migrate_if_needed().await;
        assert!(matches!(res, Err(MigrationError::InvalidStoreFormat)));

        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_migration_produces_metadata_schema_2() {
        let (d, paths) = test_paths("schema_2_check");
        std::fs::write(&paths.metadata_file, sample_legacy_json().as_bytes()).unwrap();

        let mig = LegacyMigration::for_test(paths.clone());
        mig.migrate_if_needed().await.unwrap();

        let meta_store = MetadataFileStore::from_paths(&paths);
        let meta = meta_store.load().unwrap();
        assert_eq!(meta.schema_version, 2);

        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_migration_final_metadata_contains_no_prohibited_secret_keys() {
        let (d, paths) = test_paths("prohibited_keys_check");
        std::fs::write(&paths.metadata_file, sample_legacy_json().as_bytes()).unwrap();

        let mig = LegacyMigration::for_test(paths.clone());
        mig.migrate_if_needed().await.unwrap();

        let raw_meta = std::fs::read(&paths.metadata_file).unwrap();
        let val: serde_json::Value = serde_json::from_slice(&raw_meta).unwrap();
        assert!(crate::auth::metadata_store::check_no_prohibited_keys(&val).is_ok());

        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_migration_final_metadata_contains_no_synthetic_secret_values() {
        let (d, paths) = test_paths("secrecy_check");
        std::fs::write(&paths.metadata_file, sample_legacy_json().as_bytes()).unwrap();

        let mig = LegacyMigration::for_test(paths.clone());
        mig.migrate_if_needed().await.unwrap();

        let raw_meta = std::fs::read_to_string(&paths.metadata_file).unwrap();
        for secret in &[
            ID_TOKEN_A,
            ACCESS_TOKEN_A,
            REFRESH_TOKEN_A,
            API_KEY_A,
            CHATGPT_ACCOUNT_A,
        ] {
            assert!(
                !raw_meta.contains(secret),
                "Metadata file contains secret '{secret}'"
            );
        }

        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_migration_vault_file_is_encrypted_non_json() {
        let (d, paths) = test_paths("vault_non_json");
        std::fs::write(&paths.metadata_file, sample_legacy_json().as_bytes()).unwrap();

        let mig = LegacyMigration::for_test(paths.clone());
        mig.migrate_if_needed().await.unwrap();

        let raw_vault = std::fs::read(&paths.vault_file).unwrap();
        assert!(serde_json::from_slice::<serde_json::Value>(&raw_vault).is_err());

        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_migration_running_twice_returns_already_secure() {
        let (d, paths) = test_paths("twice_run");
        std::fs::write(&paths.metadata_file, sample_legacy_json().as_bytes()).unwrap();

        let mig = LegacyMigration::for_test(paths.clone());
        let res1 = mig.migrate_if_needed().await.unwrap();
        assert!(matches!(res1, MigrationOutcome::Migrated { .. }));

        let res2 = mig.migrate_if_needed().await.unwrap();
        assert_eq!(res2, MigrationOutcome::AlreadySecure);

        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_migration_failure_before_vault_install_leaves_legacy_unchanged() {
        let (d, paths) = test_paths("fail_before_vault");
        let legacy_bytes = sample_legacy_json().into_bytes();
        std::fs::write(&paths.metadata_file, &legacy_bytes).unwrap();

        let committer =
            SecurePairCommitter::for_paths(paths.vault_file.clone(), paths.metadata_file.clone())
                .with_test_options(SecureCommitTestOptions {
                    fail_before_vault_install: true,
                    ..Default::default()
                });

        let mig = LegacyMigration {
            paths: paths.clone(),
            mutation_lock: MutationLock::from_paths(&paths),
            committer,
        };

        let res = mig.migrate_if_needed().await;
        assert!(matches!(
            res,
            Err(MigrationError::SecureCommitFailed(
                SecureCommitError::VaultInstallFailed
            ))
        ));

        let current_meta = std::fs::read(&paths.metadata_file).unwrap();
        assert_eq!(current_meta, legacy_bytes);
        assert!(!paths.vault_file.exists());

        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_migration_simulated_crash_after_vault_install_leaves_orphan_for_retry() {
        let (d, paths) = test_paths("simulated_crash_after_vault");
        let legacy_bytes = sample_legacy_json().into_bytes();
        std::fs::write(&paths.metadata_file, &legacy_bytes).unwrap();

        let committer =
            SecurePairCommitter::for_paths(paths.vault_file.clone(), paths.metadata_file.clone())
                .with_test_options(SecureCommitTestOptions {
                    simulate_crash_after_vault_install: true,
                    ..Default::default()
                });

        let mig = LegacyMigration {
            paths: paths.clone(),
            mutation_lock: MutationLock::from_paths(&paths),
            committer,
        };

        let res = mig.migrate_if_needed().await;
        assert!(matches!(
            res,
            Err(MigrationError::SecureCommitFailed(
                SecureCommitError::SimulatedCrashAfterVaultInstall
            ))
        ));

        let current_meta = std::fs::read(&paths.metadata_file).unwrap();
        assert_eq!(current_meta, legacy_bytes);
        let orphan_vault_bytes = std::fs::read(&paths.vault_file).unwrap();
        assert!(!orphan_vault_bytes.is_empty());
        assert!(VaultStore::from_paths(&paths).load().is_ok());

        let retry = LegacyMigration::from_paths(paths.clone());
        let retry_result = retry.migrate_if_needed().await.unwrap();
        assert_eq!(
            retry_result,
            MigrationOutcome::Migrated { account_count: 2 }
        );

        let final_metadata_bytes = std::fs::read(&paths.metadata_file).unwrap();
        assert_ne!(final_metadata_bytes, legacy_bytes);
        let final_vault_bytes = std::fs::read(&paths.vault_file).unwrap();
        assert_ne!(final_vault_bytes, orphan_vault_bytes);

        let final_metadata = MetadataFileStore::from_paths(&paths).load().unwrap();
        let final_vault = VaultStore::from_paths(&paths).load().unwrap();
        verify_secure_state_match(&final_metadata, &final_vault).unwrap();

        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_migration_metadata_failure_restores_prior_vault_exactly() {
        let (d, paths) = test_paths("fail_rollback_prior_vault");
        let legacy_bytes = sample_legacy_json().into_bytes();
        std::fs::write(&paths.metadata_file, &legacy_bytes).unwrap();

        let mut old_vault = VaultPayloadV1::new_empty();
        old_vault
            .insert(
                "unrelated-old-account",
                SecretRecord::ApiKey {
                    key: OLD_VAULT_API_KEY.to_string(),
                },
            )
            .unwrap();
        VaultStore::from_paths(&paths).save(&old_vault).unwrap();
        let old_vault_bytes = std::fs::read(&paths.vault_file).unwrap();

        let committer =
            SecurePairCommitter::for_paths(paths.vault_file.clone(), paths.metadata_file.clone())
                .with_test_options(SecureCommitTestOptions {
                    fail_metadata_install: true,
                    ..Default::default()
                });

        let mig = LegacyMigration {
            paths: paths.clone(),
            mutation_lock: MutationLock::from_paths(&paths),
            committer,
        };

        let res = mig.migrate_if_needed().await;
        assert!(matches!(
            res,
            Err(MigrationError::SecureCommitFailed(
                SecureCommitError::MetadataInstallFailed
            ))
        ));

        assert_eq!(std::fs::read(&paths.metadata_file).unwrap(), legacy_bytes);
        assert_eq!(std::fs::read(&paths.vault_file).unwrap(), old_vault_bytes);

        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_migration_metadata_failure_without_prior_vault_removes_new_vault() {
        let (d, paths) = test_paths("fail_rollback_without_prior_vault");
        let legacy_bytes = sample_legacy_json().into_bytes();
        std::fs::write(&paths.metadata_file, &legacy_bytes).unwrap();

        let committer =
            SecurePairCommitter::for_paths(paths.vault_file.clone(), paths.metadata_file.clone())
                .with_test_options(SecureCommitTestOptions {
                    fail_metadata_install: true,
                    ..Default::default()
                });

        let mig = LegacyMigration {
            paths: paths.clone(),
            mutation_lock: MutationLock::from_paths(&paths),
            committer,
        };

        let res = mig.migrate_if_needed().await;
        assert!(matches!(
            res,
            Err(MigrationError::SecureCommitFailed(
                SecureCommitError::MetadataInstallFailed
            ))
        ));
        assert_eq!(std::fs::read(&paths.metadata_file).unwrap(), legacy_bytes);
        assert!(!paths.vault_file.exists());

        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_migration_failed_migration_can_retry_successfully() {
        let (d, paths) = test_paths("retry_success");
        let legacy_bytes = sample_legacy_json().into_bytes();
        std::fs::write(&paths.metadata_file, &legacy_bytes).unwrap();

        // 1. First run fails at metadata install step
        let committer_fail =
            SecurePairCommitter::for_paths(paths.vault_file.clone(), paths.metadata_file.clone())
                .with_test_options(SecureCommitTestOptions {
                    fail_metadata_install: true,
                    ..Default::default()
                });

        let mig_fail = LegacyMigration {
            paths: paths.clone(),
            mutation_lock: MutationLock::from_paths(&paths),
            committer: committer_fail,
        };

        let res_fail = mig_fail.migrate_if_needed().await;
        assert!(matches!(
            res_fail,
            Err(MigrationError::SecureCommitFailed(
                SecureCommitError::MetadataInstallFailed
            ))
        ));
        assert_eq!(std::fs::read(&paths.metadata_file).unwrap(), legacy_bytes);
        assert!(!paths.vault_file.exists());

        // 2. Second run with normal committer retries and succeeds
        let mig_good = LegacyMigration::from_paths(paths.clone());
        let res_good = mig_good.migrate_if_needed().await.unwrap();
        assert_eq!(res_good, MigrationOutcome::Migrated { account_count: 2 });
        assert!(paths.vault_file.exists());
        let final_metadata = MetadataFileStore::from_paths(&paths).load().unwrap();
        let final_vault = VaultStore::from_paths(&paths).load().unwrap();
        verify_secure_state_match(&final_metadata, &final_vault).unwrap();

        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_migration_secure_metadata_with_missing_non_empty_vault_fails_closed() {
        let (d, paths) = test_paths("missing_vault_fails_closed");
        // Write valid metadata v2 with 1 account
        let meta = serde_json::json!({
            "schema_version": 2,
            "active_account_id": "acc-1",
            "accounts": [{
                "id": "acc-1",
                "display_name": "User 1",
                "email": null,
                "plan_type": null,
                "subscription_expires_at": null,
                "created_at": "2026-08-01T00:00:00Z",
                "last_used_at": null,
                "auth_kind": "api_key",
                "vault_ref": "acc-1"
            }]
        })
        .to_string();
        std::fs::write(&paths.metadata_file, meta.as_bytes()).unwrap();

        let mig = LegacyMigration::for_test(paths);
        let res = mig.migrate_if_needed().await;
        assert!(matches!(res, Err(MigrationError::SecureStateInconsistent)));

        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_migration_secure_metadata_with_corrupt_vault_fails_closed() {
        let (d, paths) = test_paths("corrupt_vault_fails_closed");
        let meta = serde_json::json!({
            "schema_version": 2,
            "active_account_id": null,
            "accounts": [{
                "id": "acc-1",
                "display_name": "User 1",
                "email": null,
                "plan_type": null,
                "subscription_expires_at": null,
                "created_at": "2026-08-01T00:00:00Z",
                "last_used_at": null,
                "auth_kind": "api_key",
                "vault_ref": "acc-1"
            }]
        })
        .to_string();
        std::fs::write(&paths.metadata_file, meta.as_bytes()).unwrap();
        std::fs::write(&paths.vault_file, b"corrupt-vault-bytes").unwrap();

        let mig = LegacyMigration::for_test(paths);
        let res = mig.migrate_if_needed().await;
        assert!(matches!(res, Err(MigrationError::SecureStateInconsistent)));

        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_migration_secure_metadata_vault_account_mismatch_fails_closed() {
        let (d, paths) = test_paths("mismatch_acc_fails_closed");
        let meta = serde_json::json!({
            "schema_version": 2,
            "active_account_id": null,
            "accounts": [{
                "id": "acc-1",
                "display_name": "User 1",
                "email": null,
                "plan_type": null,
                "subscription_expires_at": null,
                "created_at": "2026-08-01T00:00:00Z",
                "last_used_at": null,
                "auth_kind": "api_key",
                "vault_ref": "acc-1"
            }]
        })
        .to_string();
        std::fs::write(&paths.metadata_file, meta.as_bytes()).unwrap();

        // Write vault payload with acc-2 instead of acc-1
        let mut vault = VaultPayloadV1::new_empty();
        vault
            .insert(
                "acc-2",
                SecretRecord::ApiKey {
                    key: API_KEY_A.to_string(),
                },
            )
            .unwrap();
        VaultStore::from_paths(&paths).save(&vault).unwrap();

        let mig = LegacyMigration::for_test(paths);
        let res = mig.migrate_if_needed().await;
        assert!(matches!(res, Err(MigrationError::SecureStateInconsistent)));

        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_migration_secure_metadata_vault_auth_kind_mismatch_fails_closed() {
        let (d, paths) = test_paths("mismatch_kind_fails_closed");
        let meta = serde_json::json!({
            "schema_version": 2,
            "active_account_id": null,
            "accounts": [{
                "id": "acc-1",
                "display_name": "User 1",
                "email": null,
                "plan_type": null,
                "subscription_expires_at": null,
                "created_at": "2026-08-01T00:00:00Z",
                "last_used_at": null,
                "auth_kind": "chatgpt",
                "vault_ref": "acc-1"
            }]
        })
        .to_string();
        std::fs::write(&paths.metadata_file, meta.as_bytes()).unwrap();

        // Write vault payload with ApiKey instead of ChatGpt
        let mut vault = VaultPayloadV1::new_empty();
        vault
            .insert(
                "acc-1",
                SecretRecord::ApiKey {
                    key: API_KEY_A.to_string(),
                },
            )
            .unwrap();
        VaultStore::from_paths(&paths).save(&vault).unwrap();

        let mig = LegacyMigration::for_test(paths);
        let res = mig.migrate_if_needed().await;
        assert!(matches!(res, Err(MigrationError::SecureStateInconsistent)));

        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_migration_empty_secure_metadata_with_no_vault_returns_already_secure() {
        let (d, paths) = test_paths("empty_secure_no_vault");
        let meta = serde_json::json!({
            "schema_version": 2,
            "active_account_id": null,
            "accounts": []
        })
        .to_string();
        std::fs::write(&paths.metadata_file, meta.as_bytes()).unwrap();

        let mig = LegacyMigration::for_test(paths);
        let res = mig.migrate_if_needed().await.unwrap();
        assert_eq!(res, MigrationOutcome::AlreadySecure);

        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_migration_empty_secure_metadata_with_empty_vault_returns_already_secure() {
        let (d, paths) = test_paths("empty_secure_empty_vault");
        let meta = serde_json::json!({
            "schema_version": 2,
            "active_account_id": null,
            "accounts": []
        })
        .to_string();
        std::fs::write(&paths.metadata_file, meta.as_bytes()).unwrap();

        let vault = VaultPayloadV1::new_empty();
        VaultStore::from_paths(&paths).save(&vault).unwrap();

        let mig = LegacyMigration::for_test(paths);
        let res = mig.migrate_if_needed().await.unwrap();
        assert_eq!(res, MigrationOutcome::AlreadySecure);

        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_migration_no_plaintext_backup_files_exist() {
        let (d, paths) = test_paths("no_backup_files");
        std::fs::write(&paths.metadata_file, sample_legacy_json().as_bytes()).unwrap();

        let mig = LegacyMigration::for_test(paths.clone());
        mig.migrate_if_needed().await.unwrap();

        let entries: Vec<_> = std::fs::read_dir(&d)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();

        for name in &entries {
            assert!(
                !name.ends_with(".bak"),
                "Plaintext backup file was created: {name}"
            );
            assert!(
                !name.starts_with(".tmp_"),
                "Temporary migration file left: {name}"
            );
        }

        let _ = std::fs::remove_dir_all(d);
    }

    #[tokio::test]
    async fn test_migration_errors_contain_none_of_the_synthetic_secrets() {
        let secrets = [
            ID_TOKEN_A,
            ACCESS_TOKEN_A,
            REFRESH_TOKEN_A,
            API_KEY_A,
            CHATGPT_ACCOUNT_A,
        ];
        let errors: &[&dyn std::fmt::Display] = &[
            &MigrationError::LockFailed,
            &MigrationError::AccountsReadFailed,
            &MigrationError::InvalidStoreFormat,
            &MigrationError::UnsupportedLegacyVersion(99),
            &MigrationError::InvalidLegacyAccountId,
            &MigrationError::DuplicateLegacyAccountId,
            &MigrationError::InvalidLegacyActiveAccount,
            &MigrationError::LegacyAuthModeMismatch,
            &MigrationError::InvalidLegacyMetadata,
            &MigrationError::InvalidLegacySecret,
            &MigrationError::MetadataContainsSecret,
            &MigrationError::SecureStateInconsistent,
            &MigrationError::FinalVerificationFailed,
        ];

        for err in errors {
            let msg = err.to_string();
            for secret in &secrets {
                assert!(!msg.contains(secret));
            }
        }
    }

    #[tokio::test]
    async fn test_migration_operation_lock_serializes_concurrent_migrations() {
        let (d, paths) = test_paths("concurrent_migrations");
        std::fs::write(&paths.metadata_file, sample_legacy_json().as_bytes()).unwrap();

        let mig1 = LegacyMigration::for_test(paths.clone());
        let mig2 = LegacyMigration::for_test(paths.clone());

        let handle1 = tokio::spawn(async move { mig1.migrate_if_needed().await });
        let handle2 = tokio::spawn(async move { mig2.migrate_if_needed().await });

        let res1 = handle1.await.unwrap().unwrap();
        let res2 = handle2.await.unwrap().unwrap();

        assert!(
            (res1 == MigrationOutcome::Migrated { account_count: 2 }
                && res2 == MigrationOutcome::AlreadySecure)
                || (res2 == MigrationOutcome::Migrated { account_count: 2 }
                    && res1 == MigrationOutcome::AlreadySecure)
        );

        let _ = std::fs::remove_dir_all(d);
    }
}
