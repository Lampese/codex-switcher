//! Metadata-only account store — Phase 1B-1.
//!
//! Stores display names, email, plan metadata, timestamps, and active-account
//! state. Must never contain any secret credentials.

use std::collections::BTreeMap;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::auth::atomic_file::{atomic_write, FileSensitivity};
use crate::auth::paths::AppPaths;

// ----- Constants ----------------------------------------------------------------

pub(crate) const METADATA_SCHEMA_VERSION: u32 = 2;

/// JSON object keys that must never appear in serialized metadata output.
/// Matched structurally (not by substring scan).
const PROHIBITED_SECRET_KEYS: &[&str] = &[
    "access_token",
    "refresh_token",
    "id_token",
    "api_key",
    "auth_json",
    "tokens",
];

// ----- Error model --------------------------------------------------------------

/// Typed, sanitized errors for metadata store operations.
/// Error messages must not include email addresses or complete JSON.
/// Accounts are identified only by their short internal ID when necessary.
#[derive(Debug, thiserror::Error)]
pub(crate) enum MetadataStoreError {
    #[error("Metadata file does not exist")]
    MissingMetadata,

    #[error("Failed to read metadata file: {0}")]
    ReadFailed(std::io::Error),

    #[error("Failed to deserialize metadata")]
    DeserializeFailed,

    #[error("Metadata schema version {0} is not supported (expected 2)")]
    UnsupportedSchema(u32),

    #[error("Account ID is invalid")]
    InvalidAccountId,

    #[error("Duplicate account ID detected")]
    DuplicateAccountId,

    #[error("vault_ref must equal account ID in schema v2")]
    InvalidVaultReference,

    #[error("Duplicate vault_ref detected")]
    DuplicateVaultReference,

    #[error("Display name is empty or has leading/trailing whitespace")]
    InvalidDisplayName,

    #[error("Email is present but empty or has leading/trailing whitespace")]
    InvalidEmail,

    #[error("Plan type is present but empty or has leading/trailing whitespace")]
    InvalidPlanType,

    #[error("active_account_id references an unknown or invalid account")]
    InvalidActiveAccount,

    #[error("Serialized metadata contains a prohibited secret field name")]
    ProhibitedSecretField,

    #[error("Failed to serialize metadata")]
    SerializeFailed,

    #[error("Failed to write metadata atomically: {0}")]
    AtomicWriteFailed(String),
}

// ----- Types --------------------------------------------------------------------

/// Auth-kind tag stored alongside account metadata (no credentials).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum MetadataAuthKind {
    #[serde(rename = "chatgpt")]
    ChatGpt,
    #[serde(rename = "api_key")]
    ApiKey,
}

/// Per-account metadata record. Contains no secrets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AccountMetadataV2 {
    pub(crate) id: String,
    pub(crate) display_name: String,
    pub(crate) email: Option<String>,
    pub(crate) plan_type: Option<String>,
    pub(crate) subscription_expires_at: Option<DateTime<Utc>>,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) last_used_at: Option<DateTime<Utc>>,
    pub(crate) auth_kind: MetadataAuthKind,
    pub(crate) vault_ref: String,
}

impl AccountMetadataV2 {
    /// Validate basic syntax of an individual account record.
    fn validate(&self) -> Result<(), MetadataStoreError> {
        validate_id(&self.id)?;
        validate_vault_ref_syntax(&self.vault_ref)?;
        validate_display_name(&self.display_name)?;
        if let Some(ref email) = self.email {
            validate_optional_str(email, MetadataStoreError::InvalidEmail)?;
        }
        if let Some(ref plan) = self.plan_type {
            validate_optional_str(plan, MetadataStoreError::InvalidPlanType)?;
        }
        Ok(())
    }
}

/// The full metadata store document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MetadataStoreV2 {
    pub(crate) schema_version: u32,
    pub(crate) active_account_id: Option<String>,
    pub(crate) accounts: Vec<AccountMetadataV2>,
    #[serde(default)]
    pub(crate) masked_account_ids: Vec<String>,
}

