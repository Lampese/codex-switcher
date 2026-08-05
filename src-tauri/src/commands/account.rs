//! Account management Tauri commands

use crate::auth::account_repository::{
    AccountMetadataPatch, AccountRepository, SecureAccountInsert,
};
use crate::auth::metadata_store::{AccountMetadataV2, MetadataAuthKind};
use crate::auth::paths::AppPaths;
use crate::auth::vault::SecretRecord;
use crate::auth::{
    create_chatgpt_account_from_refresh_token, import_from_auth_json_contents, load_accounts,
    save_accounts, set_active_account, switch_to_account, touch_account,
};
use crate::types::{AccountInfo, AccountsStore, AuthData, ImportAccountsSummary, StoredAccount};

use super::process::ensure_codex_not_running;

use anyhow::Context;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    XChaCha20Poly1305, XNonce,
};
use flate2::{read::ZlibDecoder, write::ZlibEncoder, Compression};
use futures::{stream, StreamExt};
use pbkdf2::pbkdf2_hmac;
use rand::RngCore;
use sha2::Sha256;
use std::collections::HashSet;
use std::fs;
use std::io::{Read, Write};
use zeroize::{Zeroize, Zeroizing};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

const SLIM_EXPORT_PREFIX: &str = "css1.";
const SLIM_FORMAT_VERSION: u8 = 1;
const SLIM_AUTH_API_KEY: u8 = 0;
const SLIM_AUTH_CHATGPT: u8 = 1;

const FULL_FILE_MAGIC: &[u8; 4] = b"CSWF";
const FULL_FILE_VERSION: u8 = 1;
const FULL_SALT_LEN: usize = 16;
const FULL_NONCE_LEN: usize = 24;
const FULL_KDF_ITERATIONS: u32 = 210_000;
const FULL_PRESET_PASSPHRASE: &str = "gT7kQ9mV2xN4pL8sR1dH6zW3cB5yF0uJ_aE7nK2tP9vM4rX1";

const MAX_IMPORT_JSON_BYTES: u64 = 2 * 1024 * 1024;
const MAX_IMPORT_FILE_BYTES: u64 = 8 * 1024 * 1024;
const SLIM_IMPORT_CONCURRENCY: usize = 6;

fn production_repository() -> Result<AccountRepository, String> {
    let paths = AppPaths::production()
        .map_err(|_| "Failed to resolve account storage paths".to_string())?;
    Ok(AccountRepository::from_paths(paths))
}

/// Read-only adapter used by the standalone web dispatcher, which does not have Tauri State.
pub async fn list_accounts() -> Result<Vec<AccountInfo>, String> {
    production_repository()?
        .list_accounts()
        .await
        .map_err(|e| e.to_string())
}

/// Read-only adapter used by the standalone web dispatcher, which does not have Tauri State.
pub async fn get_active_account_info() -> Result<Option<AccountInfo>, String> {
    production_repository()?
        .get_active_account()
        .await
        .map_err(|e| e.to_string())
}

/// Read-only adapter used by the standalone web dispatcher, which does not have Tauri State.
pub async fn get_masked_account_ids() -> Result<Vec<String>, String> {
    production_repository()?
        .get_masked_account_ids()
        .await
        .map_err(|e| e.to_string())
}

pub(crate) mod read_only_tauri_commands {
    use super::*;

    #[tauri::command]
    pub(crate) async fn list_accounts(
        repository: tauri::State<'_, AccountRepository>,
    ) -> Result<Vec<AccountInfo>, String> {
        repository.list_accounts().await.map_err(|e| e.to_string())
    }

    #[tauri::command]
    pub(crate) async fn get_active_account_info(
        repository: tauri::State<'_, AccountRepository>,
    ) -> Result<Option<AccountInfo>, String> {
        repository
            .get_active_account()
            .await
            .map_err(|e| e.to_string())
    }

    #[tauri::command]
    pub(crate) async fn get_masked_account_ids(
        repository: tauri::State<'_, AccountRepository>,
    ) -> Result<Vec<String>, String> {
        repository
            .get_masked_account_ids()
            .await
            .map_err(|e| e.to_string())
    }
}

async fn delete_account_with_repository(
    repository: &AccountRepository,
    account_id: String,
) -> Result<(), String> {
    repository
        .remove_account(&account_id)
        .await
        .map_err(|error| error.to_string())
}

async fn rename_account_with_repository(
    repository: &AccountRepository,
    account_id: String,
    new_name: String,
) -> Result<(), String> {
    repository
        .update_account_metadata(
            &account_id,
            AccountMetadataPatch {
                display_name: Some(new_name),
                ..Default::default()
            },
        )
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
}

async fn set_masked_account_ids_with_repository(
    repository: &AccountRepository,
    ids: Vec<String>,
) -> Result<(), String> {
    repository
        .set_masked_account_ids(ids)
        .await
        .map_err(|error| error.to_string())
}

pub(crate) mod secure_mutation_tauri_commands {
    use super::*;

    #[tauri::command]
    pub(crate) async fn add_account_from_file(
        repository: tauri::State<'_, AccountRepository>,
        path: String,
        name: String,
    ) -> Result<AccountInfo, String> {
        add_account_from_file_with_repository(&repository, path, name).await
    }

    #[tauri::command]
    pub(crate) async fn delete_account(
        repository: tauri::State<'_, AccountRepository>,
        account_id: String,
    ) -> Result<(), String> {
        delete_account_with_repository(&repository, account_id).await
    }

    #[tauri::command]
    pub(crate) async fn rename_account(
        repository: tauri::State<'_, AccountRepository>,
        account_id: String,
        new_name: String,
    ) -> Result<(), String> {
        rename_account_with_repository(&repository, account_id, new_name).await
    }

    #[tauri::command]
    pub(crate) async fn set_masked_account_ids(
        repository: tauri::State<'_, AccountRepository>,
        ids: Vec<String>,
    ) -> Result<(), String> {
        set_masked_account_ids_with_repository(&repository, ids).await
    }
}

const AUTH_FILE_OPEN_ERROR: &str = "Unable to open auth.json file";
const AUTH_FILE_READ_ERROR: &str = "Unable to read auth.json file";
const AUTH_FILE_TOO_LARGE_ERROR: &str = "auth.json file is too large";
const AUTH_JSON_UTF8_ERROR: &str = "auth.json is not valid UTF-8";
const AUTH_JSON_INVALID_ERROR: &str = "Invalid auth.json contents";
const AUTH_JSON_NO_CREDENTIALS_ERROR: &str = "auth.json contains no supported credentials";
const AUTH_DATA_MISMATCH_ERROR: &str = "Authentication data does not match auth mode";

struct ImportedStoredAccountGuard {
    account: Option<StoredAccount>,
}

impl ImportedStoredAccountGuard {
    fn new(account: StoredAccount) -> Self {
        Self {
            account: Some(account),
        }
    }

    fn into_secure_insert(mut self) -> Result<SecureAccountInsert, String> {
        let auth_kind = {
            let account = self
                .account
                .as_ref()
                .ok_or_else(|| AUTH_DATA_MISMATCH_ERROR.to_string())?;
            match (&account.auth_mode, &account.auth_data) {
                (crate::types::AuthMode::ApiKey, AuthData::ApiKey { .. }) => {
                    MetadataAuthKind::ApiKey
                }
                (crate::types::AuthMode::ChatGPT, AuthData::ChatGPT { .. }) => {
                    MetadataAuthKind::ChatGpt
                }
                _ => return Err(AUTH_DATA_MISMATCH_ERROR.to_string()),
            }
        };

        let account = self
            .account
            .take()
            .ok_or_else(|| AUTH_DATA_MISMATCH_ERROR.to_string())?;
        let StoredAccount {
            id,
            name,
            email,
            plan_type,
            subscription_expires_at,
            auth_mode: _,
            auth_data,
            created_at,
            last_used_at,
        } = account;

        let secret = match (auth_kind.clone(), auth_data) {
            (MetadataAuthKind::ApiKey, AuthData::ApiKey { key }) => SecretRecord::ApiKey { key },
            (
                MetadataAuthKind::ChatGpt,
                AuthData::ChatGPT {
                    id_token,
                    access_token,
                    refresh_token,
                    account_id,
                },
            ) => SecretRecord::ChatGpt {
                id_token,
                access_token,
                refresh_token,
                account_id,
            },
            (_, mut unmatched_auth_data) => {
                zeroize_auth_data(&mut unmatched_auth_data);
                return Err(AUTH_DATA_MISMATCH_ERROR.to_string());
            }
        };

        Ok(SecureAccountInsert {
            metadata: AccountMetadataV2 {
                id: id.clone(),
                display_name: name,
                email,
                plan_type,
                subscription_expires_at,
                created_at,
                last_used_at,
                auth_kind,
                vault_ref: id,
            },
            secret,
        })
    }
}

