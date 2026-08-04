//! Encrypted credential vault — Phase 1B-1.
//!
//! The vault stores secret credentials only.
//! It must never contain display names, email addresses, plan metadata,
//! timestamps, or active-account state.
//!
//! Binary envelope format:
//!   Bytes  0..4   ASCII magic "CSVT"
//!   Byte   4      Envelope version (== 1)
//!   Bytes  5..13  u64 little-endian ciphertext length
//!   Bytes  13..   DPAPI ciphertext

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

use crate::auth::atomic_file::{atomic_write, FileSensitivity};
use crate::auth::dpapi;
use crate::auth::paths::AppPaths;

// ----- Constants ----------------------------------------------------------------

const VAULT_MAGIC: &[u8; 4] = b"CSVT";
const VAULT_ENVELOPE_VERSION: u8 = 1;
const VAULT_PAYLOAD_SCHEMA_VERSION: u32 = 1;

/// Minimum valid envelope size: 4 (magic) + 1 (version) + 8 (len) = 13.
const ENVELOPE_HEADER_LEN: usize = 13;

// ----- Error model --------------------------------------------------------------

/// Typed, sanitized errors for vault operations.
/// Error messages must not include token contents, ciphertext, raw JSON,
/// decrypted payloads, or pointer values.
#[derive(Debug, thiserror::Error)]
pub(crate) enum VaultError {
    #[error("Vault file does not exist")]
    MissingVault,

    #[error("Failed to read vault file: {0}")]
    ReadFailed(std::io::Error),

    #[error("Vault envelope has invalid magic bytes")]
    InvalidMagic,

    #[error("Vault envelope version {0} is not supported")]
    UnsupportedEnvelopeVersion(u8),

    #[error("Vault envelope ciphertext length field is invalid")]
    InvalidCiphertextLength,

    #[error("Vault envelope is truncated")]
    TruncatedEnvelope,

    #[error("Vault envelope has trailing bytes after declared ciphertext")]
    TrailingEnvelopeData,

    #[error("DPAPI protect failed with HRESULT 0x{0:08X}")]
    ProtectFailed(u32),

    #[error("DPAPI unprotect failed with HRESULT 0x{0:08X}")]
    UnprotectFailed(u32),

    #[error("Failed to serialize vault payload")]
    PayloadSerializeFailed,

    #[error("Failed to deserialize vault payload")]
    PayloadDeserializeFailed,

    #[error("Vault payload schema version {0} is not supported")]
    UnsupportedPayloadSchema(u32),

    #[error("Account ID is invalid")]
    InvalidAccountId,

    #[error("Secret record contains an empty required field")]
    InvalidSecretRecord,

    #[error("Failed to write vault atomically: {0}")]
    AtomicWriteFailed(String),

    #[error("Round-trip validation of vault envelope failed before write")]
    EnvelopeValidationFailed,
}

// Map DPAPI errors without leaking details.
fn map_dpapi_protect(e: dpapi::DpapiError) -> VaultError {
    match e {
        dpapi::DpapiError::ProtectFailed(c) => VaultError::ProtectFailed(c),
        _ => VaultError::ProtectFailed(0),
    }
}

fn map_dpapi_unprotect(e: dpapi::DpapiError) -> VaultError {
    match e {
        dpapi::DpapiError::UnprotectFailed(c) => VaultError::UnprotectFailed(c),
        _ => VaultError::UnprotectFailed(0),
    }
}

// ----- Secret record ------------------------------------------------------------

/// A secret credential record. All string fields are zeroized on Drop.
///
/// Debug is manually implemented to redact values.
/// A secret credential record. All string fields are zeroized on Drop.
///
/// Debug is manually implemented to redact values.
#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "auth_kind", rename_all = "snake_case")]
pub(crate) enum SecretRecord {
    ChatGpt {
        id_token: String,
        access_token: String,
        refresh_token: String,
        account_id: String,
    },
    ApiKey {
        key: String,
    },
}

impl fmt::Debug for SecretRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SecretRecord::ChatGpt { account_id: _, .. } => f
                .debug_struct("SecretRecord::ChatGpt")
                .field("account_id", &"[REDACTED]")
                .field("id_token", &"[REDACTED]")
                .field("access_token", &"[REDACTED]")
                .field("refresh_token", &"[REDACTED]")
                .finish(),
            SecretRecord::ApiKey { .. } => f
                .debug_struct("SecretRecord::ApiKey")
                .field("key", &"[REDACTED]")
                .finish(),
        }
    }
}