// ----- Field validators ---------------------------------------------------------

fn validate_id(id: &str) -> Result<(), MetadataStoreError> {
    if id.is_empty() || id.trim() != id {
        return Err(MetadataStoreError::InvalidAccountId);
    }
    Ok(())
}

fn validate_vault_ref_syntax(vault_ref: &str) -> Result<(), MetadataStoreError> {
    if vault_ref.is_empty() || vault_ref.trim() != vault_ref {
        return Err(MetadataStoreError::InvalidVaultReference);
    }
    Ok(())
}

fn validate_display_name(name: &str) -> Result<(), MetadataStoreError> {
    if name.is_empty() || name.trim().is_empty() {
        return Err(MetadataStoreError::InvalidDisplayName);
    }
    Ok(())
}

fn validate_optional_str(value: &str, err: MetadataStoreError) -> Result<(), MetadataStoreError> {
    if value.is_empty() || value.trim() != value {
        return Err(err);
    }
    Ok(())
}

// ----- Prohibited key check -----------------------------------------------------

/// Recursively visit every JSON object key in `value` and reject any that
/// are in `PROHIBITED_SECRET_KEYS`. Uses structural serde_json traversal —
/// not substring scanning.
pub(crate) fn check_no_prohibited_keys(
    value: &serde_json::Value,
) -> Result<(), MetadataStoreError> {
    match value {
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                if PROHIBITED_SECRET_KEYS.contains(&key.as_str()) {
                    return Err(MetadataStoreError::ProhibitedSecretField);
                }
                check_no_prohibited_keys(child)?;
            }
        }
        serde_json::Value::Array(arr) => {
            for child in arr {
                check_no_prohibited_keys(child)?;
            }
        }
        _ => {}
    }
    Ok(())
}

// ----- MetadataStoreV2 helpers --------------------------------------------------

impl MetadataStoreV2 {
    pub(crate) fn new_empty() -> Self {
        Self {
            schema_version: METADATA_SCHEMA_VERSION,
            active_account_id: None,
            accounts: Vec::new(),
            masked_account_ids: Vec::new(),
        }
    }

    pub(crate) fn masked_account_ids(&self) -> &[String] {
        &self.masked_account_ids
    }

    pub(crate) fn set_masked_account_ids(&mut self, masked_ids: Vec<String>) {
        self.masked_account_ids = masked_ids;
    }

    pub(crate) fn get(&self, id: &str) -> Option<&AccountMetadataV2> {
        self.accounts.iter().find(|a| a.id == id)
    }

    pub(crate) fn get_mut(&mut self, id: &str) -> Option<&mut AccountMetadataV2> {
        self.accounts.iter_mut().find(|a| a.id == id)
    }

    /// Insert an account. Rejects duplicates without mutating the store.
    pub(crate) fn insert(&mut self, account: AccountMetadataV2) -> Result<(), MetadataStoreError> {
        // Basic account field syntax checks first
        account.validate()?;

        // Check duplicate account ID
        if self.accounts.iter().any(|a| a.id == account.id) {
            return Err(MetadataStoreError::DuplicateAccountId);
        }
        // Check duplicate vault_ref
        if self
            .accounts
            .iter()
            .any(|a| a.vault_ref == account.vault_ref)
        {
            return Err(MetadataStoreError::DuplicateVaultReference);
        }
        // Enforce vault_ref == id
        if account.vault_ref != account.id {
            return Err(MetadataStoreError::InvalidVaultReference);
        }

        self.accounts.push(account);
        Ok(())
    }

    /// Remove an account by ID. Clears active_account_id if it matches.
    pub(crate) fn remove(&mut self, id: &str) -> Option<AccountMetadataV2> {
        if let Some(pos) = self.accounts.iter().position(|a| a.id == id) {
            let removed = self.accounts.remove(pos);
            if self.active_account_id.as_deref() == Some(id) {
                self.active_account_id = None;
            }
            Some(removed)
        } else {
            None
        }
    }

