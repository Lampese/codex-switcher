//! Account storage module - manages reading and writing accounts.json

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD_NO_PAD, Engine as _};
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    XChaCha20Poly1305, XNonce,
};
use chrono::{DateTime, Utc};
use rand::RngCore;

use crate::types::{AccountsStore, AuthData, StoredAccount};

const ENCRYPTED_STORE_PREFIX: &str = "cswa1.";
const STORE_KEY_SERVICE: &str = "codex-switcher";
const STORE_KEY_ACCOUNT: &str = "accounts-store";
const STORE_KEY_BYTES: usize = 32;
const STORE_NONCE_BYTES: usize = 24;

/// Get the path to the codex-switcher config directory
pub fn get_config_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not find home directory")?;
    Ok(home.join(".codex-switcher"))
}

/// Get the path to accounts.json
pub fn get_accounts_file() -> Result<PathBuf> {
    Ok(get_config_dir()?.join("accounts.json"))
}

/// Load the accounts store from disk
pub fn load_accounts() -> Result<AccountsStore> {
    let path = get_accounts_file()?;

    if !path.exists() {
        return Ok(AccountsStore::default());
    }

    let content = fs::read(&path)
        .with_context(|| format!("Failed to read accounts file: {}", path.display()))?;

    let json = if content.starts_with(ENCRYPTED_STORE_PREFIX.as_bytes()) {
        let key = get_or_create_store_key()?;
        decrypt_accounts_store_json(&content, &key)?
    } else {
        content
    };

    let store: AccountsStore = serde_json::from_slice(&json)
        .with_context(|| format!("Failed to parse accounts file: {}", path.display()))?;

    Ok(store)
}

/// Save the accounts store to disk
pub fn save_accounts(store: &AccountsStore) -> Result<()> {
    let path = get_accounts_file()?;

    // Ensure the config directory exists
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create config directory: {}", parent.display()))?;
    }

    let content = serde_json::to_vec_pretty(store).context("Failed to serialize accounts store")?;
    let key = get_or_create_store_key()?;
    let encrypted = encrypt_accounts_store_json(&content, &key)?;

    fs::write(&path, encrypted)
        .with_context(|| format!("Failed to write accounts file: {}", path.display()))?;

    // Set restrictive permissions on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o600);
        fs::set_permissions(&path, perms)?;
    }

    Ok(())
}

fn get_or_create_store_key() -> Result<[u8; STORE_KEY_BYTES]> {
    initialize_native_store()?;
    let entry = keyring_core::Entry::new(STORE_KEY_SERVICE, STORE_KEY_ACCOUNT)
        .context("Failed to open OS credential store")?;

    match entry.get_password() {
        Ok(encoded) => decode_store_key(&encoded),
        Err(keyring_core::Error::NoEntry) => {
            let mut key = [0u8; STORE_KEY_BYTES];
            rand::rng().fill_bytes(&mut key);
            entry
                .set_password(&STANDARD_NO_PAD.encode(key))
                .context("Failed to save account-store key to OS credential store")?;
            Ok(key)
        }
        Err(error) => {
            Err(error).context("Failed to read account-store key from OS credential store")
        }
    }
}