impl Drop for SecretRecord {
    fn drop(&mut self) {
        match self {
            SecretRecord::ChatGpt {
                id_token,
                access_token,
                refresh_token,
                account_id,
            } => {
                id_token.zeroize();
                access_token.zeroize();
                refresh_token.zeroize();
                account_id.zeroize();
            }
            SecretRecord::ApiKey { key } => {
                key.zeroize();
            }
        }
    }
}

impl SecretRecord {
    /// Validate that all required fields are non-empty.
    fn validate(&self) -> Result<(), VaultError> {
        match self {
            SecretRecord::ChatGpt {
                id_token,
                access_token,
                refresh_token,
                account_id,
            } => {
                if id_token.is_empty()
                    || access_token.is_empty()
                    || refresh_token.is_empty()
                    || account_id.is_empty()
                {
                    return Err(VaultError::InvalidSecretRecord);
                }
            }
            SecretRecord::ApiKey { key } => {
                if key.is_empty() {
                    return Err(VaultError::InvalidSecretRecord);
                }
            }
        }
        Ok(())
    }
}

// ----- Vault payload V1 ---------------------------------------------------------

/// Deserialised vault payload. BTreeMap gives deterministic JSON key order.
#[derive(Serialize, Deserialize)]
pub(crate) struct VaultPayloadV1 {
    pub(crate) schema_version: u32,
    pub(crate) accounts: BTreeMap<String, SecretRecord>,
}

impl fmt::Debug for VaultPayloadV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VaultPayloadV1")
            .field("schema_version", &self.schema_version)
            .field("account_count", &self.accounts.len())
            .finish()
    }
}

impl Drop for VaultPayloadV1 {
    fn drop(&mut self) {
        // Individual SecretRecord values zeroize themselves.
        self.accounts.clear();
    }
}

/// Validate a single account_id key.
fn validate_account_id(id: &str) -> Result<(), VaultError> {
    if id.is_empty() || id.trim() != id {
        return Err(VaultError::InvalidAccountId);
    }
    Ok(())
}

impl VaultPayloadV1 {
    pub(crate) fn new_empty() -> Self {
        Self {
            schema_version: VAULT_PAYLOAD_SCHEMA_VERSION,
            accounts: BTreeMap::new(),
        }
    }

    pub(crate) fn get(&self, account_id: &str) -> Option<&SecretRecord> {
        self.accounts.get(account_id)
    }

    /// Insert (or replace) a secret record. Returns the previous record if any.
    /// Fails if the account_id or record is invalid; on failure no mutation occurs.
    pub(crate) fn insert(
        &mut self,
        account_id: &str,
        secret: SecretRecord,
    ) -> Result<Option<SecretRecord>, VaultError> {
        validate_account_id(account_id)?;
        secret.validate()?;
        let prev = self.accounts.insert(account_id.to_string(), secret);
        Ok(prev)
    }

    pub(crate) fn remove(&mut self, account_id: &str) -> Option<SecretRecord> {
        self.accounts.remove(account_id)
    }

    pub(crate) fn contains(&self, account_id: &str) -> bool {
        self.accounts.contains_key(account_id)
    }

    pub(crate) fn len(&self) -> usize {
        self.accounts.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.accounts.is_empty()
    }

    /// Full validation of payload structure and contents.
    pub(crate) fn validate(&self) -> Result<(), VaultError> {
        if self.schema_version != VAULT_PAYLOAD_SCHEMA_VERSION {
            return Err(VaultError::UnsupportedPayloadSchema(self.schema_version));
        }
        for (id, record) in &self.accounts {
            validate_account_id(id)?;
            record.validate()?;
        }
        Ok(())
    }
}

// ----- Envelope -----------------------------------------------------------------

/// Checked calculation of total envelope capacity header + ciphertext.
fn checked_envelope_capacity(ct_len: usize) -> Result<usize, VaultError> {
    ENVELOPE_HEADER_LEN
        .checked_add(ct_len)
        .ok_or(VaultError::InvalidCiphertextLength)
}