    /// Set the active account. Rejects unknown IDs without mutating the store.
    pub(crate) fn set_active(
        &mut self,
        id_or_none: Option<&str>,
    ) -> Result<(), MetadataStoreError> {
        match id_or_none {
            None => {
                self.active_account_id = None;
                Ok(())
            }
            Some(id) => {
                if !self.accounts.iter().any(|a| a.id == id) {
                    return Err(MetadataStoreError::InvalidActiveAccount);
                }
                self.active_account_id = Some(id.to_string());
                Ok(())
            }
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.accounts.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.accounts.is_empty()
    }

    /// Full structural validation in strict ordered passes.
    pub(crate) fn validate(&self) -> Result<(), MetadataStoreError> {
        if self.schema_version != METADATA_SCHEMA_VERSION {
            return Err(MetadataStoreError::UnsupportedSchema(self.schema_version));
        }

        // Pass 1:
        // - Basic syntax of ID and vault_ref
        // - Duplicate account IDs
        // - Duplicate vault_ref values
        let mut ids: BTreeMap<&str, ()> = BTreeMap::new();
        let mut vault_refs: BTreeMap<&str, ()> = BTreeMap::new();

        for account in &self.accounts {
            validate_id(&account.id)?;
            validate_vault_ref_syntax(&account.vault_ref)?;

            if ids.insert(account.id.as_str(), ()).is_some() {
                return Err(MetadataStoreError::DuplicateAccountId);
            }
            if vault_refs.insert(account.vault_ref.as_str(), ()).is_some() {
                return Err(MetadataStoreError::DuplicateVaultReference);
            }
        }

        // Pass 2:
        // - Display name, optional email, optional plan type
        // - Enforce vault_ref == id
        for account in &self.accounts {
            validate_display_name(&account.display_name)?;
            if let Some(ref email) = account.email {
                validate_optional_str(email, MetadataStoreError::InvalidEmail)?;
            }
            if let Some(ref plan) = account.plan_type {
                validate_optional_str(plan, MetadataStoreError::InvalidPlanType)?;
            }
            if account.vault_ref != account.id {
                return Err(MetadataStoreError::InvalidVaultReference);
            }
        }

        // Validate active_account_id.
        if let Some(ref active) = self.active_account_id {
            if active.trim() != active.as_str() || active.is_empty() {
                return Err(MetadataStoreError::InvalidActiveAccount);
            }
            if !self.accounts.iter().any(|a| a.id == *active) {
                return Err(MetadataStoreError::InvalidActiveAccount);
            }
        } else if self.accounts.is_empty() {
            // Empty store with no active account is valid.
        }

        Ok(())
    }
}

// ----- MetadataFileStore --------------------------------------------------------

pub(crate) struct MetadataFileStore {
    path: PathBuf,
}

impl MetadataFileStore {
    pub(crate) fn from_paths(paths: &AppPaths) -> Self {
        Self {
            path: paths.metadata_file.clone(),
        }
    }

    pub(crate) fn for_path(path: PathBuf) -> Self {
        Self { path }
    }

    pub(crate) fn exists(&self) -> bool {
        self.path.exists()
    }

    /// Load metadata from disk. Never repairs or rewrites automatically.
    pub(crate) fn load(&self) -> Result<MetadataStoreV2, MetadataStoreError> {
        if !self.path.exists() {
            return Err(MetadataStoreError::MissingMetadata);
        }
        let raw = std::fs::read(&self.path).map_err(MetadataStoreError::ReadFailed)?;
        let store: MetadataStoreV2 =
            serde_json::from_slice(&raw).map_err(|_| MetadataStoreError::DeserializeFailed)?;
        store.validate()?;
        Ok(store)
    }