fn initialize_native_store() -> Result<()> {
    #[cfg(target_os = "windows")]
    {
        keyring_core::set_default_store(
            windows_native_keyring_store::Store::new()
                .context("Failed to initialize Windows Credential Manager")?,
        );
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    {
        keyring_core::set_default_store(
            apple_native_keyring_store::keychain::Store::new()
                .context("Failed to initialize macOS Keychain")?,
        );
        return Ok(());
    }

    #[cfg(target_os = "linux")]
    {
        keyring_core::set_default_store(
            zbus_secret_service_keyring_store::Store::new()
                .context("Failed to initialize Linux Secret Service")?,
        );
        return Ok(());
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        anyhow::bail!("No native credential store is configured for this platform")
    }
}

fn decode_store_key(encoded: &str) -> Result<[u8; STORE_KEY_BYTES]> {
    let decoded = STANDARD_NO_PAD
        .decode(encoded.trim())
        .context("Stored account encryption key is invalid base64")?;
    let key: [u8; STORE_KEY_BYTES] = decoded
        .try_into()
        .map_err(|_| anyhow::anyhow!("Stored account encryption key has invalid length"))?;
    Ok(key)
}

fn encrypt_accounts_store_json(json: &[u8], key: &[u8; STORE_KEY_BYTES]) -> Result<Vec<u8>> {
    let mut nonce = [0u8; STORE_NONCE_BYTES];
    rand::rng().fill_bytes(&mut nonce);

    let cipher = XChaCha20Poly1305::new(key.into());
    let ciphertext = cipher
        .encrypt(XNonce::from_slice(&nonce), json)
        .map_err(|_| anyhow::anyhow!("Failed to encrypt accounts store"))?;

    let mut payload = Vec::with_capacity(STORE_NONCE_BYTES + ciphertext.len());
    payload.extend_from_slice(&nonce);
    payload.extend_from_slice(&ciphertext);

    Ok(format!(
        "{ENCRYPTED_STORE_PREFIX}{}",
        STANDARD_NO_PAD.encode(payload)
    )
    .into_bytes())
}

fn decrypt_accounts_store_json(encrypted: &[u8], key: &[u8; STORE_KEY_BYTES]) -> Result<Vec<u8>> {
    let encoded = std::str::from_utf8(encrypted)
        .context("Encrypted accounts store is not UTF-8")?
        .strip_prefix(ENCRYPTED_STORE_PREFIX)
        .context("Encrypted accounts store header is invalid")?;

    let payload = STANDARD_NO_PAD
        .decode(encoded)
        .context("Encrypted accounts store payload is invalid base64")?;

    if payload.len() <= STORE_NONCE_BYTES {
        anyhow::bail!("Encrypted accounts store is truncated");
    }

    let (nonce, ciphertext) = payload.split_at(STORE_NONCE_BYTES);
    let cipher = XChaCha20Poly1305::new(key.into());
    cipher
        .decrypt(XNonce::from_slice(nonce), ciphertext)
        .map_err(|_| anyhow::anyhow!("Failed to decrypt accounts store"))
}

/// Add a new account to the store
pub fn add_account(account: StoredAccount) -> Result<StoredAccount> {
    let mut store = load_accounts()?;

    // Check for duplicate names
    if store.accounts.iter().any(|a| a.name == account.name) {
        anyhow::bail!("An account with name '{}' already exists", account.name);
    }

    let account_clone = account.clone();
    store.accounts.push(account);

    // If this is the first account, make it active
    if store.accounts.len() == 1 {
        store.active_account_id = Some(account_clone.id.clone());
    }

    save_accounts(&store)?;
    Ok(account_clone)
}

/// Remove an account by ID
pub fn remove_account(account_id: &str) -> Result<()> {
    let mut store = load_accounts()?;

    let initial_len = store.accounts.len();
    store.accounts.retain(|a| a.id != account_id);

    if store.accounts.len() == initial_len {
        anyhow::bail!("Account not found: {account_id}");
    }

    // If we removed the active account, clear it or set to first available
    if store.active_account_id.as_deref() == Some(account_id) {
        store.active_account_id = store.accounts.first().map(|a| a.id.clone());
    }

    save_accounts(&store)?;
    Ok(())
}

/// Update the active account ID
pub fn set_active_account(account_id: &str) -> Result<()> {
    let mut store = load_accounts()?;

    // Verify the account exists
    if !store.accounts.iter().any(|a| a.id == account_id) {
        anyhow::bail!("Account not found: {account_id}");
    }

    store.active_account_id = Some(account_id.to_string());
    save_accounts(&store)?;
    Ok(())
}

/// Get an account by ID
pub fn get_account(account_id: &str) -> Result<Option<StoredAccount>> {
    let store = load_accounts()?;
    Ok(store.accounts.into_iter().find(|a| a.id == account_id))
}

/// Get the currently active account
pub fn get_active_account() -> Result<Option<StoredAccount>> {
    let store = load_accounts()?;
    let active_id = match &store.active_account_id {
        Some(id) => id,
        None => return Ok(None),
    };
    Ok(store.accounts.into_iter().find(|a| a.id == *active_id))
}

/// Update an account's last_used_at timestamp
pub fn touch_account(account_id: &str) -> Result<()> {
    let mut store = load_accounts()?;

    if let Some(account) = store.accounts.iter_mut().find(|a| a.id == account_id) {
        account.last_used_at = Some(chrono::Utc::now());
        save_accounts(&store)?;
    }

    Ok(())
}

/// Update an account's metadata (name, email, plan_type, subscription expiry)
pub fn update_account_metadata(
    account_id: &str,
    name: Option<String>,
    email: Option<String>,
    plan_type: Option<String>,
    subscription_expires_at: Option<Option<DateTime<Utc>>>,
) -> Result<StoredAccount> {
    let mut store = load_accounts()?;

    // Check for duplicate names first (if renaming)
    if let Some(ref new_name) = name {
        if store
            .accounts
            .iter()
            .any(|a| a.id != account_id && a.name == *new_name)
        {
            anyhow::bail!("An account with name '{new_name}' already exists");
        }
    }

    // Now find and update the account
    let account = store
        .accounts
        .iter_mut()
        .find(|a| a.id == account_id)
        .context("Account not found")?;

    if let Some(new_name) = name {
        account.name = new_name;
    }

    if email.is_some() {
        account.email = email;
    }

    if plan_type.is_some() {
        account.plan_type = plan_type;
    }

    if let Some(subscription_expires_at) = subscription_expires_at {
        account.subscription_expires_at = subscription_expires_at;
    }

    let updated = account.clone();
    save_accounts(&store)?;
    Ok(updated)
}

/// Update ChatGPT OAuth tokens for an account and return the updated account.
pub fn update_account_chatgpt_tokens(
    account_id: &str,
    id_token: String,
    access_token: String,
    refresh_token: String,
    chatgpt_account_id: Option<String>,
    email: Option<String>,
    plan_type: Option<String>,
    subscription_expires_at: Option<DateTime<Utc>>,
) -> Result<StoredAccount> {
    let mut store = load_accounts()?;

    let account = store
        .accounts
        .iter_mut()
        .find(|a| a.id == account_id)
        .context("Account not found")?;

    match &mut account.auth_data {
        AuthData::ChatGPT {
            id_token: stored_id_token,
            access_token: stored_access_token,
            refresh_token: stored_refresh_token,
            account_id: stored_account_id,
        } => {
            *stored_id_token = id_token;
            *stored_access_token = access_token;
            *stored_refresh_token = refresh_token;
            if let Some(new_account_id) = chatgpt_account_id {
                *stored_account_id = Some(new_account_id);
            }
        }
        AuthData::ApiKey { .. } => {
            anyhow::bail!("Cannot update OAuth tokens for an API key account");
        }
    }

    if let Some(new_email) = email {
        account.email = Some(new_email);
    }

    if let Some(new_plan_type) = plan_type {
        account.plan_type = Some(new_plan_type);
    }

    if let Some(subscription_expires_at) = subscription_expires_at {
        account.subscription_expires_at = Some(subscription_expires_at);
    }

    let updated = account.clone();
    save_accounts(&store)?;
    Ok(updated)
}

/// Get the list of masked account IDs
pub fn get_masked_account_ids() -> Result<Vec<String>> {
    let store = load_accounts()?;
    Ok(store.masked_account_ids.clone())
}

/// Set the list of masked account IDs
pub fn set_masked_account_ids(ids: Vec<String>) -> Result<()> {
    let mut store = load_accounts()?;
    store.masked_account_ids = ids;
    save_accounts(&store)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{decrypt_accounts_store_json, encrypt_accounts_store_json};

    #[test]
    fn encrypted_accounts_store_payload_does_not_contain_plaintext_credentials() {
        let key = [7u8; 32];
        let json = br#"{"accounts":[{"auth_data":{"type":"api_key","key":"sk-test-secret"}}]}"#;

        let encrypted = encrypt_accounts_store_json(json, &key).expect("encrypt store");

        assert_ne!(encrypted, json);
        assert!(!String::from_utf8_lossy(&encrypted).contains("sk-test-secret"));
        assert_eq!(
            decrypt_accounts_store_json(&encrypted, &key).expect("decrypt store"),
            json
        );
    }
}