/// Build the binary envelope: CSVT | version | u64-LE len | ciphertext.
fn build_envelope(ciphertext: &[u8]) -> Result<Vec<u8>, VaultError> {
    let ct_len_u64 =
        u64::try_from(ciphertext.len()).map_err(|_| VaultError::InvalidCiphertextLength)?;
    if ct_len_u64 == 0 {
        return Err(VaultError::InvalidCiphertextLength);
    }
    let total_len = checked_envelope_capacity(ciphertext.len())?;
    let mut buf = Vec::with_capacity(total_len);
    buf.extend_from_slice(VAULT_MAGIC);
    buf.push(VAULT_ENVELOPE_VERSION);
    buf.extend_from_slice(&ct_len_u64.to_le_bytes());
    buf.extend_from_slice(ciphertext);
    Ok(buf)
}

/// Parse and validate the binary envelope. Returns only the ciphertext slice.
fn parse_envelope(data: &[u8]) -> Result<&[u8], VaultError> {
    if data.len() < ENVELOPE_HEADER_LEN {
        return Err(VaultError::TruncatedEnvelope);
    }

    // Magic
    let magic = &data[0..4];
    if magic != VAULT_MAGIC {
        return Err(VaultError::InvalidMagic);
    }

    // Envelope version
    let version = data[4];
    if version != VAULT_ENVELOPE_VERSION {
        return Err(VaultError::UnsupportedEnvelopeVersion(version));
    }

    // Ciphertext length (u64 LE)
    let len_bytes: [u8; 8] = data[5..13]
        .try_into()
        .map_err(|_| VaultError::TruncatedEnvelope)?;
    let ct_len_u64 = u64::from_le_bytes(len_bytes);

    if ct_len_u64 == 0 {
        return Err(VaultError::InvalidCiphertextLength);
    }

    // Safe checked conversion to usize.
    let ct_len = usize::try_from(ct_len_u64).map_err(|_| VaultError::InvalidCiphertextLength)?;

    let body = &data[ENVELOPE_HEADER_LEN..];

    if body.len() < ct_len {
        return Err(VaultError::TruncatedEnvelope);
    }
    if body.len() > ct_len {
        return Err(VaultError::TrailingEnvelopeData);
    }

    Ok(body)
}

// ----- VaultStore ---------------------------------------------------------------

pub(crate) struct VaultStore {
    path: PathBuf,
}

impl VaultStore {
    pub(crate) fn from_paths(paths: &AppPaths) -> Self {
        Self {
            path: paths.vault_file.clone(),
        }
    }

    pub(crate) fn for_path(path: PathBuf) -> Self {
        Self { path }
    }

    pub(crate) fn exists(&self) -> bool {
        self.path.exists()
    }

    /// Load the vault from disk. Does not create or repair the vault on error.
    pub(crate) fn load(&self) -> Result<VaultPayloadV1, VaultError> {
        if !self.path.exists() {
            return Err(VaultError::MissingVault);
        }
        let raw = std::fs::read(&self.path).map_err(VaultError::ReadFailed)?;
        let ciphertext = parse_envelope(&raw)?;
        let plaintext = dpapi::unprotect(ciphertext).map_err(map_dpapi_unprotect)?;
        let payload: VaultPayloadV1 =
            serde_json::from_slice(&plaintext).map_err(|_| VaultError::PayloadDeserializeFailed)?;
        payload.validate()?;
        Ok(payload)
    }

    /// Save the vault atomically.
    ///
    /// Full procedure:
    /// 1. Validate payload
    /// 2. Serialize directly to Zeroizing<Vec<u8>>
    /// 3. DPAPI-protect
    /// 4. Build envelope
    /// 5. Validate envelope in memory (parse + unprotect + deserialize + compare)
    /// 6. Write atomically — only after validation passes
    pub(crate) fn save(&self, payload: &VaultPayloadV1) -> Result<(), VaultError> {
        // Step 1 — Validate.
        payload.validate()?;

        // Step 2 — Serialize directly into a Zeroizing buffer.
        let mut plaintext = Zeroizing::new(Vec::new());
        serde_json::to_writer(&mut *plaintext, payload)
            .map_err(|_| VaultError::PayloadSerializeFailed)?;

        // Step 3 — DPAPI protect.
        let ciphertext = dpapi::protect(&plaintext).map_err(map_dpapi_protect)?;

        // Step 4 — Build envelope.
        let envelope = build_envelope(&ciphertext)?;

        // Step 5 — In-memory round-trip validation.
        self.validate_envelope_round_trip(&envelope, payload)?;

        // Step 6 — Atomic write.
        atomic_write(&self.path, &envelope, FileSensitivity::Secret)
            .map_err(|e| VaultError::AtomicWriteFailed(e.to_string()))?;

        Ok(())
    }