    /// Like load() but returns Ok(None) for a missing file.
    /// Any other error — including corrupt JSON — is propagated as an error.
    pub(crate) fn load_optional(&self) -> Result<Option<MetadataStoreV2>, MetadataStoreError> {
        match self.load() {
            Ok(store) => Ok(Some(store)),
            Err(MetadataStoreError::MissingMetadata) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Encode store into in-memory JSON bytes after validation and secrecy verification.
    pub(crate) fn encode(&self, store: &MetadataStoreV2) -> Result<Vec<u8>, MetadataStoreError> {
        store.validate()?;
        let bytes =
            serde_json::to_vec_pretty(store).map_err(|_| MetadataStoreError::SerializeFailed)?;
        let json_value: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|_| MetadataStoreError::SerializeFailed)?;
        check_no_prohibited_keys(&json_value)?;
        Ok(bytes)
    }

    /// Install pre-encoded metadata JSON bytes into accounts.json after strict validation and secrecy check.
    pub(crate) fn install_encoded(&self, bytes: &[u8]) -> Result<(), MetadataStoreError> {
        let store: MetadataStoreV2 =
            serde_json::from_slice(bytes).map_err(|_| MetadataStoreError::DeserializeFailed)?;
        store.validate()?;
        let json_value: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|_| MetadataStoreError::DeserializeFailed)?;
        check_no_prohibited_keys(&json_value)?;

        atomic_write(&self.path, bytes, FileSensitivity::Metadata)
            .map_err(|e| MetadataStoreError::AtomicWriteFailed(e.to_string()))?;
        Ok(())
    }

    /// Save metadata payload atomically via encode and install_encoded.
    pub(crate) fn save(&self, store: &MetadataStoreV2) -> Result<(), MetadataStoreError> {
        let bytes = self.encode(store)?;
        self.install_encoded(&bytes)
    }
}

// ----- Tests --------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::path::PathBuf;

    // Synthetic test values — no realistic prefixes.
    const ID_TOKEN_A: &str = "synthetic-id-token-A";
    const ACCESS_TOKEN_A: &str = "synthetic-access-token-A";
    const REFRESH_TOKEN_A: &str = "synthetic-refresh-token-A";
    const API_KEY_A: &str = "synthetic-api-key-A";

