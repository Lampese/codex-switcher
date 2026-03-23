//! Account switching logic - writes credentials to ~/.codex/auth.json

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use chrono::Utc;

use crate::types::{AuthData, AuthDotJson, StoredAccount, TokenData};

/// Get the official Codex home directory
pub fn get_codex_home() -> Result<PathBuf> {
    // Check for CODEX_HOME environment variable first
    if let Ok(codex_home) = std::env::var("CODEX_HOME") {
        return Ok(PathBuf::from(codex_home));
    }

    let home = dirs::home_dir().context("Could not find home directory")?;
    Ok(home.join(".codex"))
}

/// Get the path to the official auth.json file
pub fn get_codex_auth_file() -> Result<PathBuf> {
    Ok(get_codex_home()?.join("auth.json"))
}

/// Switch to a specific account by writing its credentials to ~/.codex/auth.json
pub fn switch_to_account(account: &StoredAccount) -> Result<()> {
    let codex_home = get_codex_home()?;

    // Ensure the codex home directory exists
    fs::create_dir_all(&codex_home)
        .with_context(|| format!("Failed to create codex home: {}", codex_home.display()))?;

    let auth_json = create_auth_json(account)?;

    let auth_path = codex_home.join("auth.json");
    let content =
        serde_json::to_string_pretty(&auth_json).context("Failed to serialize auth.json")?;

    fs::write(&auth_path, content)
        .with_context(|| format!("Failed to write auth.json: {}", auth_path.display()))?;

    // Set restrictive permissions on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o600);
        fs::set_permissions(&auth_path, perms)?;
    }

    Ok(())
}

/// Create an AuthDotJson structure from a StoredAccount
fn create_auth_json(account: &StoredAccount) -> Result<AuthDotJson> {
    match &account.auth_data {
        AuthData::ApiKey { key } => Ok(AuthDotJson {
            openai_api_key: Some(key.clone()),
            tokens: None,
            last_refresh: None,
        }),
        AuthData::ChatGPT {
            id_token,
            access_token,
            refresh_token,
            account_id,
        } => Ok(AuthDotJson {
            openai_api_key: None,
            tokens: Some(TokenData {
                id_token: id_token.clone(),
                access_token: access_token.clone(),
                refresh_token: refresh_token.clone(),
                account_id: account_id.clone(),
            }),
            last_refresh: Some(Utc::now()),
        }),
    }
}

/// Import an account from an existing auth.json file
pub fn import_from_auth_json(path: &str, account_name: String) -> Result<StoredAccount> {
    let content =
        fs::read_to_string(path).with_context(|| format!("Failed to read auth.json: {path}"))?;

    let auth: AuthDotJson = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse auth.json: {path}"))?;

    // Determine auth mode and create account
    if let Some(api_key) = auth.openai_api_key {
        Ok(StoredAccount::new_api_key(account_name, api_key))
    } else if let Some(tokens) = auth.tokens {
        // Try to extract email and plan from id_token
        let (email, plan_type) = parse_id_token_claims(&tokens.id_token);

        Ok(StoredAccount::new_chatgpt(
            account_name,
            email,
            plan_type,
            tokens.id_token,
            tokens.access_token,
            tokens.refresh_token,
            tokens.account_id,
        ))
    } else {
        anyhow::bail!("auth.json contains neither API key nor tokens");
    }
}

/// Parse claims from a JWT ID token (without validation)
fn parse_id_token_claims(id_token: &str) -> (Option<String>, Option<String>) {
    let parts: Vec<&str> = id_token.split('.').collect();
    if parts.len() != 3 {
        return (None, None);
    }

    // Decode the payload (second part)
    let payload =
        match base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, parts[1]) {
            Ok(bytes) => bytes,
            Err(_) => return (None, None),
        };

    let json: serde_json::Value = match serde_json::from_slice(&payload) {
        Ok(v) => v,
        Err(_) => return (None, None),
    };

    let email = json.get("email").and_then(|v| v.as_str()).map(String::from);

    // Look for plan type in the OpenAI auth claims
    let plan_type = json
        .get("https://api.openai.com/auth")
        .and_then(|auth| auth.get("chatgpt_plan_type"))
        .and_then(|v| v.as_str())
        .map(String::from);

    (email, plan_type)
}