    /// Parse the in-memory envelope, decrypt, deserialize, and compare with the
    /// original payload. Does not touch the production file.
    fn validate_envelope_round_trip(
        &self,
        envelope: &[u8],
        original: &VaultPayloadV1,
    ) -> Result<(), VaultError> {
        let ciphertext =
            parse_envelope(envelope).map_err(|_| VaultError::EnvelopeValidationFailed)?;
        let plaintext =
            dpapi::unprotect(ciphertext).map_err(|_| VaultError::EnvelopeValidationFailed)?;
        let recovered: VaultPayloadV1 =
            serde_json::from_slice(&plaintext).map_err(|_| VaultError::EnvelopeValidationFailed)?;
        recovered
            .validate()
            .map_err(|_| VaultError::EnvelopeValidationFailed)?;

        // Compare logical contents.
        if recovered.schema_version != original.schema_version
            || recovered.accounts.len() != original.accounts.len()
        {
            return Err(VaultError::EnvelopeValidationFailed);
        }
        for (id, rec) in &original.accounts {
            match recovered.accounts.get(id) {
                Some(r) if r == rec => {}
                _ => return Err(VaultError::EnvelopeValidationFailed),
            }
        }
        Ok(())
    }
}

// ----- Tests --------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // --- Synthetic credential constants (no realistic prefixes) ---

    const ID_TOKEN_A: &str = "synthetic-id-token-A";
    const ACCESS_TOKEN_A: &str = "synthetic-access-token-A";
    const REFRESH_TOKEN_A: &str = "synthetic-refresh-token-A";
    const API_KEY_A: &str = "synthetic-api-key-A";
    const ACCOUNT_ID_A: &str = "account-A";

    fn chatgpt_record() -> SecretRecord {
        SecretRecord::ChatGpt {
            id_token: ID_TOKEN_A.to_string(),
            access_token: ACCESS_TOKEN_A.to_string(),
            refresh_token: REFRESH_TOKEN_A.to_string(),
            account_id: ACCOUNT_ID_A.to_string(),
        }
    }

    fn api_key_record() -> SecretRecord {
        SecretRecord::ApiKey {
            key: API_KEY_A.to_string(),
        }
    }

    fn test_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "codex_vault_test_{}_{}",
            tag,
            rand::random::<u32>()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    // ---- Payload mutation helpers ----

    #[test]
    fn test_vault_empty_payload_validates() {
        let p = VaultPayloadV1::new_empty();
        assert!(p.validate().is_ok());
        assert!(p.is_empty());
    }

    #[test]
    fn test_vault_chatgpt_record_insert_and_retrieve() {
        let mut p = VaultPayloadV1::new_empty();
        p.insert(ACCOUNT_ID_A, chatgpt_record()).unwrap();
        assert!(p.contains(ACCOUNT_ID_A));
        let r = p.get(ACCOUNT_ID_A).unwrap();
        assert!(matches!(r, SecretRecord::ChatGpt { .. }));
    }

    #[test]
    fn test_vault_api_key_record_insert_and_retrieve() {
        let mut p = VaultPayloadV1::new_empty();
        p.insert(ACCOUNT_ID_A, api_key_record()).unwrap();
        let r = p.get(ACCOUNT_ID_A).unwrap();
        assert!(matches!(r, SecretRecord::ApiKey { .. }));
    }

    #[test]
    fn test_vault_existing_id_replacement_returns_old_record() {
        let mut p = VaultPayloadV1::new_empty();
        p.insert(ACCOUNT_ID_A, chatgpt_record()).unwrap();
        let old = p.insert(ACCOUNT_ID_A, api_key_record()).unwrap();
        assert!(old.is_some());
        assert!(matches!(old.unwrap(), SecretRecord::ChatGpt { .. }));
        assert!(matches!(
            p.get(ACCOUNT_ID_A).unwrap(),
            SecretRecord::ApiKey { .. }
        ));
    }

    #[test]
    fn test_vault_removing_missing_id_is_safe() {
        let mut p = VaultPayloadV1::new_empty();
        let r = p.remove("nonexistent");
        assert!(r.is_none());
    }

    #[test]
    fn test_vault_empty_account_id_rejected() {
        let mut p = VaultPayloadV1::new_empty();
        let r = p.insert("", chatgpt_record());
        assert!(matches!(r, Err(VaultError::InvalidAccountId)));
    }

    #[test]
    fn test_vault_whitespace_padded_account_id_rejected() {
        let mut p = VaultPayloadV1::new_empty();
        let r = p.insert(" account-A ", chatgpt_record());
        assert!(matches!(r, Err(VaultError::InvalidAccountId)));
    }

    #[test]
    fn test_vault_empty_secret_field_rejected() {
        let mut p = VaultPayloadV1::new_empty();
        let bad = SecretRecord::ChatGpt {
            id_token: "".to_string(),
            access_token: ACCESS_TOKEN_A.to_string(),
            refresh_token: REFRESH_TOKEN_A.to_string(),
            account_id: ACCOUNT_ID_A.to_string(),
        };
        let r = p.insert(ACCOUNT_ID_A, bad);
        assert!(matches!(r, Err(VaultError::InvalidSecretRecord)));
    }

    // ---- Envelope parsing ----

    #[test]
    fn test_vault_envelope_wrong_magic_rejected() {
        let mut buf = b"XXXX".to_vec();
        buf.push(VAULT_ENVELOPE_VERSION);
        buf.extend_from_slice(&8u64.to_le_bytes());
        buf.extend_from_slice(&[0u8; 8]);
        assert!(matches!(
            parse_envelope(&buf),
            Err(VaultError::InvalidMagic)
        ));
    }

    #[test]
    fn test_vault_envelope_unknown_version_rejected() {
        let mut buf = VAULT_MAGIC.to_vec();
        buf.push(99); // unknown version
        buf.extend_from_slice(&8u64.to_le_bytes());
        buf.extend_from_slice(&[0u8; 8]);
        assert!(matches!(
            parse_envelope(&buf),
            Err(VaultError::UnsupportedEnvelopeVersion(99))
        ));
    }

    #[test]
    fn test_vault_envelope_truncated_header_rejected() {
        let buf = b"CSV".to_vec(); // < 13 bytes
        assert!(matches!(
            parse_envelope(&buf),
            Err(VaultError::TruncatedEnvelope)
        ));
    }

    #[test]
    fn test_vault_envelope_truncated_body_rejected() {
        let mut buf = VAULT_MAGIC.to_vec();
        buf.push(VAULT_ENVELOPE_VERSION);
        buf.extend_from_slice(&100u64.to_le_bytes()); // claims 100 bytes
        buf.extend_from_slice(&[0u8; 10]); // but only 10 bytes
        assert!(matches!(
            parse_envelope(&buf),
            Err(VaultError::TruncatedEnvelope)
        ));
    }

    #[test]
    fn test_vault_envelope_trailing_bytes_rejected() {
        let ct = vec![0u8; 8];
        let mut buf = VAULT_MAGIC.to_vec();
        buf.push(VAULT_ENVELOPE_VERSION);
        buf.extend_from_slice(&8u64.to_le_bytes()); // declare 8 bytes
        buf.extend_from_slice(&ct); // exactly 8 bytes ...
        buf.push(0x00); // ... plus one extra
        assert!(matches!(
            parse_envelope(&buf),
            Err(VaultError::TrailingEnvelopeData)
        ));
    }

    #[test]
    fn test_vault_envelope_declared_length_larger_than_remaining_rejected() {
        // Declare 50 but only 4 bytes follow.
        let mut buf = VAULT_MAGIC.to_vec();
        buf.push(VAULT_ENVELOPE_VERSION);
        buf.extend_from_slice(&50u64.to_le_bytes());
        buf.extend_from_slice(&[0u8; 4]);
        assert!(matches!(
            parse_envelope(&buf),
            Err(VaultError::TruncatedEnvelope)
        ));
    }

    #[test]
    fn test_vault_envelope_declared_length_smaller_than_remaining_rejected() {
        // Declare 4 but 8 bytes follow.
        let mut buf = VAULT_MAGIC.to_vec();
        buf.push(VAULT_ENVELOPE_VERSION);
        buf.extend_from_slice(&4u64.to_le_bytes());
        buf.extend_from_slice(&[0u8; 8]);
        assert!(matches!(
            parse_envelope(&buf),
            Err(VaultError::TrailingEnvelopeData)
        ));
    }

    #[test]
    fn test_vault_envelope_empty_ciphertext_rejected() {
        let mut buf = VAULT_MAGIC.to_vec();
        buf.push(VAULT_ENVELOPE_VERSION);
        buf.extend_from_slice(&0u64.to_le_bytes()); // 0 length
                                                    // no body
        assert!(matches!(
            parse_envelope(&buf),
            Err(VaultError::InvalidCiphertextLength)
        ));
    }

    #[test]
    fn test_vault_unsupported_payload_schema_rejected() {
        let bad = VaultPayloadV1 {
            schema_version: 99,
            accounts: BTreeMap::new(),
        };
        assert!(matches!(
            bad.validate(),
            Err(VaultError::UnsupportedPayloadSchema(99))
        ));
    }

    #[test]
    fn test_vault_missing_vault_returns_missing_vault_error() {
        let d = test_dir("missing");
        let store = VaultStore::for_path(d.join("vault.dat"));
        let r = store.load();
        assert!(matches!(r, Err(VaultError::MissingVault)));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn test_vault_dat_is_not_valid_json() {
        // Build a real envelope (cross-platform pure-envelope test; payload is fake JSON).
        let fake_ct = b"definitely-not-json-payload";
        // build_envelope with zero-length fails; test the binary format itself.
        let envelope = build_envelope(fake_ct).unwrap();
        // Envelope is not valid JSON.
        assert!(serde_json::from_slice::<serde_json::Value>(&envelope).is_err());
    }

    #[test]
    fn test_vault_error_messages_contain_no_synthetic_secrets() {
        let secrets = [ID_TOKEN_A, ACCESS_TOKEN_A, REFRESH_TOKEN_A, API_KEY_A];
        let errors: &[&dyn std::fmt::Display] = &[
            &VaultError::MissingVault,
            &VaultError::InvalidMagic,
            &VaultError::UnsupportedEnvelopeVersion(99),
            &VaultError::InvalidCiphertextLength,
            &VaultError::TruncatedEnvelope,
            &VaultError::TrailingEnvelopeData,
            &VaultError::ProtectFailed(0x8009_0006),
            &VaultError::UnprotectFailed(0x8009_0006),
            &VaultError::PayloadSerializeFailed,
            &VaultError::PayloadDeserializeFailed,
            &VaultError::UnsupportedPayloadSchema(99),
            &VaultError::InvalidAccountId,
            &VaultError::InvalidSecretRecord,
        ];
        for err in errors {
            let msg = err.to_string();
            for secret in &secrets {
                assert!(
                    !msg.contains(secret),
                    "Error message '{msg}' contains secret '{secret}'"
                );
            }
        }
    }

    // ---- Windows DPAPI-dependent tests ----

    #[test]
    #[cfg(windows)]
    fn test_vault_envelope_round_trip_succeeds() {
        let mut payload = VaultPayloadV1::new_empty();
        payload.insert(ACCOUNT_ID_A, chatgpt_record()).unwrap();

        let plaintext = Zeroizing::new(serde_json::to_vec(&payload).unwrap());
        let ciphertext = dpapi::protect(&plaintext).unwrap();
        let envelope = build_envelope(&ciphertext).unwrap();

        let ct_back = parse_envelope(&envelope).unwrap();
        let pt_back = dpapi::unprotect(ct_back).unwrap();
        let payload_back: VaultPayloadV1 = serde_json::from_slice(&pt_back).unwrap();

        assert_eq!(payload_back.schema_version, VAULT_PAYLOAD_SCHEMA_VERSION);
        assert!(payload_back.contains(ACCOUNT_ID_A));
    }

    #[test]
    #[cfg(windows)]
    fn test_vault_corrupted_dpapi_ciphertext_fails_closed() {
        let d = test_dir("corrupt");
        let store = VaultStore::for_path(d.join("vault.dat"));

        // Build a syntactically valid envelope with garbage ciphertext.
        let ct = vec![0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x01, 0x02, 0x03];
        let envelope = build_envelope(&ct).unwrap();
        std::fs::write(&store.path, &envelope).unwrap();

        let r = store.load();
        assert!(
            matches!(r, Err(VaultError::UnprotectFailed(_))),
            "Expected UnprotectFailed, got: {:?}",
            r
        );
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    #[cfg(windows)]
    fn test_vault_malformed_decrypted_json_fails_closed() {
        // Protect some bytes that are valid DPAPI but not valid JSON payload.
        let garbage_json = b"{not valid json for VaultPayloadV1}";
        let ct = dpapi::protect(garbage_json).unwrap();
        let envelope = build_envelope(&ct).unwrap();

        let d = test_dir("malformed_json");
        let path = d.join("vault.dat");
        std::fs::write(&path, &envelope).unwrap();

        let store = VaultStore::for_path(path);
        let r = store.load();
        assert!(
            matches!(r, Err(VaultError::PayloadDeserializeFailed)),
            "Expected PayloadDeserializeFailed, got: {:?}",
            r
        );
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    #[cfg(windows)]
    fn test_vault_save_then_load_round_trip_succeeds() {
        let d = test_dir("roundtrip");
        let store = VaultStore::for_path(d.join("vault.dat"));

        let mut payload = VaultPayloadV1::new_empty();
        payload.insert(ACCOUNT_ID_A, chatgpt_record()).unwrap();
        payload.insert("account-B", api_key_record()).unwrap();

        store.save(&payload).unwrap();
        assert!(store.exists());

        let loaded = store.load().unwrap();
        assert_eq!(loaded.len(), 2);
        assert!(loaded.contains(ACCOUNT_ID_A));
        assert!(loaded.contains("account-B"));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    #[cfg(windows)]
    fn test_vault_failed_save_validation_preserves_existing_valid_vault() {
        let d = test_dir("preserve_on_fail");
        let store = VaultStore::for_path(d.join("vault.dat"));

        let mut good = VaultPayloadV1::new_empty();
        good.insert(ACCOUNT_ID_A, chatgpt_record()).unwrap();
        store.save(&good).unwrap();

        // Attempt to save an invalid payload.
        let bad = VaultPayloadV1 {
            schema_version: 99,
            accounts: BTreeMap::new(),
        };
        let r = store.save(&bad);
        assert!(r.is_err());

        // Original vault must still be loadable.
        let loaded = store.load().unwrap();
        assert_eq!(loaded.len(), 1);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    #[cfg(windows)]
    fn test_vault_no_plaintext_temp_files_beside_vault_dat() {
        let d = test_dir("no_plaintext_temp");
        let vault_path = d.join("vault.dat");
        let store = VaultStore::for_path(vault_path);

        let mut payload = VaultPayloadV1::new_empty();
        payload.insert(ACCOUNT_ID_A, api_key_record()).unwrap();
        store.save(&payload).unwrap();

        // After save, only vault.dat must exist (plus maybe nothing else).
        let entries: Vec<_> = std::fs::read_dir(&d)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();

        for name in &entries {
            assert!(
                !name.ends_with(".json"),
                "Unexpected .json file beside vault: {name}"
            );
            assert!(
                !name.starts_with(".tmp_"),
                "Temporary file left beside vault: {name}"
            );
        }
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    #[cfg(windows)]
    fn test_vault_dat_raw_bytes_contain_no_secrets() {
        let d = test_dir("raw_bytes_secrecy");
        let vault_path = d.join("vault.dat");
        let store = VaultStore::for_path(vault_path);

        let mut payload = VaultPayloadV1::new_empty();
        payload.insert(ACCOUNT_ID_A, chatgpt_record()).unwrap();
        payload.insert("account-B", api_key_record()).unwrap();

        store.save(&payload).unwrap();

        let raw_bytes = std::fs::read(&store.path).unwrap();

        // Assert file is not valid JSON
        assert!(serde_json::from_slice::<serde_json::Value>(&raw_bytes).is_err());

        // Assert file raw bytes contain no synthetic secrets
        let secrets = [
            ID_TOKEN_A.as_bytes(),
            ACCESS_TOKEN_A.as_bytes(),
            REFRESH_TOKEN_A.as_bytes(),
            API_KEY_A.as_bytes(),
        ];

        for secret in secrets {
            let found = raw_bytes
                .windows(secret.len())
                .any(|window| window == secret);
            assert!(!found, "Raw vault file contains synthetic secret");
        }

        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn test_checked_envelope_capacity_normal() {
        let cap = checked_envelope_capacity(100).unwrap();
        assert_eq!(cap, 113);
    }

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn test_checked_envelope_capacity_overflow() {
        let max = usize::MAX;
        let res = checked_envelope_capacity(max);
        assert!(matches!(res, Err(VaultError::InvalidCiphertextLength)));
    }
}