    fn test_dir(tag: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("codex_meta_test_{}_{}", tag, rand::random::<u32>()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn account(id: &str, kind: MetadataAuthKind) -> AccountMetadataV2 {
        AccountMetadataV2 {
            id: id.to_string(),
            display_name: format!("User {id}"),
            email: Some(format!("{id}@example.com")),
            plan_type: Some("pro".to_string()),
            subscription_expires_at: None,
            created_at: Utc::now(),
            last_used_at: None,
            auth_kind: kind,
            vault_ref: id.to_string(), // vault_ref == id in schema v2
        }
    }

    // ---- MetadataAuthKind serialization tests ----

    #[test]
    fn test_metadata_auth_kind_chatgpt_serializes_as_chatgpt() {
        let json = serde_json::to_string(&MetadataAuthKind::ChatGpt).unwrap();
        assert_eq!(json, "\"chatgpt\"");
    }

    #[test]
    fn test_metadata_auth_kind_api_key_serializes_as_api_key() {
        let json = serde_json::to_string(&MetadataAuthKind::ApiKey).unwrap();
        assert_eq!(json, "\"api_key\"");
    }

    #[test]
    fn test_metadata_auth_kind_deserializes_exact_strings() {
        let chatgpt: MetadataAuthKind = serde_json::from_str("\"chatgpt\"").unwrap();
        let api_key: MetadataAuthKind = serde_json::from_str("\"api_key\"").unwrap();
        assert_eq!(chatgpt, MetadataAuthKind::ChatGpt);
        assert_eq!(api_key, MetadataAuthKind::ApiKey);
    }

    #[test]
    fn test_metadata_auth_kind_rejects_chat_gpt_and_unknown() {
        assert!(serde_json::from_str::<MetadataAuthKind>("\"chat_gpt\"").is_err());
        assert!(serde_json::from_str::<MetadataAuthKind>("\"unknown\"").is_err());
    }

    // ---- Validation ----

    #[test]
    fn test_metadata_empty_store_validates() {
        let s = MetadataStoreV2::new_empty();
        assert!(s.validate().is_ok());
    }

    #[test]
    fn test_metadata_empty_store_with_active_id_is_rejected() {
        let mut s = MetadataStoreV2::new_empty();
        s.active_account_id = Some("ghost".to_string());
        assert!(matches!(
            s.validate(),
            Err(MetadataStoreError::InvalidActiveAccount)
        ));
    }

    #[test]
    fn test_metadata_two_account_store_validates() {
        let mut s = MetadataStoreV2::new_empty();
        s.insert(account("acc-1", MetadataAuthKind::ChatGpt))
            .unwrap();
        s.insert(account("acc-2", MetadataAuthKind::ApiKey))
            .unwrap();
        assert!(s.validate().is_ok());
    }

    #[test]
    fn test_metadata_duplicate_account_id_rejected() {
        let mut s = MetadataStoreV2::new_empty();
        s.insert(account("acc-1", MetadataAuthKind::ChatGpt))
            .unwrap();
        let r = s.insert(account("acc-1", MetadataAuthKind::ApiKey));
        assert!(matches!(r, Err(MetadataStoreError::DuplicateAccountId)));
        assert_eq!(s.len(), 1); // store unchanged
    }

    #[test]
    fn test_metadata_duplicate_vault_ref_rejected() {
        let mut a1 = account("acc-a", MetadataAuthKind::ChatGpt);
        a1.vault_ref = "shared-ref".to_string();
        let mut a2 = account("acc-b", MetadataAuthKind::ApiKey);
        a2.vault_ref = "shared-ref".to_string();

        let store = MetadataStoreV2 {
            schema_version: METADATA_SCHEMA_VERSION,
            active_account_id: None,
            accounts: vec![a1, a2],
            masked_account_ids: Vec::new(),
        };

        let r = store.validate();
        assert!(matches!(
            r,
            Err(MetadataStoreError::DuplicateVaultReference)
        ));
    }

    #[test]
    fn test_metadata_unknown_active_account_rejected() {
        let mut s = MetadataStoreV2::new_empty();
        s.insert(account("acc-1", MetadataAuthKind::ChatGpt))
            .unwrap();
        let r = s.set_active(Some("unknown-id"));
        assert!(matches!(r, Err(MetadataStoreError::InvalidActiveAccount)));
        assert!(s.active_account_id.is_none()); // store unchanged
    }

    #[test]
    fn test_metadata_vault_ref_different_from_id_rejected() {
        let mut a = account("acc-1", MetadataAuthKind::ChatGpt);
        a.vault_ref = "different-ref".to_string();
        let store = MetadataStoreV2 {
            schema_version: METADATA_SCHEMA_VERSION,
            active_account_id: None,
            accounts: vec![a],
            masked_account_ids: Vec::new(),
        };
        let r = store.validate();
        assert!(matches!(r, Err(MetadataStoreError::InvalidVaultReference)));
    }

    #[test]
    fn test_metadata_whitespace_display_name_rejected() {
        let mut a = account("acc-1", MetadataAuthKind::ChatGpt);
        a.display_name = "   ".to_string();
        assert!(matches!(
            a.validate(),
            Err(MetadataStoreError::InvalidDisplayName)
        ));
    }

    #[test]
    fn test_metadata_padded_non_empty_display_name_accepted() {
        let mut a = account("acc-1", MetadataAuthKind::ChatGpt);
        a.display_name = "  Padded Name  ".to_string();
        assert!(a.validate().is_ok());
    }

    // ---- masked_account_ids tests ----

    #[test]
    fn test_metadata_masked_account_ids_default_empty() {
        let s = MetadataStoreV2::new_empty();
        assert!(s.masked_account_ids().is_empty());
    }

    #[test]
    fn test_metadata_masked_account_ids_preserves_ordering_stale_and_duplicates() {
        let mut s = MetadataStoreV2::new_empty();
        s.insert(account("acc-1", MetadataAuthKind::ChatGpt))
            .unwrap();
        s.set_masked_account_ids(vec![
            "stale-acc".to_string(),
            "acc-1".to_string(),
            "stale-acc".to_string(),
        ]);
        assert_eq!(s.masked_account_ids(), &["stale-acc", "acc-1", "stale-acc"]);
        assert!(s.validate().is_ok());
    }

    #[test]
    fn test_metadata_empty_optional_email_rejected() {
        let mut a = account("acc-1", MetadataAuthKind::ChatGpt);
        a.email = Some("".to_string());
        assert!(matches!(
            a.validate(),
            Err(MetadataStoreError::InvalidEmail)
        ));
    }

    #[test]
    fn test_metadata_whitespace_padded_email_rejected() {
        let mut a = account("acc-1", MetadataAuthKind::ChatGpt);
        a.email = Some(" user@example.com ".to_string());
        assert!(matches!(
            a.validate(),
            Err(MetadataStoreError::InvalidEmail)
        ));
    }

    #[test]
    fn test_metadata_empty_optional_plan_type_rejected() {
        let mut a = account("acc-1", MetadataAuthKind::ChatGpt);
        a.plan_type = Some("".to_string());
        assert!(matches!(
            a.validate(),
            Err(MetadataStoreError::InvalidPlanType)
        ));
    }

    #[test]
    fn test_metadata_whitespace_padded_plan_type_rejected() {
        let mut a = account("acc-1", MetadataAuthKind::ChatGpt);
        a.plan_type = Some(" pro ".to_string());
        assert!(matches!(
            a.validate(),
            Err(MetadataStoreError::InvalidPlanType)
        ));
    }

    #[test]
    fn test_metadata_insert_duplicate_does_not_mutate_store() {
        let mut s = MetadataStoreV2::new_empty();
        s.insert(account("acc-1", MetadataAuthKind::ChatGpt))
            .unwrap();
        let _ = s.insert(account("acc-1", MetadataAuthKind::ApiKey));
        assert_eq!(s.len(), 1);
        assert!(matches!(
            s.get("acc-1").unwrap().auth_kind,
            MetadataAuthKind::ChatGpt
        ));
    }

    #[test]
    fn test_metadata_remove_active_account_clears_active_id() {
        let mut s = MetadataStoreV2::new_empty();
        s.insert(account("acc-1", MetadataAuthKind::ChatGpt))
            .unwrap();
        s.set_active(Some("acc-1")).unwrap();
        assert_eq!(s.active_account_id.as_deref(), Some("acc-1"));
        s.remove("acc-1");
        assert!(s.active_account_id.is_none());
    }

    #[test]
    fn test_metadata_set_active_unknown_id_does_not_mutate_store() {
        let mut s = MetadataStoreV2::new_empty();
        s.insert(account("acc-1", MetadataAuthKind::ChatGpt))
            .unwrap();
        let _ = s.set_active(Some("acc-1")); // set first
        let r = s.set_active(Some("ghost"));
        assert!(r.is_err());
        // Active account must remain acc-1, not changed to ghost.
        assert_eq!(s.active_account_id.as_deref(), Some("acc-1"));
    }

    // ---- Prohibited fields ----

    #[test]
    fn test_metadata_prohibited_secret_field_structurally_detected() {
        // Build a JSON object that has a prohibited key nested inside.
        let bad: serde_json::Value = serde_json::json!({
            "accounts": [{
                "access_token": "should-be-rejected"
            }]
        });
        assert!(matches!(
            check_no_prohibited_keys(&bad),
            Err(MetadataStoreError::ProhibitedSecretField)
        ));
    }

    #[test]
    fn test_metadata_serialized_output_contains_no_prohibited_keys() {
        let mut s = MetadataStoreV2::new_empty();
        s.insert(account("acc-1", MetadataAuthKind::ChatGpt))
            .unwrap();
        let bytes = serde_json::to_vec_pretty(&s).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(check_no_prohibited_keys(&value).is_ok());
    }

    #[test]
    fn test_metadata_serialized_output_contains_no_synthetic_secret_values() {
        let mut s = MetadataStoreV2::new_empty();
        s.insert(account("acc-1", MetadataAuthKind::ChatGpt))
            .unwrap();
        let json = serde_json::to_string(&s).unwrap();
        for secret in &[ID_TOKEN_A, ACCESS_TOKEN_A, REFRESH_TOKEN_A, API_KEY_A] {
            assert!(
                !json.contains(secret),
                "Serialized metadata contains secret value '{secret}'"
            );
        }
    }

    // ---- Persistence ----

    #[test]
    fn test_metadata_missing_file_returns_missing_metadata_error() {
        let d = test_dir("missing");
        let store = MetadataFileStore::for_path(d.join("accounts.json"));
        assert!(matches!(
            store.load(),
            Err(MetadataStoreError::MissingMetadata)
        ));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn test_metadata_load_optional_missing_file_returns_none() {
        let d = test_dir("load_optional_missing");
        let store = MetadataFileStore::for_path(d.join("accounts.json"));
        assert!(matches!(store.load_optional(), Ok(None)));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn test_metadata_corrupt_json_is_not_treated_as_absent() {
        let d = test_dir("corrupt");
        let path = d.join("accounts.json");
        std::fs::write(&path, b"{ not valid json }").unwrap();
        let store = MetadataFileStore::for_path(path);
        // load_optional must NOT return Ok(None) for corrupt JSON.
        let r = store.load_optional();
        assert!(
            !matches!(r, Ok(None)),
            "Corrupt JSON was silently treated as absent"
        );
        assert!(r.is_err());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn test_metadata_unknown_json_field_rejected() {
        let d = test_dir("unknown_field");
        let path = d.join("accounts.json");
        // Write a JSON object with an extra unknown field.
        let json = serde_json::json!({
            "schema_version": 2,
            "active_account_id": null,
            "accounts": [],
            "unknown_extra_field": true
        })
        .to_string();
        std::fs::write(&path, json.as_bytes()).unwrap();
        let store = MetadataFileStore::for_path(path);
        let r = store.load();
        assert!(matches!(r, Err(MetadataStoreError::DeserializeFailed)));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn test_metadata_two_account_round_trip_succeeds() {
        let d = test_dir("round_trip");
        let store = MetadataFileStore::for_path(d.join("accounts.json"));

        let mut ms = MetadataStoreV2::new_empty();
        ms.insert(account("acc-1", MetadataAuthKind::ChatGpt))
            .unwrap();
        ms.insert(account("acc-2", MetadataAuthKind::ApiKey))
            .unwrap();
        ms.set_active(Some("acc-1")).unwrap();

        store.save(&ms).unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded.active_account_id.as_deref(), Some("acc-1"));
        assert!(loaded.get("acc-1").is_some());
        assert!(loaded.get("acc-2").is_some());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn test_metadata_failed_save_validation_preserves_existing_file() {
        let d = test_dir("preserve_on_fail");
        let store = MetadataFileStore::for_path(d.join("accounts.json"));

        let mut good = MetadataStoreV2::new_empty();
        good.insert(account("acc-1", MetadataAuthKind::ChatGpt))
            .unwrap();
        store.save(&good).unwrap();

        // Bad store: schema_version wrong.
        let bad = MetadataStoreV2 {
            schema_version: 99,
            active_account_id: None,
            accounts: Vec::new(),
            masked_account_ids: Vec::new(),
        };
        let r = store.save(&bad);
        assert!(r.is_err());

        // Original file must still be loadable.
        let loaded = store.load().unwrap();
        assert_eq!(loaded.len(), 1);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn test_metadata_successful_save_leaves_no_temporary_sibling_files() {
        let d = test_dir("no_temp_sibling");
        let path = d.join("accounts.json");
        let store = MetadataFileStore::for_path(path);

        let mut ms = MetadataStoreV2::new_empty();
        ms.insert(account("acc-1", MetadataAuthKind::ApiKey))
            .unwrap();
        store.save(&ms).unwrap();

        let entries: Vec<_> = std::fs::read_dir(&d)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();

        for name in &entries {
            assert!(
                !name.starts_with(".tmp_"),
                "Temporary file left beside accounts.json: {name}"
            );
        }
        let _ = std::fs::remove_dir_all(d);
    }
}
