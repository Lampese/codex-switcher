//! Account management Tauri commands

use crate::auth::{
    add_account, clear_scheduled_warmup_prompt, create_chatgpt_account_from_refresh_token,
    get_active_account, get_keychain_secret, get_or_create_keychain_secret, import_from_auth_json,
    load_accounts, load_settings, remove_account, save_accounts, set_active_account,
    set_export_security_mode, set_scheduled_warmup, switch_to_account, touch_account,
    write_bytes_atomic,
};
use crate::commands::{
    collect_running_codex_processes, gracefully_stop_codex_processes, restart_codex_processes,
};
use crate::scheduler::{
    current_local_date_string, get_scheduled_warmup_status as compute_scheduled_warmup_status,
    parse_local_time, run_scheduled_warmup_now as execute_scheduled_warmup_now,
    ScheduledWarmupRuntimeState,
};
use crate::types::{AccountInfo, AccountsStore, AuthData, ImportAccountsSummary, StoredAccount};
use crate::types::{
    AppSettings, ExportSecurityMode, ScheduledWarmupSettings, ScheduledWarmupStatus,
};

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
use tauri::{AppHandle, State};

const SLIM_EXPORT_PREFIX: &str = "css1.";
const SLIM_FORMAT_VERSION: u8 = 1;
const SLIM_AUTH_API_KEY: u8 = 0;
const SLIM_AUTH_CHATGPT: u8 = 1;

const FULL_FILE_MAGIC: &[u8; 4] = b"CSWF";
const FULL_FILE_VERSION: u8 = 2;
const FULL_SALT_LEN: usize = 16;
const FULL_NONCE_LEN: usize = 24;
const FULL_KDF_ITERATIONS: u32 = 210_000;
const FULL_PRESET_PASSPHRASE: &str = "gT7kQ9mV2xN4pL8sR1dH6zW3cB5yF0uJ_aE7nK2tP9vM4rX1";
const FULL_SECURITY_LESS_SECURE: u8 = 0;
const FULL_SECURITY_PASSPHRASE: u8 = 1;
const FULL_SECURITY_KEYCHAIN: u8 = 2;

const MAX_IMPORT_JSON_BYTES: u64 = 2 * 1024 * 1024;
const MAX_IMPORT_FILE_BYTES: u64 = 8 * 1024 * 1024;
const SLIM_IMPORT_CONCURRENCY: usize = 6;

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

/// List all accounts with their info
#[tauri::command]
pub async fn list_accounts() -> Result<Vec<AccountInfo>, String> {
    let store = load_accounts().map_err(|e| e.to_string())?;
    let active_id = store.active_account_id.as_deref();

    let accounts: Vec<AccountInfo> = store
        .accounts
        .iter()
        .map(|a| AccountInfo::from_stored(a, active_id))
        .collect();

    Ok(accounts)
}

/// Get the currently active account
#[tauri::command]
pub async fn get_active_account_info() -> Result<Option<AccountInfo>, String> {
    let store = load_accounts().map_err(|e| e.to_string())?;
    let active_id = store.active_account_id.as_deref();

    if let Some(active) = get_active_account().map_err(|e| e.to_string())? {
        Ok(Some(AccountInfo::from_stored(&active, active_id)))
    } else {
        Ok(None)
    }
}

/// Add an account from an auth.json file
#[tauri::command]
pub async fn add_account_from_file(path: String, name: String) -> Result<AccountInfo, String> {
    // Import from the file
    let account = import_from_auth_json(&path, name).map_err(|e| e.to_string())?;

    // Add to storage
    let stored = add_account(account).map_err(|e| e.to_string())?;

    let store = load_accounts().map_err(|e| e.to_string())?;
    let active_id = store.active_account_id.as_deref();

    Ok(AccountInfo::from_stored(&stored, active_id))
}

