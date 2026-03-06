use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use base64::Engine;
use rand::RngCore;

use crate::types::{AppSettings, ExportSecurityMode};

use super::storage::get_config_dir;

const KEYCHAIN_SERVICE: &str = "com.lampese.codex-switcher";
const KEYCHAIN_ACCOUNT: &str = "full-file-export-secret";

pub fn get_settings_file() -> Result<PathBuf> {
    Ok(get_config_dir()?.join("settings.json"))
}

pub fn load_settings() -> Result<AppSettings> {
    let path = get_settings_file()?;

    if !path.exists() {
        return Ok(AppSettings::default());
    }

    let content = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read settings file: {}", path.display()))?;

    let settings: AppSettings = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse settings file: {}", path.display()))?;

    Ok(settings)
}

pub fn save_settings(settings: &AppSettings) -> Result<()> {
    let path = get_settings_file()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create config directory: {}", parent.display()))?;
    }
    let content = serde_json::to_vec_pretty(settings).context("Failed to serialize settings")?;
    fs::write(&path, content)
        .with_context(|| format!("Failed to write settings file: {}", path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    }

    Ok(())
}

pub fn set_export_security_mode(mode: ExportSecurityMode) -> Result<AppSettings> {
    let mut settings = load_settings()?;
    settings.export_security_mode = Some(mode);
    save_settings(&settings)?;
    Ok(settings)
}

pub fn get_or_create_keychain_secret() -> Result<String> {
    let entry = keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT)
        .context("Failed to access OS keychain entry")?;

    if let Ok(secret) = entry.get_password() {
        if !secret.trim().is_empty() {
            return Ok(secret);
        }
    }

    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let secret = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);

    entry
        .set_password(&secret)
        .context("Failed to store backup secret in OS keychain")?;

    Ok(secret)
}

pub fn get_keychain_secret() -> Result<String> {
    let entry = keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT)
        .context("Failed to access OS keychain entry")?;
    let secret = entry
        .get_password()
        .context("No OS keychain backup secret has been created on this device yet")?;

    if secret.trim().is_empty() {
        anyhow::bail!("Stored OS keychain backup secret is empty");
    }

    Ok(secret)
}