/// Read the current auth.json file if it exists
pub fn read_current_auth() -> Result<Option<AuthDotJson>> {
    let path = get_codex_auth_file()?;

    if !path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read auth.json: {}", path.display()))?;

    let auth: AuthDotJson = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse auth.json: {}", path.display()))?;

    Ok(Some(auth))
}

/// Check if there is an active Codex login
pub fn has_active_login() -> Result<bool> {
    match read_current_auth()? {
        Some(auth) => Ok(auth.openai_api_key.is_some() || auth.tokens.is_some()),
        None => Ok(false),
    }
}

// ============================================================================
// OpenCode auth.json support
// ============================================================================

/// Get the path to OpenCode's auth.json file
fn get_opencode_auth_file() -> Result<PathBuf> {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        // OpenCode uses ~/.local/share on macOS and Windows.
        // See: https://github.com/anomalyco/opencode/issues/5238
        let home = dirs::home_dir().context("Could not find home directory")?;
        Ok(home
            .join(".local")
            .join("share")
            .join("opencode")
            .join("auth.json"))
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        // On Linux: $XDG_DATA_HOME/opencode/auth.json (or ~/.local/share/...)
        let data_dir = dirs::data_dir().context("Could not find data directory")?;
        Ok(data_dir.join("opencode").join("auth.json"))
    }
}

/// Switch account in OpenCode's auth.json.
///
/// This function **merges** with the existing file content so that other
/// provider entries (e.g. `"groq"`) are preserved untouched.  Only the
/// `"openai"` key is updated.
pub fn switch_to_opencode(account: &StoredAccount) -> Result<()> {
    let auth_path = get_opencode_auth_file()?;

    // 1. Read existing file into a generic JSON map to preserve other keys
    let mut auth_map: serde_json::Map<String, serde_json::Value> = if auth_path.exists() {
        let content = fs::read_to_string(&auth_path).with_context(|| {
            format!("Failed to read OpenCode auth.json: {}", auth_path.display())
        })?;
        serde_json::from_str(&content).unwrap_or_default()
    } else {
        serde_json::Map::new()
    };

    // 2. Build the new "openai" provider value from account credentials
    let openai_value = match &account.auth_data {
        AuthData::ChatGPT {
            access_token,
            refresh_token,
            account_id,
            ..
        } => {
            serde_json::json!({
                "type": "oauth",
                "refresh": refresh_token,
                "access": access_token,
                "expires": opencode_expires_timestamp(),
                "accountId": account_id.clone().unwrap_or_default(),
            })
        }
        AuthData::ApiKey { key } => {
            serde_json::json!({
                "type": "api",
                "key": key,
            })
        }
    };

    // 3. Replace only the "openai" key, leaving everything else intact
    auth_map.insert("openai".to_string(), openai_value);

    // 4. Write back the full merged content
    if let Some(parent) = auth_path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!("Failed to create OpenCode config dir: {}", parent.display())
        })?;
    }

    let content = serde_json::to_string_pretty(&auth_map)
        .context("Failed to serialize OpenCode auth.json")?;

    fs::write(&auth_path, &content).with_context(|| {
        format!(
            "Failed to write OpenCode auth.json: {}",
            auth_path.display()
        )
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o600);
        fs::set_permissions(&auth_path, perms)?;
    }

    Ok(())
}

/// Calculate an expiry timestamp ~7 days from now, in milliseconds (OpenCode format).
fn opencode_expires_timestamp() -> i64 {
    (Utc::now() + chrono::Duration::days(7)).timestamp_millis()
}