/// Switch to a different account
#[tauri::command]
pub async fn switch_account(
    account_id: String,
    restart_running_codex: Option<bool>,
) -> Result<(), String> {
    let store = load_accounts().map_err(|e| e.to_string())?;
    let running_processes = collect_running_codex_processes().map_err(|e| e.to_string())?;

    // Find the account
    let account = store
        .accounts
        .iter()
        .find(|a| a.id == account_id)
        .ok_or_else(|| format!("Account not found: {account_id}"))?;

    let should_restart = restart_running_codex.unwrap_or(false);
    if !running_processes.is_empty() && !should_restart {
        return Err(String::from(
            "Codex is currently running. Confirm a graceful restart before switching accounts.",
        ));
    }

    if should_restart {
        gracefully_stop_codex_processes(&running_processes).map_err(|e| e.to_string())?;
    }

    switch_to_account(account).map_err(|e| e.to_string())?;
    set_active_account(&account_id).map_err(|e| e.to_string())?;
    touch_account(&account_id).map_err(|e| e.to_string())?;

    if should_restart {
        restart_codex_processes(&running_processes).map_err(|e| e.to_string())?;
    }

    Ok(())
}

#[tauri::command]
pub async fn get_app_settings() -> Result<AppSettings, String> {
    load_settings().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn save_export_security_mode(mode: ExportSecurityMode) -> Result<AppSettings, String> {
    set_export_security_mode(mode).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn save_scheduled_warmup_settings(
    schedule: ScheduledWarmupSettings,
) -> Result<AppSettings, String> {
    if parse_local_time(&schedule.local_time).is_none() {
        return Err(String::from("Time must use HH:MM in local time"));
    }

    let store = load_accounts().map_err(|e| e.to_string())?;
    let valid_ids: HashSet<String> = store
        .accounts
        .iter()
        .map(|account| account.id.clone())
        .collect();
    let mut seen = HashSet::new();
    let mut sanitized_ids = Vec::new();

    for account_id in schedule.account_ids {
        if valid_ids.contains(&account_id) && seen.insert(account_id.clone()) {
            sanitized_ids.push(account_id);
        }
    }

    if schedule.enabled && sanitized_ids.is_empty() {
        return Err(String::from(
            "Select at least one account for scheduled warmups",
        ));
    }

    set_scheduled_warmup(ScheduledWarmupSettings {
        account_ids: sanitized_ids,
        ..schedule
    })
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_scheduled_warmup_status(
    runtime: State<'_, ScheduledWarmupRuntimeState>,
) -> Result<ScheduledWarmupStatus, String> {
    compute_scheduled_warmup_status(runtime.session_started_at())
}

#[tauri::command]
pub async fn dismiss_missed_scheduled_warmup() -> Result<AppSettings, String> {
    clear_scheduled_warmup_prompt(&current_local_date_string()).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn run_scheduled_warmup_now(
    app: AppHandle,
) -> Result<crate::types::WarmupSummary, String> {
    execute_scheduled_warmup_now(&app).await
}

/// Remove an account
#[tauri::command]
pub async fn delete_account(account_id: String) -> Result<(), String> {
    remove_account(&account_id).map_err(|e| e.to_string())?;
    Ok(())
}

/// Rename an account
#[tauri::command]
pub async fn rename_account(account_id: String, new_name: String) -> Result<(), String> {
    crate::auth::storage::update_account_metadata(&account_id, Some(new_name), None, None)
        .map_err(|e| e.to_string())?;
    Ok(())
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
pub async fn export_accounts_full_encrypted_file(
    path: String,
    passphrase: Option<String>,
) -> Result<(), String> {
    let store = load_accounts().map_err(|e| e.to_string())?;
    let settings = load_settings().map_err(|e| e.to_string())?;
    let mode = settings
        .export_security_mode
        .unwrap_or(ExportSecurityMode::LessSecure);
    let secret = resolve_export_secret(mode, passphrase).map_err(|e| e.to_string())?;
    let encrypted =
        encode_full_encrypted_store(&store, mode, &secret).map_err(|e| e.to_string())?;
    write_encrypted_file(&path, &encrypted).map_err(|e| e.to_string())?;
    Ok(())
}

/// Import full account config from an encrypted file, skipping existing accounts.
#[tauri::command]
pub async fn import_accounts_full_encrypted_file(
    path: String,
    passphrase: Option<String>,
) -> Result<ImportAccountsSummary, String> {
    let encrypted = read_encrypted_file(&path).map_err(|e| e.to_string())?;
    let imported =
        decode_full_encrypted_store(&encrypted, passphrase).map_err(|e| e.to_string())?;
    validate_imported_store(&imported).map_err(|e| e.to_string())?;

    let current = load_accounts().map_err(|e| e.to_string())?;
    let (merged, summary) = merge_accounts_store(current, imported);
    save_accounts(&merged).map_err(|e| e.to_string())?;
    Ok(summary)
}

fn resolve_export_secret(
    mode: ExportSecurityMode,
    passphrase: Option<String>,
) -> anyhow::Result<String> {
    match mode {
        ExportSecurityMode::LessSecure => Ok(String::from(FULL_PRESET_PASSPHRASE)),
        ExportSecurityMode::Passphrase => {
            let passphrase =
                passphrase.context("A passphrase is required for passphrase-protected backups")?;
            if passphrase.trim().is_empty() {
                anyhow::bail!("Passphrase cannot be empty");
            }
            Ok(passphrase)
        }
        ExportSecurityMode::Keychain => get_or_create_keychain_secret(),
    }
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

fn encode_full_encrypted_store(
    store: &AccountsStore,
    mode: ExportSecurityMode,
    passphrase: &str,
) -> anyhow::Result<Vec<u8>> {
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

    let mut out = Vec::with_capacity(4 + 1 + 1 + FULL_SALT_LEN + FULL_NONCE_LEN + ciphertext.len());
    out.extend_from_slice(FULL_FILE_MAGIC);
    out.push(FULL_FILE_VERSION);
    out.push(match mode {
        ExportSecurityMode::LessSecure => FULL_SECURITY_LESS_SECURE,
        ExportSecurityMode::Passphrase => FULL_SECURITY_PASSPHRASE,
        ExportSecurityMode::Keychain => FULL_SECURITY_KEYCHAIN,
    });
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);

    Ok(out)
}

fn decode_full_encrypted_store(
    file_bytes: &[u8],
    passphrase: Option<String>,
) -> anyhow::Result<AccountsStore> {
    if file_bytes.len() as u64 > MAX_IMPORT_FILE_BYTES {
        anyhow::bail!("Encrypted file is too large");
    }

    let header_len_v1 = 4 + 1 + FULL_SALT_LEN + FULL_NONCE_LEN;
    let header_len_v2 = 4 + 1 + 1 + FULL_SALT_LEN + FULL_NONCE_LEN;
    if file_bytes.len() <= header_len_v1 {
        anyhow::bail!("Encrypted file is invalid or truncated");
    }

    if &file_bytes[..4] != FULL_FILE_MAGIC {
        anyhow::bail!("Encrypted file header is invalid");
    }

    let version = file_bytes[4];
    let (mode, salt_start) = match version {
        1 => (ExportSecurityMode::LessSecure, 5),
        FULL_FILE_VERSION => {
            if file_bytes.len() <= header_len_v2 {
                anyhow::bail!("Encrypted file is invalid or truncated");
            }

            let mode = match file_bytes[5] {
                FULL_SECURITY_LESS_SECURE => ExportSecurityMode::LessSecure,
                FULL_SECURITY_PASSPHRASE => ExportSecurityMode::Passphrase,
                FULL_SECURITY_KEYCHAIN => ExportSecurityMode::Keychain,
                other => anyhow::bail!("Unsupported encrypted file security mode: {other}"),
            };
            (mode, 6)
        }
        _ => anyhow::bail!("Unsupported encrypted file version: {version}"),
    };

    let nonce_start = salt_start + FULL_SALT_LEN;
    let ciphertext_start = nonce_start + FULL_NONCE_LEN;

    let salt = &file_bytes[salt_start..nonce_start];
    let nonce = &file_bytes[nonce_start..ciphertext_start];
    let ciphertext = &file_bytes[ciphertext_start..];

    let secret = match mode {
        ExportSecurityMode::LessSecure => String::from(FULL_PRESET_PASSPHRASE),
        ExportSecurityMode::Passphrase => {
            let passphrase = passphrase
                .context("This backup requires the passphrase that was used to export it")?;
            if passphrase.trim().is_empty() {
                anyhow::bail!("Passphrase cannot be empty");
            }
            passphrase
        }
        ExportSecurityMode::Keychain => get_keychain_secret()?,
    };

    let key = derive_encryption_key(&secret, salt);
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
    let path = std::path::Path::new(path);
    write_bytes_atomic(path, bytes, true)
        .with_context(|| format!("Failed to write file: {}", path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .with_context(|| format!("Failed to set file permissions: {}", path.display()))?;
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