impl Drop for ImportedStoredAccountGuard {
    fn drop(&mut self) {
        if let Some(account) = self.account.as_mut() {
            zeroize_auth_data(&mut account.auth_data);
        }
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

fn secure_insert_from_stored(account: StoredAccount) -> Result<SecureAccountInsert, String> {
    ImportedStoredAccountGuard::new(account).into_secure_insert()
}

fn map_auth_json_import_error(error: anyhow::Error) -> String {
    if error
        .to_string()
        .contains("auth.json contains neither API key nor tokens")
    {
        AUTH_JSON_NO_CREDENTIALS_ERROR.to_string()
    } else {
        AUTH_JSON_INVALID_ERROR.to_string()
    }
}

fn read_auth_json_file_contents(path: &str) -> Result<Zeroizing<Vec<u8>>, String> {
    let file = fs::File::open(path).map_err(|_| AUTH_FILE_OPEN_ERROR.to_string())?;
    let mut raw_bytes = Zeroizing::new(Vec::new());
    file.take(MAX_IMPORT_JSON_BYTES + 1)
        .read_to_end(&mut raw_bytes)
        .map_err(|_| AUTH_FILE_READ_ERROR.to_string())?;

    if raw_bytes.len() > MAX_IMPORT_JSON_BYTES as usize {
        return Err(AUTH_FILE_TOO_LARGE_ERROR.to_string());
    }

    Ok(raw_bytes)
}

pub(crate) async fn add_stored_account_with_repository(
    repository: &AccountRepository,
    account: StoredAccount,
) -> Result<AccountInfo, String> {
    let insert = secure_insert_from_stored(account)?;
    repository
        .add_account(insert)
        .await
        .map_err(|error| error.to_string())
}

async fn add_account_from_file_with_repository(
    repository: &AccountRepository,
    path: String,
    name: String,
) -> Result<AccountInfo, String> {
    let raw_bytes = read_auth_json_file_contents(&path)?;
    let contents =
        std::str::from_utf8(raw_bytes.as_slice()).map_err(|_| AUTH_JSON_UTF8_ERROR.to_string())?;
    let account =
        import_from_auth_json_contents(contents, name).map_err(map_auth_json_import_error)?;
    add_stored_account_with_repository(repository, account).await
}

async fn add_account_from_auth_json_text_with_repository(
    repository: &AccountRepository,
    name: String,
    contents: String,
) -> Result<AccountInfo, String> {
    let contents = Zeroizing::new(contents);
    if contents.len() > MAX_IMPORT_JSON_BYTES as usize {
        return Err(AUTH_FILE_TOO_LARGE_ERROR.to_string());
    }

    let account = import_from_auth_json_contents(contents.as_str(), name)
        .map_err(map_auth_json_import_error)?;
    add_stored_account_with_repository(repository, account).await
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct SlimPayload {
    #[serde(rename = "v")]
    version: u8,
    #[serde(rename = "a", skip_serializing_if = "Option::is_none")]
    active_name: Option<String>,
    #[serde(rename = "c")]
    accounts: Vec<SlimAccountPayload>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct SlimAccountPayload {
    #[serde(rename = "n")]
    name: String,
    #[serde(rename = "t")]
    auth_type: u8,
    #[serde(rename = "k", skip_serializing_if = "Option::is_none")]
    api_key: Option<String>,
    #[serde(rename = "r", skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
}

/// Standalone adapter used by the web dispatcher, which does not have Tauri State.
pub async fn add_account_from_file(path: String, name: String) -> Result<AccountInfo, String> {
    let repository = production_repository()?;
    add_account_from_file_with_repository(&repository, path, name).await
}

/// Standalone adapter used by the web dispatcher, which does not have Tauri State.
pub async fn add_account_from_auth_json_text(
    name: String,
    contents: String,
) -> Result<AccountInfo, String> {
    let repository = production_repository()?;
    add_account_from_auth_json_text_with_repository(&repository, name, contents).await
}

/// Switch to a different account
#[tauri::command]
pub async fn switch_account(account_id: String) -> Result<(), String> {
    switch_account_by_id(&account_id)
}

pub fn switch_account_by_id(account_id: &str) -> Result<(), String> {
    let store = load_accounts().map_err(|e| e.to_string())?;

    // Find the account
    let account = store
        .accounts
        .iter()
        .find(|a| a.id == account_id)
        .ok_or_else(|| format!("Account not found: {account_id}"))?;

    ensure_codex_not_running()?;

    // Write to ~/.codex/auth.json
    switch_to_account(account).map_err(|e| e.to_string())?;

    // Update the active account in our store
    set_active_account(account_id).map_err(|e| e.to_string())?;

    // Update last_used_at
    touch_account(account_id).map_err(|e| e.to_string())?;

    // Restart Antigravity background process if it is running
    // This allows it to pick up the new authorization file seamlessly
    if let Ok(pids) = find_antigravity_processes() {
        for pid in pids {
            #[cfg(unix)]
            {
                let _ = std::process::Command::new("kill")
                    .arg("-9")
                    .arg(pid.to_string())
                    .output();
            }
            #[cfg(windows)]
            {
                let _ = std::process::Command::new("taskkill")
                    .args(["/F", "/PID", &pid.to_string()])
                    .output();
            }
        }
    }

    Ok(())
}

/// Standalone adapter used by the web dispatcher, which does not have Tauri State.
pub async fn delete_account(account_id: String) -> Result<(), String> {
    let repository = production_repository()?;
    delete_account_with_repository(&repository, account_id).await
}

/// Standalone adapter used by the web dispatcher, which does not have Tauri State.
pub async fn rename_account(account_id: String, new_name: String) -> Result<(), String> {
    let repository = production_repository()?;
    rename_account_with_repository(&repository, account_id, new_name).await
}

/// Export minimal account config as a compact text string.
/// For ChatGPT accounts, only refresh token is exported.
#[tauri::command]
pub async fn export_accounts_slim_text() -> Result<String, String> {
    let store = load_accounts().map_err(|e| e.to_string())?;
    encode_slim_payload_from_store(&store).map_err(|e| e.to_string())
}

/// Import minimal account config from a compact text string, skipping existing accounts.
#[tauri::command]
pub async fn import_accounts_slim_text(payload: String) -> Result<ImportAccountsSummary, String> {
    let slim_payload = decode_slim_payload(&payload).map_err(|e| format!("{e:#}"))?;
    let total_in_payload = slim_payload.accounts.len();

    let current = load_accounts().map_err(|e| e.to_string())?;
    let existing_names: HashSet<String> = current.accounts.iter().map(|a| a.name.clone()).collect();

    let imported = build_store_from_slim_payload(slim_payload, &existing_names)
        .await
        .map_err(|e| {
            format!(
                "{e:#}\nHint: Slim import needs network access to refresh ChatGPT tokens. You can use Full encrypted file import when offline."
            )
        })?;
    validate_imported_store(&imported).map_err(|e| format!("{e:#}"))?;

    let (merged, summary) = merge_accounts_store(current, imported);
    save_accounts(&merged).map_err(|e| e.to_string())?;
    Ok(ImportAccountsSummary {
        total_in_payload,
        imported_count: summary.imported_count,
        skipped_count: total_in_payload.saturating_sub(summary.imported_count),
    })
}

/// Export full account config as an encrypted file.
#[tauri::command]
pub async fn export_accounts_full_encrypted_file(path: String) -> Result<(), String> {
    let store = load_accounts().map_err(|e| e.to_string())?;
    let encrypted =
        encode_full_encrypted_store(&store, FULL_PRESET_PASSPHRASE).map_err(|e| e.to_string())?;
    write_encrypted_file(&path, &encrypted).map_err(|e| e.to_string())?;
    Ok(())
}

/// Export full account config as encrypted bytes for browser clients.
pub async fn export_accounts_full_encrypted_bytes() -> Result<Vec<u8>, String> {
    let store = load_accounts().map_err(|e| e.to_string())?;
    encode_full_encrypted_store(&store, FULL_PRESET_PASSPHRASE).map_err(|e| e.to_string())
}

/// Import full account config from an encrypted file, skipping existing accounts.
#[tauri::command]
pub async fn import_accounts_full_encrypted_file(
    path: String,
) -> Result<ImportAccountsSummary, String> {
    let encrypted = read_encrypted_file(&path).map_err(|e| e.to_string())?;
    let imported = decode_full_encrypted_store(&encrypted, FULL_PRESET_PASSPHRASE)
        .map_err(|e| e.to_string())?;
    validate_imported_store(&imported).map_err(|e| e.to_string())?;

    let current = load_accounts().map_err(|e| e.to_string())?;
    let (merged, summary) = merge_accounts_store(current, imported);
    save_accounts(&merged).map_err(|e| e.to_string())?;
    Ok(summary)
}

/// Import full account config from encrypted bytes uploaded through the browser UI.
pub async fn import_accounts_full_encrypted_bytes(
    bytes: Vec<u8>,
) -> Result<ImportAccountsSummary, String> {
    let imported =
        decode_full_encrypted_store(&bytes, FULL_PRESET_PASSPHRASE).map_err(|e| e.to_string())?;
    validate_imported_store(&imported).map_err(|e| e.to_string())?;

    let current = load_accounts().map_err(|e| e.to_string())?;
    let (merged, summary) = merge_accounts_store(current, imported);
    save_accounts(&merged).map_err(|e| e.to_string())?;
    Ok(summary)
}

/// Find all running Antigravity codex assistant processes
fn find_antigravity_processes() -> anyhow::Result<Vec<u32>> {
    let mut pids = Vec::new();

    #[cfg(unix)]
    {
        // Use ps with custom format to get the pid and full command line
        let output = std::process::Command::new("ps")
            .args(["-eo", "pid,command"])
            .output()?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines().skip(1) {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            if let Some((pid_str, command)) = line.split_once(' ') {
                let pid_str = pid_str.trim();
                let command = command.trim();

                // Antigravity processes have a specific path format
                let is_antigravity = (command.contains(".antigravity/extensions/openai.chatgpt")
                    || command.contains(".vscode/extensions/openai.chatgpt"))
                    && (command.ends_with("codex app-server --analytics-default-enabled")
                        || command.contains("/codex app-server"));

                if is_antigravity {
                    if let Ok(pid) = pid_str.parse::<u32>() {
                        pids.push(pid);
                    }
                }
            }
        }
    }

    #[cfg(windows)]
    {
        // Use tasklist on Windows
        // For Windows we might need a more precise WMI query to get command line args,
        // but for now we look for codex.exe PIDs and verify they're not ours
        let output = std::process::Command::new("tasklist")
            .creation_flags(CREATE_NO_WINDOW)
            .args(["/FI", "IMAGENAME eq codex.exe", "/FO", "CSV", "/NH"])
            .output()?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            let parts: Vec<&str> = line.split(',').collect();
            if parts.len() > 1 {
                let name = parts[0].trim_matches('"').to_lowercase();
                if name == "codex.exe" {
                    let pid_str = parts[1].trim_matches('"');
                    if let Ok(pid) = pid_str.parse::<u32>() {
                        pids.push(pid);
                    }
                }
            }
        }
    }

    Ok(pids)
}

fn encode_slim_payload_from_store(store: &AccountsStore) -> anyhow::Result<String> {
    let active_name = store.active_account_id.as_ref().and_then(|active_id| {
        store
            .accounts
            .iter()
            .find(|account| account.id == *active_id)
            .map(|account| account.name.clone())
    });

    let slim_accounts = store
        .accounts
        .iter()
        .map(|account| match &account.auth_data {
            AuthData::ApiKey { key } => SlimAccountPayload {
                name: account.name.clone(),
                auth_type: SLIM_AUTH_API_KEY,
                api_key: Some(key.clone()),
                refresh_token: None,
            },
            AuthData::ChatGPT { refresh_token, .. } => SlimAccountPayload {
                name: account.name.clone(),
                auth_type: SLIM_AUTH_CHATGPT,
                api_key: None,
                refresh_token: Some(refresh_token.clone()),
            },
        })
        .collect();

    let payload = SlimPayload {
        version: SLIM_FORMAT_VERSION,
        active_name,
        accounts: slim_accounts,
    };

    let json = serde_json::to_vec(&payload).context("Failed to serialize slim payload")?;
    let compressed = compress_bytes(&json).context("Failed to compress slim payload")?;

    Ok(format!(
        "{SLIM_EXPORT_PREFIX}{}",
        URL_SAFE_NO_PAD.encode(compressed)
    ))
}

fn decode_slim_payload(payload: &str) -> anyhow::Result<SlimPayload> {
    let normalized: String = payload.chars().filter(|c| !c.is_whitespace()).collect();
    if normalized.is_empty() {
        anyhow::bail!("Import string is empty");
    }

    let encoded = normalized
        .strip_prefix(SLIM_EXPORT_PREFIX)
        .unwrap_or(&normalized);

    let compressed = URL_SAFE_NO_PAD
        .decode(encoded)
        .context("Invalid slim import string (base64 decode failed)")?;

    let decompressed = decompress_bytes_with_limit(&compressed, MAX_IMPORT_JSON_BYTES)
        .context("Invalid slim import string (decompression failed)")?;

    let parsed: SlimPayload = serde_json::from_slice(&decompressed)
        .context("Invalid slim import string (JSON parse failed)")?;

    validate_slim_payload(&parsed)?;
    Ok(parsed)
}

fn validate_slim_payload(payload: &SlimPayload) -> anyhow::Result<()> {
    if payload.version != SLIM_FORMAT_VERSION {
        anyhow::bail!("Unsupported slim payload version: {}", payload.version);
    }

    let mut names = HashSet::new();

    for account in &payload.accounts {
        if account.name.trim().is_empty() {
            anyhow::bail!("Slim import contains an account with empty name");
        }

        if !names.insert(account.name.clone()) {
            anyhow::bail!(
                "Slim import contains duplicate account name: {}",
                account.name
            );
        }

        match account.auth_type {
            SLIM_AUTH_API_KEY => {
                if account
                    .api_key
                    .as_ref()
                    .map_or(true, |key| key.trim().is_empty())
                {
                    anyhow::bail!("API key is missing for account {}", account.name);
                }
            }
            SLIM_AUTH_CHATGPT => {
                if account
                    .refresh_token
                    .as_ref()
                    .map_or(true, |token| token.trim().is_empty())
                {
                    anyhow::bail!("Refresh token is missing for account {}", account.name);
                }
            }
            _ => {
                anyhow::bail!(
                    "Unsupported auth type {} for account {}",
                    account.auth_type,
                    account.name
                );
            }
        }
    }

    if let Some(active_name) = &payload.active_name {
        if !names.contains(active_name) {
            anyhow::bail!("Slim import references missing active account: {active_name}");
        }
    }

    Ok(())
}

async fn build_store_from_slim_payload(
    payload: SlimPayload,
    existing_names: &HashSet<String>,
) -> anyhow::Result<AccountsStore> {
    let active_name = payload.active_name;
    let import_candidates: Vec<SlimAccountPayload> = payload
        .accounts
        .into_iter()
        .filter(|entry| !existing_names.contains(&entry.name))
        .collect();

    let accounts = restore_slim_accounts(import_candidates).await?;
    let mut active_account_id = None;

    if let Some(active) = active_name {
        active_account_id = accounts
            .iter()
            .find(|account| account.name == active)
            .map(|account| account.id.clone());
    }

    if active_account_id.is_none() {
        active_account_id = accounts.first().map(|a| a.id.clone());
    }

    Ok(AccountsStore {
        version: 1,
        accounts,
        active_account_id,
        masked_account_ids: Vec::new(),
    })
}

async fn restore_slim_accounts(
    entries: Vec<SlimAccountPayload>,
) -> anyhow::Result<Vec<StoredAccount>> {
    if entries.is_empty() {
        return Ok(Vec::new());
    }

    let mut restored = Vec::with_capacity(entries.len());
    let mut tasks = stream::iter(entries.into_iter().map(|entry| async move {
        let account_name = entry.name;
        let account = match entry.auth_type {
            SLIM_AUTH_API_KEY => StoredAccount::new_api_key(
                account_name.clone(),
                entry.api_key.context("API key payload is missing")?,
            ),
            SLIM_AUTH_CHATGPT => {
                let refresh_token = entry
                    .refresh_token
                    .context("Refresh token payload is missing")?;
                create_chatgpt_account_from_refresh_token(account_name.clone(), refresh_token)
                    .await
                    .with_context(|| {
                        format!(
                            "Failed to restore ChatGPT account `{account_name}` from refresh token"
                        )
                    })?
            }
            _ => anyhow::bail!("Unsupported auth type in slim payload"),
        };
        Ok::<StoredAccount, anyhow::Error>(account)
    }))
    .buffered(SLIM_IMPORT_CONCURRENCY);

    while let Some(account_result) = tasks.next().await {
        restored.push(account_result?);
    }

    Ok(restored)
}

fn encode_full_encrypted_store(store: &AccountsStore, passphrase: &str) -> anyhow::Result<Vec<u8>> {
    let json = serde_json::to_vec(store).context("Failed to serialize account store")?;
    let compressed = compress_bytes(&json).context("Failed to compress account store")?;

    let mut salt = [0u8; FULL_SALT_LEN];
    rand::rng().fill_bytes(&mut salt);

    let mut nonce = [0u8; FULL_NONCE_LEN];
    rand::rng().fill_bytes(&mut nonce);

    let key = derive_encryption_key(passphrase, &salt);
    let cipher = XChaCha20Poly1305::new((&key).into());
    let ciphertext = cipher
        .encrypt(XNonce::from_slice(&nonce), compressed.as_slice())
        .map_err(|_| anyhow::anyhow!("Failed to encrypt account store"))?;

    let mut out = Vec::with_capacity(4 + 1 + FULL_SALT_LEN + FULL_NONCE_LEN + ciphertext.len());
    out.extend_from_slice(FULL_FILE_MAGIC);
    out.push(FULL_FILE_VERSION);
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);

    Ok(out)
}

fn decode_full_encrypted_store(
    file_bytes: &[u8],
    passphrase: &str,
) -> anyhow::Result<AccountsStore> {
    if file_bytes.len() as u64 > MAX_IMPORT_FILE_BYTES {
        anyhow::bail!("Encrypted file is too large");
    }

    let header_len = 4 + 1 + FULL_SALT_LEN + FULL_NONCE_LEN;
    if file_bytes.len() <= header_len {
        anyhow::bail!("Encrypted file is invalid or truncated");
    }

    if &file_bytes[..4] != FULL_FILE_MAGIC {
        anyhow::bail!("Encrypted file header is invalid");
    }

    let version = file_bytes[4];
    if version != FULL_FILE_VERSION {
        anyhow::bail!("Unsupported encrypted file version: {version}");
    }

    let salt_start = 5;
    let nonce_start = salt_start + FULL_SALT_LEN;
    let ciphertext_start = nonce_start + FULL_NONCE_LEN;

    let salt = &file_bytes[salt_start..nonce_start];
    let nonce = &file_bytes[nonce_start..ciphertext_start];
    let ciphertext = &file_bytes[ciphertext_start..];

    let key = derive_encryption_key(passphrase, salt);
    let cipher = XChaCha20Poly1305::new((&key).into());
    let compressed = cipher
        .decrypt(XNonce::from_slice(nonce), ciphertext)
        .map_err(|_| {
            anyhow::anyhow!("Failed to decrypt file (wrong passphrase or corrupted file)")
        })?;

    let json = decompress_bytes_with_limit(&compressed, MAX_IMPORT_JSON_BYTES)
        .context("Failed to decompress decrypted payload")?;

    let store: AccountsStore =
        serde_json::from_slice(&json).context("Failed to parse decrypted account payload")?;

    Ok(store)
}

fn derive_encryption_key(passphrase: &str, salt: &[u8]) -> [u8; 32] {
    let mut key = [0u8; 32];
    pbkdf2_hmac::<Sha256>(passphrase.as_bytes(), salt, FULL_KDF_ITERATIONS, &mut key);
    key
}

fn compress_bytes(input: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
    encoder.write_all(input)?;
    encoder.finish().context("Failed to finalize compression")
}

fn decompress_bytes_with_limit(input: &[u8], max_bytes: u64) -> anyhow::Result<Vec<u8>> {
    let mut decoder = ZlibDecoder::new(input);
    let mut limited = decoder.by_ref().take(max_bytes + 1);
    let mut decompressed = Vec::new();
    limited.read_to_end(&mut decompressed)?;

    if decompressed.len() as u64 > max_bytes {
        anyhow::bail!("Import data is too large");
    }

    Ok(decompressed)
}

fn write_encrypted_file(path: &str, bytes: &[u8]) -> anyhow::Result<()> {
    fs::write(path, bytes).with_context(|| format!("Failed to write file: {path}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .with_context(|| format!("Failed to set file permissions: {path}"))?;
    }

    Ok(())
}

fn read_encrypted_file(path: &str) -> anyhow::Result<Vec<u8>> {
    let metadata =
        fs::metadata(path).with_context(|| format!("Failed to read file metadata: {path}"))?;
    if metadata.len() > MAX_IMPORT_FILE_BYTES {
        anyhow::bail!("Encrypted file is too large");
    }

    fs::read(path).with_context(|| format!("Failed to read file: {path}"))
}

fn validate_imported_store(store: &AccountsStore) -> anyhow::Result<()> {
    let mut ids = HashSet::new();
    let mut names = HashSet::new();

    for account in &store.accounts {
        if account.id.trim().is_empty() {
            anyhow::bail!("Import contains an account with empty id");
        }
        if account.name.trim().is_empty() {
            anyhow::bail!("Import contains an account with empty name");
        }
        if !ids.insert(account.id.clone()) {
            anyhow::bail!("Import contains duplicate account id: {}", account.id);
        }
        if !names.insert(account.name.clone()) {
            anyhow::bail!("Import contains duplicate account name: {}", account.name);
        }
    }

    if let Some(active_id) = &store.active_account_id {
        if !ids.contains(active_id) {
            anyhow::bail!("Import references a missing active account: {active_id}");
        }
    }

    Ok(())
}

fn merge_accounts_store(
    mut current: AccountsStore,
    imported: AccountsStore,
) -> (AccountsStore, ImportAccountsSummary) {
    let imported_version = imported.version;
    let imported_active_id = imported.active_account_id;
    let total_in_payload = imported.accounts.len();
    let mut imported_count = 0usize;
    let mut existing_ids: HashSet<String> = current.accounts.iter().map(|a| a.id.clone()).collect();
    let mut existing_names: HashSet<String> =
        current.accounts.iter().map(|a| a.name.clone()).collect();

    for account in imported.accounts {
        if existing_ids.contains(&account.id) || existing_names.contains(&account.name) {
            continue;
        }
        existing_ids.insert(account.id.clone());
        existing_names.insert(account.name.clone());
        current.accounts.push(account);
        imported_count += 1;
    }

    current.version = current.version.max(imported_version).max(1);

    let current_active_is_valid = current
        .active_account_id
        .as_ref()
        .is_some_and(|id| current.accounts.iter().any(|a| &a.id == id));

    if !current_active_is_valid {
        if let Some(imported_active) = imported_active_id {
            if current.accounts.iter().any(|a| a.id == imported_active) {
                current.active_account_id = Some(imported_active);
            } else {
                current.active_account_id = current.accounts.first().map(|a| a.id.clone());
            }
        } else {
            current.active_account_id = current.accounts.first().map(|a| a.id.clone());
        }
    }

    (
        current,
        ImportAccountsSummary {
            total_in_payload,
            imported_count,
            skipped_count: total_in_payload.saturating_sub(imported_count),
        },
    )
}

/// Standalone adapter used by the web dispatcher, which does not have Tauri State.
pub async fn set_masked_account_ids(ids: Vec<String>) -> Result<(), String> {
    let repository = production_repository()?;
    set_masked_account_ids_with_repository(&repository, ids).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::account_repository::SecureAccountInsert;
    use crate::auth::metadata_store::{
        AccountMetadataV2, MetadataAuthKind, MetadataFileStore, MetadataStoreV2,
    };
    use crate::auth::paths::AppPaths;
    use crate::auth::secure_commit::SecureCommitTestOptions;
    use crate::auth::vault::{SecretRecord, VaultPayloadV1, VaultStore};
    use crate::types::{AccountsStore, AuthData, AuthMode, StoredAccount};
    use chrono::{DateTime, TimeZone, Utc};
    use std::path::{Path, PathBuf};

    const ID_TOKEN_A: &str = "synthetic-id-token-A";
    const ACCESS_TOKEN_A: &str = "synthetic-access-token-A";
    const REFRESH_TOKEN_A: &str = "synthetic-refresh-token-A";
    const API_KEY_A: &str = "synthetic-api-key-A";
    const CHATGPT_ACCOUNT_A: &str = "synthetic-chatgpt-account-A";
    const ACCOUNT_ID_A: &str = "command-account-A";
    const ACCOUNT_ID_B: &str = "command-account-B";
    const DISPLAY_NAME_A: &str = "Command ChatGPT";
    const DISPLAY_NAME_B: &str = "Command API";
    const EMAIL_A: &str = "command-account-A@example.test";
    const EMAIL_B: &str = "command-account-B@example.test";

    fn test_paths(label: &str) -> (PathBuf, AppPaths) {
        let root = std::env::temp_dir().join(format!(
            "codex_command_routing_test_{}_{}",
            label,
            rand::random::<u32>()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let paths = AppPaths::for_test(&root);
        (root, paths)
    }

    fn cleanup(root: PathBuf) {
        let _ = std::fs::remove_dir_all(root);
    }

    fn ensure_switcher_dir(paths: &AppPaths) {
        std::fs::create_dir_all(&paths.switcher_dir).unwrap();
    }

    fn timestamp(hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 5, hour, 0, 0)
            .single()
            .unwrap()
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
            subscription_expires_at: Some(timestamp(20)),
            created_at: timestamp(hour),
            last_used_at: Some(timestamp(hour + 1)),
            auth_kind,
            vault_ref: id.to_string(),
        }
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

    async fn seeded_repository(paths: &AppPaths) -> AccountRepository {
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
    }

    fn read_pair(paths: &AppPaths) -> (Vec<u8>, Vec<u8>) {
        (
            std::fs::read(&paths.metadata_file).unwrap(),
            std::fs::read(&paths.vault_file).unwrap(),
        )
    }

    fn load_metadata(paths: &AppPaths) -> MetadataStoreV2 {
        MetadataFileStore::from_paths(paths).load().unwrap()
    }

    fn load_vault(paths: &AppPaths) -> VaultPayloadV1 {
        VaultStore::from_paths(paths).load().unwrap()
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

    fn legacy_store() -> AccountsStore {
        AccountsStore {
            version: 1,
            accounts: vec![StoredAccount {
                id: ACCOUNT_ID_A.to_string(),
                name: DISPLAY_NAME_A.to_string(),
                email: Some(EMAIL_A.to_string()),
                plan_type: Some("pro".to_string()),
                subscription_expires_at: Some(timestamp(20)),
                auth_mode: AuthMode::ChatGPT,
                auth_data: AuthData::ChatGPT {
                    id_token: ID_TOKEN_A.to_string(),
                    access_token: ACCESS_TOKEN_A.to_string(),
                    refresh_token: REFRESH_TOKEN_A.to_string(),
                    account_id: Some(CHATGPT_ACCOUNT_A.to_string()),
                },
                created_at: timestamp(1),
                last_used_at: Some(timestamp(2)),
            }],
            active_account_id: Some(ACCOUNT_ID_A.to_string()),
            masked_account_ids: vec![ACCOUNT_ID_A.to_string()],
        }
    }

    fn write_legacy(paths: &AppPaths) -> Vec<u8> {
        ensure_switcher_dir(paths);
        let bytes = serde_json::to_vec(&legacy_store()).unwrap();
        std::fs::write(&paths.metadata_file, &bytes).unwrap();
        bytes
    }

    fn write_orphan_vault(paths: &AppPaths) -> Vec<u8> {
        let bytes = b"opaque legacy orphan vault bytes".to_vec();
        std::fs::write(&paths.vault_file, &bytes).unwrap();
        bytes
    }

    fn assert_sanitized(message: &str, root: &Path) {
        for value in [
            ID_TOKEN_A,
            ACCESS_TOKEN_A,
            REFRESH_TOKEN_A,
            API_KEY_A,
            CHATGPT_ACCOUNT_A,
            ACCOUNT_ID_A,
            ACCOUNT_ID_B,
            DISPLAY_NAME_A,
            DISPLAY_NAME_B,
            EMAIL_A,
            EMAIL_B,
        ] {
            assert!(
                !message.contains(value),
                "error leaked sensitive value: {message}"
            );
        }
        let root_text = root.to_string_lossy();
        assert!(
            !message.contains(root_text.as_ref()),
            "error leaked filesystem path: {message}"
        );
    }

    fn valid_chatgpt_id_token(email: Option<&str>, account_id: &str) -> String {
        let payload = serde_json::json!({
            "email": email,
            "https://api.openai.com/auth": {
                "chatgpt_plan_type": "plus",
                "chatgpt_account_id": account_id,
                "chatgpt_subscription_active_until": "2026-08-05T20:00:00Z"
            }
        });
        let encoded_payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
        format!("header.{encoded_payload}.signature")
    }

    fn api_key_auth_json(key: &str) -> String {
        serde_json::json!({"OPENAI_API_KEY": key}).to_string()
    }

    fn chatgpt_auth_json(email: Option<&str>, account_id: &str) -> String {
        serde_json::json!({
            "tokens": {
                "id_token": valid_chatgpt_id_token(email, account_id),
                "access_token": ACCESS_TOKEN_A,
                "refresh_token": REFRESH_TOKEN_A,
                "account_id": account_id
            }
        })
        .to_string()
    }

    fn write_auth_file(root: &Path, contents: &str, label: &str) -> PathBuf {
        let path = root.join(format!("{label}.json"));
        std::fs::write(&path, contents).unwrap();
        path
    }

    fn stored_api_key(id: &str, name: &str, key: &str) -> StoredAccount {
        StoredAccount {
            id: id.to_string(),
            name: name.to_string(),
            email: Some("mapped@example.test".to_string()),
            plan_type: Some("team".to_string()),
            subscription_expires_at: Some(timestamp(20)),
            auth_mode: AuthMode::ApiKey,
            auth_data: AuthData::ApiKey {
                key: key.to_string(),
            },
            created_at: timestamp(1),
            last_used_at: Some(timestamp(2)),
        }
    }

    fn stored_chatgpt(id: &str, name: &str) -> StoredAccount {
        StoredAccount {
            id: id.to_string(),
            name: name.to_string(),
            email: Some("mapped-chatgpt@example.test".to_string()),
            plan_type: Some("pro".to_string()),
            subscription_expires_at: Some(timestamp(21)),
            auth_mode: AuthMode::ChatGPT,
            auth_data: AuthData::ChatGPT {
                id_token: ID_TOKEN_A.to_string(),
                access_token: ACCESS_TOKEN_A.to_string(),
                refresh_token: REFRESH_TOKEN_A.to_string(),
                account_id: Some(CHATGPT_ACCOUNT_A.to_string()),
            },
            created_at: timestamp(3),
            last_used_at: Some(timestamp(4)),
        }
    }

    #[tokio::test]
    async fn test_delete_adapter_removes_secure_account() {
        let (root, paths) = test_paths("delete_secure");
        let repository = seeded_repository(&paths).await;

        delete_account_with_repository(&repository, ACCOUNT_ID_B.to_string())
            .await
            .unwrap();

        let metadata = load_metadata(&paths);
        let vault = load_vault(&paths);
        assert!(metadata.get(ACCOUNT_ID_B).is_none());
        assert!(!vault.contains(ACCOUNT_ID_B));
        assert_chatgpt_secret(&vault, ACCOUNT_ID_A);

        drop(repository);
        drop(metadata);
        drop(vault);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_deleting_active_secure_account_uses_repository_active_fallback() {
        let (root, paths) = test_paths("delete_active");
        let repository = seeded_repository(&paths).await;

        delete_account_with_repository(&repository, ACCOUNT_ID_A.to_string())
            .await
            .unwrap();

        let metadata = load_metadata(&paths);
        let vault = load_vault(&paths);
        assert_eq!(metadata.active_account_id.as_deref(), Some(ACCOUNT_ID_B));
        assert!(metadata.get(ACCOUNT_ID_A).is_none());
        assert_api_key_secret(&vault, ACCOUNT_ID_B);

        drop(repository);
        drop(metadata);
        drop(vault);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_deleting_unknown_secure_account_returns_sanitized_account_not_found() {
        let (root, paths) = test_paths("delete_unknown");
        let repository = seeded_repository(&paths).await;
        let before = read_pair(&paths);

        let error = delete_account_with_repository(&repository, "unknown-command-id".to_string())
            .await
            .unwrap_err();

        assert_eq!(error, "Account was not found");
        assert_eq!(read_pair(&paths), before);
        assert!(!error.contains("unknown-command-id"));

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_delete_adapter_does_not_invoke_legacy_storage() {
        let (root, paths) = test_paths("delete_no_legacy");
        let metadata_before = write_legacy(&paths);
        let vault_before = write_orphan_vault(&paths);
        let repository = AccountRepository::for_test(paths.clone());

        let error = delete_account_with_repository(&repository, ACCOUNT_ID_A.to_string())
            .await
            .unwrap_err();

        assert_eq!(error, "Legacy account storage requires secure migration");
        assert_eq!(
            std::fs::read(&paths.metadata_file).unwrap(),
            metadata_before
        );
        assert_eq!(std::fs::read(&paths.vault_file).unwrap(), vault_before);

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_delete_adapter_leaves_state_unchanged_on_failure() {
        let (root, paths) = test_paths("delete_commit_failure");
        let repository = seeded_repository(&paths).await;
        drop(repository);
        let before = read_pair(&paths);
        let repository = AccountRepository::for_test_with_commit_options(
            paths.clone(),
            SecureCommitTestOptions {
                fail_metadata_install: true,
                ..Default::default()
            },
        );

        let error = delete_account_with_repository(&repository, ACCOUNT_ID_A.to_string())
            .await
            .unwrap_err();

        assert_eq!(error, "Secure account mutation commit failed");
        assert_eq!(read_pair(&paths), before);

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_rename_adapter_changes_only_display_name() {
        let (root, paths) = test_paths("rename_display_name");
        let repository = seeded_repository(&paths).await;
        let before = load_metadata(&paths);
        let before_account = before.get(ACCOUNT_ID_A).unwrap().clone();
        let before_active = before.active_account_id.clone();
        let before_masked = before.masked_account_ids.clone();
        drop(before);

        rename_account_with_repository(
            &repository,
            ACCOUNT_ID_A.to_string(),
            "Renamed Command Account".to_string(),
        )
        .await
        .unwrap();

        let after = load_metadata(&paths);
        let after_account = after.get(ACCOUNT_ID_A).unwrap();
        assert_eq!(after_account.display_name, "Renamed Command Account");
        assert_eq!(after_account.id, before_account.id);
        assert_eq!(after_account.vault_ref, before_account.vault_ref);
        assert_eq!(after_account.email, before_account.email);
        assert_eq!(after_account.plan_type, before_account.plan_type);
        assert_eq!(
            after_account.subscription_expires_at,
            before_account.subscription_expires_at
        );
        assert_eq!(after_account.auth_kind, before_account.auth_kind);
        assert_eq!(after_account.created_at, before_account.created_at);
        assert_eq!(after_account.last_used_at, before_account.last_used_at);
        assert_eq!(after.active_account_id, before_active);
        assert_eq!(after.masked_account_ids, before_masked);

        drop(repository);
        drop(before_account);
        drop(after);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_rename_preserves_metadata_and_secret() {
        let (root, paths) = test_paths("rename_preserves_state");
        let repository = seeded_repository(&paths).await;
        let before = load_metadata(&paths);
        let before_account = before.get(ACCOUNT_ID_A).unwrap().clone();
        let before_vault = load_vault(&paths);
        drop(before);

        rename_account_with_repository(
            &repository,
            ACCOUNT_ID_A.to_string(),
            "Renamed With Secret".to_string(),
        )
        .await
        .unwrap();

        let after = load_metadata(&paths);
        let after_vault = load_vault(&paths);
        let after_account = after.get(ACCOUNT_ID_A).unwrap();
        assert_eq!(after_account.email, before_account.email);
        assert_eq!(after_account.plan_type, before_account.plan_type);
        assert_eq!(
            after_account.subscription_expires_at,
            before_account.subscription_expires_at
        );
        assert_eq!(after_account.created_at, before_account.created_at);
        assert_eq!(after_account.last_used_at, before_account.last_used_at);
        assert_eq!(after_account.auth_kind, before_account.auth_kind);
        assert_chatgpt_secret(&before_vault, ACCOUNT_ID_A);
        assert_api_key_secret(&before_vault, ACCOUNT_ID_B);
        assert_chatgpt_secret(&after_vault, ACCOUNT_ID_A);
        assert_api_key_secret(&after_vault, ACCOUNT_ID_B);

        drop(repository);
        drop(before_account);
        drop(before_vault);
        drop(after);
        drop(after_vault);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_duplicate_rename_returns_sanitized_duplicate_display_name() {
        let (root, paths) = test_paths("rename_duplicate");
        let repository = seeded_repository(&paths).await;
        let before = read_pair(&paths);

        let error = rename_account_with_repository(
            &repository,
            ACCOUNT_ID_A.to_string(),
            DISPLAY_NAME_B.to_string(),
        )
        .await
        .unwrap_err();

        assert_eq!(error, "Duplicate display name");
        assert_eq!(read_pair(&paths), before);
        assert!(!error.contains(DISPLAY_NAME_B));

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_blank_rename_returns_sanitized_invalid_account_data() {
        let (root, paths) = test_paths("rename_blank");
        let repository = seeded_repository(&paths).await;
        let before = read_pair(&paths);

        let error = rename_account_with_repository(
            &repository,
            ACCOUNT_ID_A.to_string(),
            "   ".to_string(),
        )
        .await
        .unwrap_err();

        assert_eq!(error, "Invalid account data");
        assert_eq!(read_pair(&paths), before);

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_identical_rename_is_a_byte_for_byte_no_op() {
        let (root, paths) = test_paths("rename_noop");
        let repository = seeded_repository(&paths).await;
        let before = read_pair(&paths);

        rename_account_with_repository(
            &repository,
            ACCOUNT_ID_A.to_string(),
            DISPLAY_NAME_A.to_string(),
        )
        .await
        .unwrap();

        assert_eq!(read_pair(&paths), before);

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_masked_adapter_preserves_ordering_stale_ids_and_duplicates() {
        let (root, paths) = test_paths("masked_ordering");
        let repository = seeded_repository(&paths).await;
        let ids = vec![
            "stale-command-id".to_string(),
            ACCOUNT_ID_A.to_string(),
            "stale-command-id".to_string(),
            ACCOUNT_ID_B.to_string(),
        ];

        set_masked_account_ids_with_repository(&repository, ids.clone())
            .await
            .unwrap();

        assert_eq!(repository.get_masked_account_ids().await.unwrap(), ids);

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_identical_masked_vector_is_a_byte_for_byte_no_op() {
        let (root, paths) = test_paths("masked_noop");
        let repository = seeded_repository(&paths).await;
        let ids = vec!["stale-command-id".to_string(), ACCOUNT_ID_A.to_string()];
        set_masked_account_ids_with_repository(&repository, ids.clone())
            .await
            .unwrap();
        let before = read_pair(&paths);

        set_masked_account_ids_with_repository(&repository, ids)
            .await
            .unwrap();

        assert_eq!(read_pair(&paths), before);

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_empty_masked_vector_is_preserved() {
        let (root, paths) = test_paths("masked_empty_existing");
        let repository = seeded_repository(&paths).await;

        set_masked_account_ids_with_repository(&repository, vec![ACCOUNT_ID_A.to_string()])
            .await
            .unwrap();
        set_masked_account_ids_with_repository(&repository, Vec::new())
            .await
            .unwrap();

        assert!(repository
            .get_masked_account_ids()
            .await
            .unwrap()
            .is_empty());

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_setting_masked_ids_on_empty_creates_repository_defined_secure_pair() {
        let (root, paths) = test_paths("masked_empty_store");
        let repository = AccountRepository::for_test(paths.clone());

        set_masked_account_ids_with_repository(
            &repository,
            vec!["stale-empty-command-id".to_string()],
        )
        .await
        .unwrap();

        assert!(paths.metadata_file.exists());
        assert!(paths.vault_file.exists());
        assert_eq!(
            repository.validate_startup_state().await.unwrap(),
            crate::auth::account_repository::RepositoryFormat::Secure
        );
        let metadata = load_metadata(&paths);
        let vault = load_vault(&paths);
        assert!(metadata.is_empty());
        assert_eq!(metadata.masked_account_ids, ["stale-empty-command-id"]);
        assert!(vault.is_empty());

        drop(repository);
        drop(metadata);
        drop(vault);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_delete_on_legacy_returns_legacy_migration_required() {
        let (root, paths) = test_paths("legacy_delete");
        write_legacy(&paths);
        let repository = AccountRepository::for_test(paths);

        let error = delete_account_with_repository(&repository, ACCOUNT_ID_A.to_string())
            .await
            .unwrap_err();

        assert_eq!(error, "Legacy account storage requires secure migration");

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_rename_on_legacy_returns_legacy_migration_required() {
        let (root, paths) = test_paths("legacy_rename");
        write_legacy(&paths);
        let repository = AccountRepository::for_test(paths);

        let error = rename_account_with_repository(
            &repository,
            ACCOUNT_ID_A.to_string(),
            "Legacy Rename Attempt".to_string(),
        )
        .await
        .unwrap_err();

        assert_eq!(error, "Legacy account storage requires secure migration");

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_masked_update_on_legacy_returns_legacy_migration_required() {
        let (root, paths) = test_paths("legacy_masked");
        write_legacy(&paths);
        let repository = AccountRepository::for_test(paths);

        let error =
            set_masked_account_ids_with_repository(&repository, vec!["legacy-attempt".to_string()])
                .await
                .unwrap_err();

        assert_eq!(error, "Legacy account storage requires secure migration");

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_all_legacy_mutations_preserve_accounts_bytes() {
        let (root, paths) = test_paths("legacy_metadata_unchanged");
        let metadata_before = write_legacy(&paths);
        let repository = AccountRepository::for_test(paths.clone());

        assert!(
            delete_account_with_repository(&repository, ACCOUNT_ID_A.to_string())
                .await
                .is_err()
        );
        assert!(rename_account_with_repository(
            &repository,
            ACCOUNT_ID_A.to_string(),
            "Legacy Rename Attempt".to_string(),
        )
        .await
        .is_err());
        assert!(set_masked_account_ids_with_repository(
            &repository,
            vec!["legacy-attempt".to_string()],
        )
        .await
        .is_err());

        assert_eq!(
            std::fs::read(&paths.metadata_file).unwrap(),
            metadata_before
        );

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_all_legacy_mutations_preserve_orphan_vault_bytes() {
        let (root, paths) = test_paths("legacy_orphan_unchanged");
        write_legacy(&paths);
        let vault_before = write_orphan_vault(&paths);
        let repository = AccountRepository::for_test(paths.clone());

        assert!(
            delete_account_with_repository(&repository, ACCOUNT_ID_A.to_string())
                .await
                .is_err()
        );
        assert!(rename_account_with_repository(
            &repository,
            ACCOUNT_ID_A.to_string(),
            "Legacy Rename Attempt".to_string(),
        )
        .await
        .is_err());
        assert!(set_masked_account_ids_with_repository(
            &repository,
            vec!["legacy-attempt".to_string()],
        )
        .await
        .is_err());

        assert_eq!(std::fs::read(&paths.vault_file).unwrap(), vault_before);

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_command_errors_do_not_contain_synthetic_values_or_paths() {
        let (root, paths) = test_paths("error_secrecy");
        let repository = seeded_repository(&paths).await;
        let delete_error =
            delete_account_with_repository(&repository, "secret-command-unknown-id".to_string())
                .await
                .unwrap_err();
        let rename_error = rename_account_with_repository(
            &repository,
            ACCOUNT_ID_A.to_string(),
            DISPLAY_NAME_B.to_string(),
        )
        .await
        .unwrap_err();
        let blank_error = rename_account_with_repository(
            &repository,
            ACCOUNT_ID_A.to_string(),
            "   ".to_string(),
        )
        .await
        .unwrap_err();

        for error in [delete_error, rename_error, blank_error] {
            assert_sanitized(&error, &root);
        }

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_shared_helpers_route_directly_to_repository_mutations() {
        let (root, paths) = test_paths("shared_helpers");
        let repository = seeded_repository(&paths).await;

        rename_account_with_repository(
            &repository,
            ACCOUNT_ID_A.to_string(),
            "Shared Helper Rename".to_string(),
        )
        .await
        .unwrap();
        set_masked_account_ids_with_repository(&repository, vec![ACCOUNT_ID_A.to_string()])
            .await
            .unwrap();
        delete_account_with_repository(&repository, ACCOUNT_ID_B.to_string())
            .await
            .unwrap();

        let metadata = load_metadata(&paths);
        assert_eq!(metadata.accounts.len(), 1);
        assert_eq!(metadata.accounts[0].display_name, "Shared Helper Rename");
        assert_eq!(metadata.masked_account_ids, [ACCOUNT_ID_A]);

        drop(repository);
        drop(metadata);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_shared_helpers_preserve_web_result_shapes() {
        let (root, paths) = test_paths("web_result_shapes");
        let repository = seeded_repository(&paths).await;

        let rename_result: Result<(), String> = rename_account_with_repository(
            &repository,
            ACCOUNT_ID_A.to_string(),
            "Result Shape Rename".to_string(),
        )
        .await;
        let masked_result: Result<(), String> =
            set_masked_account_ids_with_repository(&repository, Vec::new()).await;
        let delete_result: Result<(), String> =
            delete_account_with_repository(&repository, ACCOUNT_ID_B.to_string()).await;

        assert!(rename_result.is_ok());
        assert!(masked_result.is_ok());
        assert!(delete_result.is_ok());

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_secure_helpers_never_fall_back_to_legacy_storage() {
        let (root, paths) = test_paths("no_legacy_fallback");
        let metadata_before = write_legacy(&paths);
        let vault_before = write_orphan_vault(&paths);
        let repository = AccountRepository::for_test(paths.clone());

        let delete_error = delete_account_with_repository(&repository, ACCOUNT_ID_A.to_string())
            .await
            .unwrap_err();
        let rename_error = rename_account_with_repository(
            &repository,
            ACCOUNT_ID_A.to_string(),
            "Fallback Attempt".to_string(),
        )
        .await
        .unwrap_err();
        let masked_error = set_masked_account_ids_with_repository(
            &repository,
            vec!["fallback-attempt".to_string()],
        )
        .await
        .unwrap_err();

        assert_eq!(
            delete_error,
            "Legacy account storage requires secure migration"
        );
        assert_eq!(
            rename_error,
            "Legacy account storage requires secure migration"
        );
        assert_eq!(
            masked_error,
            "Legacy account storage requires secure migration"
        );
        assert_eq!(
            std::fs::read(&paths.metadata_file).unwrap(),
            metadata_before
        );
        assert_eq!(std::fs::read(&paths.vault_file).unwrap(), vault_before);

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_add_api_key_from_secure_text_stores_metadata_and_vault() {
        let (root, paths) = test_paths("add_api_text");
        let repository = AccountRepository::for_test(paths.clone());

        let info = add_account_from_auth_json_text_with_repository(
            &repository,
            "Imported API".to_string(),
            api_key_auth_json(API_KEY_A),
        )
        .await
        .unwrap();

        assert_eq!(info.name, "Imported API");
        assert_eq!(info.auth_mode, AuthMode::ApiKey);
        assert!(info.is_active);
        let metadata = load_metadata(&paths);
        let vault = load_vault(&paths);
        assert_eq!(metadata.accounts.len(), 1);
        assert_eq!(metadata.get(&info.id).unwrap().display_name, "Imported API");
        assert_api_key_secret(&vault, &info.id);

        drop(repository);
        drop(metadata);
        drop(vault);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_add_chatgpt_from_secure_text_stores_metadata_and_vault() {
        let (root, paths) = test_paths("add_chatgpt_text");
        let repository = AccountRepository::for_test(paths.clone());
        let id_token = valid_chatgpt_id_token(Some(EMAIL_A), CHATGPT_ACCOUNT_A);

        let info = add_account_from_auth_json_text_with_repository(
            &repository,
            "Imported ChatGPT".to_string(),
            chatgpt_auth_json(Some(EMAIL_A), CHATGPT_ACCOUNT_A),
        )
        .await
        .unwrap();

        assert_eq!(info.name, "Imported ChatGPT");
        assert_eq!(info.email.as_deref(), Some(EMAIL_A));
        assert_eq!(info.plan_type.as_deref(), Some("plus"));
        assert_eq!(info.auth_mode, AuthMode::ChatGPT);
        assert!(info.is_active);
        let metadata = load_metadata(&paths);
        let vault = load_vault(&paths);
        assert_eq!(
            metadata.get(&info.id).unwrap().auth_kind,
            MetadataAuthKind::ChatGpt
        );
        match vault.get(&info.id) {
            Some(SecretRecord::ChatGpt {
                id_token: stored_id_token,
                access_token,
                refresh_token,
                account_id,
            }) => {
                assert_eq!(stored_id_token, &id_token);
                assert_eq!(access_token, ACCESS_TOKEN_A);
                assert_eq!(refresh_token, REFRESH_TOKEN_A);
                assert_eq!(account_id.as_deref(), Some(CHATGPT_ACCOUNT_A));
            }
            _ => panic!("expected imported ChatGPT secret"),
        }

        drop(repository);
        drop(metadata);
        drop(vault);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_first_secure_add_becomes_active() {
        let (root, paths) = test_paths("first_add_active");
        let repository = AccountRepository::for_test(paths.clone());

        let info = add_account_from_auth_json_text_with_repository(
            &repository,
            "First Account".to_string(),
            api_key_auth_json(API_KEY_A),
        )
        .await
        .unwrap();

        assert!(info.is_active);
        let active = repository.get_active_account().await.unwrap().unwrap();
        assert_eq!(active.id, info.id);
        assert!(active.is_active);

        drop(repository);
        drop(active);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_second_secure_add_preserves_active_account() {
        let (root, paths) = test_paths("second_add_active");
        let repository = AccountRepository::for_test(paths.clone());

        let first = add_account_from_auth_json_text_with_repository(
            &repository,
            "First Account".to_string(),
            api_key_auth_json(API_KEY_A),
        )
        .await
        .unwrap();
        let second = add_account_from_auth_json_text_with_repository(
            &repository,
            "Second Account".to_string(),
            api_key_auth_json(API_KEY_A),
        )
        .await
        .unwrap();

        assert!(first.is_active);
        assert!(!second.is_active);
        assert_eq!(
            repository.get_active_account().await.unwrap().unwrap().id,
            first.id
        );

        drop(repository);
        drop(first);
        drop(second);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_secure_add_preserves_account_order() {
        let (root, paths) = test_paths("add_order");
        let repository = AccountRepository::for_test(paths.clone());

        add_account_from_auth_json_text_with_repository(
            &repository,
            "First Ordered Account".to_string(),
            api_key_auth_json(API_KEY_A),
        )
        .await
        .unwrap();
        add_account_from_auth_json_text_with_repository(
            &repository,
            "Second Ordered Account".to_string(),
            api_key_auth_json(API_KEY_A),
        )
        .await
        .unwrap();

        let metadata = load_metadata(&paths);
        assert_eq!(
            metadata
                .accounts
                .iter()
                .map(|account| account.display_name.as_str())
                .collect::<Vec<_>>(),
            ["First Ordered Account", "Second Ordered Account"]
        );

        drop(repository);
        drop(metadata);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_secure_add_account_info_contains_no_secret() {
        let (root, paths) = test_paths("add_info_public");
        let repository = AccountRepository::for_test(paths.clone());

        let info = add_account_from_auth_json_text_with_repository(
            &repository,
            "Public Account".to_string(),
            chatgpt_auth_json(Some(EMAIL_A), CHATGPT_ACCOUNT_A),
        )
        .await
        .unwrap();
        let serialized = serde_json::to_string(&info).unwrap();
        for secret in [
            ID_TOKEN_A,
            ACCESS_TOKEN_A,
            REFRESH_TOKEN_A,
            API_KEY_A,
            CHATGPT_ACCOUNT_A,
        ] {
            assert!(!serialized.contains(secret));
        }

        drop(repository);
        drop(info);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_add_api_key_from_file_uses_secure_repository() {
        let (root, paths) = test_paths("add_api_file");
        let auth_path = write_auth_file(&root, &api_key_auth_json(API_KEY_A), "api-auth");
        let repository = AccountRepository::for_test(paths.clone());

        let info = add_account_from_file_with_repository(
            &repository,
            auth_path.to_string_lossy().into_owned(),
            "File API".to_string(),
        )
        .await
        .unwrap();

        assert_eq!(info.name, "File API");
        assert_eq!(info.auth_mode, AuthMode::ApiKey);
        assert_api_key_secret(&load_vault(&paths), &info.id);

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_add_chatgpt_from_file_uses_secure_repository() {
        let (root, paths) = test_paths("add_chatgpt_file");
        let auth_path = write_auth_file(
            &root,
            &chatgpt_auth_json(Some(EMAIL_A), CHATGPT_ACCOUNT_A),
            "chatgpt-auth",
        );
        let repository = AccountRepository::for_test(paths.clone());

        let info = add_account_from_file_with_repository(
            &repository,
            auth_path.to_string_lossy().into_owned(),
            "File ChatGPT".to_string(),
        )
        .await
        .unwrap();

        assert_eq!(info.name, "File ChatGPT");
        assert_eq!(info.auth_mode, AuthMode::ChatGPT);
        let metadata = load_metadata(&paths);
        let vault = load_vault(&paths);
        assert_eq!(
            metadata.get(&info.id).unwrap().auth_kind,
            MetadataAuthKind::ChatGpt
        );
        assert!(matches!(
            vault.get(&info.id),
            Some(SecretRecord::ChatGpt { .. })
        ));

        drop(repository);
        drop(metadata);
        drop(vault);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_file_and_text_auth_json_adapters_have_same_semantics() {
        let (file_root, file_paths) = test_paths("file_text_file");
        let (text_root, text_paths) = test_paths("file_text_text");
        let contents = chatgpt_auth_json(Some(EMAIL_A), CHATGPT_ACCOUNT_A);
        let auth_path = write_auth_file(&file_root, &contents, "equivalent-auth");
        let file_repository = AccountRepository::for_test(file_paths.clone());
        let text_repository = AccountRepository::for_test(text_paths.clone());

        let file_info = add_account_from_file_with_repository(
            &file_repository,
            auth_path.to_string_lossy().into_owned(),
            "Equivalent Account".to_string(),
        )
        .await
        .unwrap();
        let text_info = add_account_from_auth_json_text_with_repository(
            &text_repository,
            "Equivalent Account".to_string(),
            contents,
        )
        .await
        .unwrap();

        assert_eq!(file_info.name, text_info.name);
        assert_eq!(file_info.email, text_info.email);
        assert_eq!(file_info.plan_type, text_info.plan_type);
        assert_eq!(file_info.auth_mode, text_info.auth_mode);
        let file_metadata = load_metadata(&file_paths);
        let text_metadata = load_metadata(&text_paths);
        let file_vault = load_vault(&file_paths);
        let text_vault = load_vault(&text_paths);
        assert_eq!(
            file_metadata.get(&file_info.id).unwrap().auth_kind,
            text_metadata.get(&text_info.id).unwrap().auth_kind
        );
        assert!(matches!(
            file_vault.get(&file_info.id),
            Some(SecretRecord::ChatGpt { .. })
        ));
        assert!(matches!(
            text_vault.get(&text_info.id),
            Some(SecretRecord::ChatGpt { .. })
        ));

        drop(file_repository);
        drop(text_repository);
        drop(file_metadata);
        drop(text_metadata);
        drop(file_vault);
        drop(text_vault);
        cleanup(file_root);
        cleanup(text_root);
    }

    #[tokio::test]
    async fn test_explicit_name_trimming_is_preserved_by_secure_add() {
        let (root, paths) = test_paths("trimmed_name");
        let repository = AccountRepository::for_test(paths.clone());

        let info = add_account_from_auth_json_text_with_repository(
            &repository,
            "  Trimmed Imported Name  ".to_string(),
            api_key_auth_json(API_KEY_A),
        )
        .await
        .unwrap();

        assert_eq!(info.name, "Trimmed Imported Name");

        drop(repository);
        drop(info);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_blank_chatgpt_name_uses_email_fallback() {
        let (root, paths) = test_paths("chatgpt_email_fallback");
        let repository = AccountRepository::for_test(paths.clone());

        let info = add_account_from_auth_json_text_with_repository(
            &repository,
            String::new(),
            chatgpt_auth_json(Some(EMAIL_A), CHATGPT_ACCOUNT_A),
        )
        .await
        .unwrap();

        assert_eq!(info.name, EMAIL_A);
        assert_eq!(info.email.as_deref(), Some(EMAIL_A));

        drop(repository);
        drop(info);
        cleanup(root);
    }

    #[test]
    fn test_secure_insert_maps_stored_account_metadata_exactly() {
        let account = stored_chatgpt("mapped-chatgpt-id", "Mapped ChatGPT");
        let expected_created_at = account.created_at;
        let expected_last_used_at = account.last_used_at;
        let insert = secure_insert_from_stored(account).unwrap();

        assert_eq!(insert.metadata.id, "mapped-chatgpt-id");
        assert_eq!(insert.metadata.vault_ref, "mapped-chatgpt-id");
        assert_eq!(insert.metadata.display_name, "Mapped ChatGPT");
        assert_eq!(
            insert.metadata.email.as_deref(),
            Some("mapped-chatgpt@example.test")
        );
        assert_eq!(insert.metadata.plan_type.as_deref(), Some("pro"));
        assert_eq!(insert.metadata.subscription_expires_at, Some(timestamp(21)));
        assert_eq!(insert.metadata.created_at, expected_created_at);
        assert_eq!(insert.metadata.last_used_at, expected_last_used_at);
        assert_eq!(insert.metadata.auth_kind, MetadataAuthKind::ChatGpt);
        assert!(matches!(insert.secret, SecretRecord::ChatGpt { .. }));
    }

    #[test]
    fn test_secure_insert_maps_api_key_metadata_and_secret_exactly() {
        let insert = secure_insert_from_stored(stored_api_key(
            "mapped-api-key-id",
            "Mapped API Key",
            API_KEY_A,
        ))
        .unwrap();

        assert_eq!(insert.metadata.id, "mapped-api-key-id");
        assert_eq!(insert.metadata.vault_ref, "mapped-api-key-id");
        assert_eq!(insert.metadata.display_name, "Mapped API Key");
        assert_eq!(
            insert.metadata.email.as_deref(),
            Some("mapped@example.test")
        );
        assert_eq!(insert.metadata.plan_type.as_deref(), Some("team"));
        assert_eq!(insert.metadata.subscription_expires_at, Some(timestamp(20)));
        assert_eq!(insert.metadata.created_at, timestamp(1));
        assert_eq!(insert.metadata.last_used_at, Some(timestamp(2)));
        assert_eq!(insert.metadata.auth_kind, MetadataAuthKind::ApiKey);
        match &insert.secret {
            SecretRecord::ApiKey { key } => assert_eq!(key, API_KEY_A),
            _ => panic!("expected API key secret"),
        }
    }

    #[tokio::test]
    async fn test_api_key_is_only_in_decrypted_vault() {
        let (root, paths) = test_paths("api_secret_boundary");
        let repository = AccountRepository::for_test(paths.clone());
        let info = add_account_from_auth_json_text_with_repository(
            &repository,
            "API Secret Boundary".to_string(),
            api_key_auth_json(API_KEY_A),
        )
        .await
        .unwrap();

        let metadata_bytes = std::fs::read(&paths.metadata_file).unwrap();
        let metadata_text = String::from_utf8_lossy(&metadata_bytes);
        assert!(!metadata_text.contains(API_KEY_A));
        let vault = load_vault(&paths);
        assert_api_key_secret(&vault, &info.id);

        drop(repository);
        drop(info);
        drop(vault);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_chatgpt_secrets_are_only_in_decrypted_vault() {
        let (root, paths) = test_paths("chatgpt_secret_boundary");
        let repository = AccountRepository::for_test(paths.clone());
        let info = add_account_from_auth_json_text_with_repository(
            &repository,
            "ChatGPT Secret Boundary".to_string(),
            chatgpt_auth_json(Some(EMAIL_A), CHATGPT_ACCOUNT_A),
        )
        .await
        .unwrap();
        let metadata_bytes = std::fs::read(&paths.metadata_file).unwrap();
        let metadata_text = String::from_utf8_lossy(&metadata_bytes);
        for secret in [ACCESS_TOKEN_A, REFRESH_TOKEN_A, CHATGPT_ACCOUNT_A] {
            assert!(!metadata_text.contains(secret));
        }
        let vault = load_vault(&paths);
        assert!(matches!(
            vault.get(&info.id),
            Some(SecretRecord::ChatGpt { .. })
        ));

        drop(repository);
        drop(info);
        drop(vault);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_secure_metadata_has_schema_version_and_no_legacy_version() {
        let (root, paths) = test_paths("secure_schema_version");
        let repository = AccountRepository::for_test(paths.clone());
        add_account_from_auth_json_text_with_repository(
            &repository,
            "Schema Account".to_string(),
            api_key_auth_json(API_KEY_A),
        )
        .await
        .unwrap();

        let metadata_json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&paths.metadata_file).unwrap()).unwrap();
        assert_eq!(metadata_json["schema_version"], serde_json::json!(2));
        assert!(metadata_json.get("version").is_none());

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_secure_metadata_contains_no_prohibited_secret_fields() {
        let (root, paths) = test_paths("secure_metadata_fields");
        let repository = AccountRepository::for_test(paths.clone());
        add_account_from_auth_json_text_with_repository(
            &repository,
            "Field Boundary Account".to_string(),
            chatgpt_auth_json(Some(EMAIL_A), CHATGPT_ACCOUNT_A),
        )
        .await
        .unwrap();

        let metadata_json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&paths.metadata_file).unwrap()).unwrap();
        let serialized = metadata_json.to_string();
        for prohibited in ["id_token", "access_token", "refresh_token", "key"] {
            assert!(!serialized.contains(&format!("\"{prohibited}\"")));
        }

        drop(repository);
        cleanup(root);
    }

    #[test]
    fn test_auth_mode_and_data_mismatch_is_sanitized_without_partial_insert() {
        let (root, paths) = test_paths("auth_mismatch");
        let mut account = stored_chatgpt("mismatch-account", "Mismatch Account");
        account.auth_mode = AuthMode::ApiKey;

        let error = secure_insert_from_stored(account).err().unwrap();
        assert_eq!(error, AUTH_DATA_MISMATCH_ERROR);
        assert_sanitized(&error, &root);
        assert!(!paths.metadata_file.exists());
        assert!(!paths.vault_file.exists());

        cleanup(root);
    }

    #[tokio::test]
    async fn test_malformed_auth_json_text_leaves_account_files_absent() {
        let (root, paths) = test_paths("malformed_text");
        let repository = AccountRepository::for_test(paths.clone());
        let error = add_account_from_auth_json_text_with_repository(
            &repository,
            "Malformed Text".to_string(),
            "{\"tokens\":".to_string(),
        )
        .await
        .unwrap_err();

        assert_eq!(error, AUTH_JSON_INVALID_ERROR);
        assert!(!paths.metadata_file.exists());
        assert!(!paths.vault_file.exists());

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_malformed_auth_json_file_leaves_account_files_absent() {
        let (root, paths) = test_paths("malformed_file");
        let auth_path = write_auth_file(&root, "{\"tokens\":", "malformed-auth");
        let repository = AccountRepository::for_test(paths.clone());
        let error = add_account_from_file_with_repository(
            &repository,
            auth_path.to_string_lossy().into_owned(),
            "Malformed File".to_string(),
        )
        .await
        .unwrap_err();

        assert_eq!(error, AUTH_JSON_INVALID_ERROR);
        assert!(!paths.metadata_file.exists());
        assert!(!paths.vault_file.exists());

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_text_auth_json_size_limit_leaves_account_files_absent() {
        let (root, paths) = test_paths("oversized_text");
        let repository = AccountRepository::for_test(paths.clone());
        let error = add_account_from_auth_json_text_with_repository(
            &repository,
            "Oversized Text".to_string(),
            "x".repeat(MAX_IMPORT_JSON_BYTES as usize + 1),
        )
        .await
        .unwrap_err();

        assert_eq!(error, AUTH_FILE_TOO_LARGE_ERROR);
        assert!(!paths.metadata_file.exists());
        assert!(!paths.vault_file.exists());

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_file_auth_json_size_limit_leaves_account_files_absent() {
        let (root, paths) = test_paths("oversized_file");
        let auth_path = root.join("oversized-auth.json");
        std::fs::write(&auth_path, vec![b'x'; MAX_IMPORT_JSON_BYTES as usize + 1]).unwrap();
        let repository = AccountRepository::for_test(paths.clone());
        let error = add_account_from_file_with_repository(
            &repository,
            auth_path.to_string_lossy().into_owned(),
            "Oversized File".to_string(),
        )
        .await
        .unwrap_err();

        assert_eq!(error, AUTH_FILE_TOO_LARGE_ERROR);
        assert!(!paths.metadata_file.exists());
        assert!(!paths.vault_file.exists());

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_missing_auth_json_file_leaves_account_files_absent() {
        let (root, paths) = test_paths("missing_auth_file");
        let repository = AccountRepository::for_test(paths.clone());
        let missing_path = root.join("does-not-exist-auth.json");
        let error = add_account_from_file_with_repository(
            &repository,
            missing_path.to_string_lossy().into_owned(),
            "Missing File".to_string(),
        )
        .await
        .unwrap_err();

        assert_eq!(error, AUTH_FILE_OPEN_ERROR);
        assert!(!paths.metadata_file.exists());
        assert!(!paths.vault_file.exists());
        assert!(!error.contains(root.to_string_lossy().as_ref()));

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_unreadable_auth_json_path_leaves_account_files_absent() {
        let (root, paths) = test_paths("unreadable_auth_path");
        let repository = AccountRepository::for_test(paths.clone());
        let error = add_account_from_file_with_repository(
            &repository,
            root.to_string_lossy().into_owned(),
            "Unreadable Path".to_string(),
        )
        .await
        .unwrap_err();

        assert!(
            error == AUTH_FILE_OPEN_ERROR || error == AUTH_FILE_READ_ERROR,
            "unexpected sanitized file error: {error}"
        );
        assert!(!paths.metadata_file.exists());
        assert!(!paths.vault_file.exists());
        assert!(!error.contains(root.to_string_lossy().as_ref()));

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_invalid_utf8_auth_json_file_leaves_account_files_absent() {
        let (root, paths) = test_paths("invalid_utf8_auth");
        let auth_path = root.join("invalid-utf8-auth.json");
        std::fs::write(&auth_path, [0xFF, 0xFE, 0xFD]).unwrap();
        let repository = AccountRepository::for_test(paths.clone());
        let error = add_account_from_file_with_repository(
            &repository,
            auth_path.to_string_lossy().into_owned(),
            "Invalid UTF8".to_string(),
        )
        .await
        .unwrap_err();

        assert_eq!(error, AUTH_JSON_UTF8_ERROR);
        assert!(!paths.metadata_file.exists());
        assert!(!paths.vault_file.exists());

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_auth_json_without_credentials_leaves_account_files_absent() {
        let (root, paths) = test_paths("no_auth_credentials");
        let repository = AccountRepository::for_test(paths.clone());
        let error = add_account_from_auth_json_text_with_repository(
            &repository,
            "No Credentials".to_string(),
            "{}".to_string(),
        )
        .await
        .unwrap_err();

        assert_eq!(error, AUTH_JSON_NO_CREDENTIALS_ERROR);
        assert!(!paths.metadata_file.exists());
        assert!(!paths.vault_file.exists());

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_duplicate_display_name_preserves_secure_pair_bytes() {
        let (root, paths) = test_paths("duplicate_add_name");
        let repository = seeded_repository(&paths).await;
        let before = read_pair(&paths);
        let error = add_account_from_auth_json_text_with_repository(
            &repository,
            DISPLAY_NAME_A.to_string(),
            api_key_auth_json(API_KEY_A),
        )
        .await
        .unwrap_err();

        assert_eq!(error, "Duplicate display name");
        assert_eq!(read_pair(&paths), before);
        assert!(!error.contains(DISPLAY_NAME_A));

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_auth_json_text_on_legacy_requires_migration() {
        let (root, paths) = test_paths("legacy_add_text");
        let metadata_before = write_legacy(&paths);
        let repository = AccountRepository::for_test(paths.clone());
        let error = add_account_from_auth_json_text_with_repository(
            &repository,
            "Legacy Text Attempt".to_string(),
            api_key_auth_json(API_KEY_A),
        )
        .await
        .unwrap_err();

        assert_eq!(error, "Legacy account storage requires secure migration");
        assert_eq!(
            std::fs::read(&paths.metadata_file).unwrap(),
            metadata_before
        );

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_auth_json_file_on_legacy_requires_migration() {
        let (root, paths) = test_paths("legacy_add_file");
        let metadata_before = write_legacy(&paths);
        let auth_path = write_auth_file(&root, &api_key_auth_json(API_KEY_A), "legacy-auth");
        let repository = AccountRepository::for_test(paths.clone());
        let error = add_account_from_file_with_repository(
            &repository,
            auth_path.to_string_lossy().into_owned(),
            "Legacy File Attempt".to_string(),
        )
        .await
        .unwrap_err();

        assert_eq!(error, "Legacy account storage requires secure migration");
        assert_eq!(
            std::fs::read(&paths.metadata_file).unwrap(),
            metadata_before
        );

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_auth_json_add_on_legacy_preserves_metadata_and_orphan_vault() {
        let (root, paths) = test_paths("legacy_add_unchanged");
        let metadata_before = write_legacy(&paths);
        let vault_before = write_orphan_vault(&paths);
        let auth_path = write_auth_file(&root, &api_key_auth_json(API_KEY_A), "legacy-auth");
        let repository = AccountRepository::for_test(paths.clone());

        let text_error = add_account_from_auth_json_text_with_repository(
            &repository,
            "Legacy Text Attempt".to_string(),
            api_key_auth_json(API_KEY_A),
        )
        .await
        .unwrap_err();
        let file_error = add_account_from_file_with_repository(
            &repository,
            auth_path.to_string_lossy().into_owned(),
            "Legacy File Attempt".to_string(),
        )
        .await
        .unwrap_err();

        assert_eq!(
            text_error,
            "Legacy account storage requires secure migration"
        );
        assert_eq!(
            file_error,
            "Legacy account storage requires secure migration"
        );
        assert_eq!(
            std::fs::read(&paths.metadata_file).unwrap(),
            metadata_before
        );
        assert_eq!(std::fs::read(&paths.vault_file).unwrap(), vault_before);

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_concurrent_unique_auth_json_adds_do_not_lose_accounts() {
        let (root, paths) = test_paths("concurrent_adds");
        let repository_a = AccountRepository::for_test(paths.clone());
        let repository_b = AccountRepository::for_test(paths.clone());

        let (left, right) = tokio::join!(
            add_account_from_auth_json_text_with_repository(
                &repository_a,
                "Concurrent Account A".to_string(),
                api_key_auth_json(API_KEY_A),
            ),
            add_account_from_auth_json_text_with_repository(
                &repository_b,
                "Concurrent Account B".to_string(),
                api_key_auth_json(API_KEY_A),
            )
        );
        let left = left.unwrap();
        let right = right.unwrap();

        let metadata = load_metadata(&paths);
        let vault = load_vault(&paths);
        assert_eq!(metadata.accounts.len(), 2);
        assert_eq!(vault.len(), 2);
        assert!(metadata
            .accounts
            .iter()
            .any(|account| account.display_name == left.name));
        assert!(metadata
            .accounts
            .iter()
            .any(|account| account.display_name == right.name));

        drop(repository_a);
        drop(repository_b);
        drop(left);
        drop(right);
        drop(metadata);
        drop(vault);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_auth_json_errors_exclude_credentials_raw_json_and_paths() {
        let (root, paths) = test_paths("auth_error_secrecy");
        let repository = AccountRepository::for_test(paths.clone());
        let raw_json = format!(
            "{{\"OPENAI_API_KEY\":\"{API_KEY_A}\",\"id_token\":\"{ID_TOKEN_A}\",\
\"access_token\":\"{ACCESS_TOKEN_A}\",\"refresh_token\":\"{REFRESH_TOKEN_A}\",\
\"account_id\":\"{CHATGPT_ACCOUNT_A}\",\"email\":\"{EMAIL_A}\",\
\"display_name\":\"{DISPLAY_NAME_A}\""
        );
        let malformed_error = add_account_from_auth_json_text_with_repository(
            &repository,
            DISPLAY_NAME_A.to_string(),
            raw_json.clone(),
        )
        .await
        .unwrap_err();
        assert_eq!(malformed_error, AUTH_JSON_INVALID_ERROR);
        assert_sanitized(&malformed_error, &root);
        assert!(!malformed_error.contains(&raw_json));

        let missing_error = add_account_from_file_with_repository(
            &repository,
            root.join("secret-path-auth.json")
                .to_string_lossy()
                .into_owned(),
            DISPLAY_NAME_A.to_string(),
        )
        .await
        .unwrap_err();
        assert_eq!(missing_error, AUTH_FILE_OPEN_ERROR);
        assert_sanitized(&missing_error, &root);

        drop(repository);
        cleanup(root);
    }
}
