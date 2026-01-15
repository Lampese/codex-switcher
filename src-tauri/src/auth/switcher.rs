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
