//! Account switching logic - writes credentials to ~/.codex/auth.json

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use chrono::Utc;

use crate::types::{
    parse_chatgpt_id_token_claims, AuthData, AuthDotJson, StoredAccount, TokenData,
};

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
    // Before writing, check if ~/.codex/auth.json already has fresher tokens
    // for this same account (Codex rotates tokens during normal use). If so,
    // absorb those tokens into accounts.json first so we never write stale
    // credentials and trigger a refresh_token_reused error.
    let account = &sync_tokens_from_auth_json(account)?;

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

/// If ~/.codex/auth.json holds a newer token set for the same ChatGPT account,
/// update accounts.json with those tokens before we proceed. This prevents
/// writing an old refresh token back and causing `refresh_token_reused`.
fn sync_tokens_from_auth_json(account: &StoredAccount) -> Result<StoredAccount> {
    use crate::auth::storage::update_account_chatgpt_tokens;
    use crate::types::parse_chatgpt_id_token_claims;

    // Only relevant for ChatGPT OAuth accounts.
    let (stored_refresh_token, stored_account_id) = match &account.auth_data {
        AuthData::ChatGPT {
            refresh_token,
            account_id,
            ..
        } => (refresh_token.clone(), account_id.clone()),
        AuthData::ApiKey { .. } => return Ok(account.clone()),
    };

    let auth_file = match read_current_auth()? {
        Some(auth) => auth,
        None => return Ok(account.clone()),
    };

    let disk_tokens = match auth_file.tokens {
        Some(tokens) => tokens,
        None => return Ok(account.clone()),
    };

    // Only sync if the auth.json belongs to the same ChatGPT account.
    let disk_account_id = disk_tokens
        .account_id
        .clone()
        .or_else(|| parse_chatgpt_id_token_claims(&disk_tokens.id_token).account_id);

    let same_account = match (&stored_account_id, &disk_account_id) {
        (Some(stored), Some(disk)) => stored == disk,
        // No account_id on either side — compare refresh tokens as a fallback.
        _ => disk_tokens.refresh_token == stored_refresh_token,
    };

    if !same_account {
        return Ok(account.clone());
    }

    // Tokens differ — auth.json is newer (Codex rotated them). Absorb them.
    if disk_tokens.refresh_token == stored_refresh_token
        && disk_tokens.access_token
            == match &account.auth_data {
                AuthData::ChatGPT { access_token, .. } => access_token.clone(),
                _ => String::new(),
            }
    {
        // Tokens are identical — nothing to sync.
        return Ok(account.clone());
    }

    println!(
        "[Auth] auth.json has fresher tokens for account '{}', syncing into accounts.json before switch",
        account.name
    );

    let claims = parse_chatgpt_id_token_claims(&disk_tokens.id_token);
    let updated = update_account_chatgpt_tokens(
        &account.id,
        disk_tokens.id_token,
        disk_tokens.access_token,
        disk_tokens.refresh_token,
        disk_account_id,
        claims.email,
        claims.plan_type,
        claims.subscription_expires_at,
    )?;

    Ok(updated)
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

    import_from_auth_json_contents(&content, account_name)
        .with_context(|| format!("Failed to parse auth.json: {path}"))
}

/// Import an account from auth.json file contents.
pub fn import_from_auth_json_contents(
    content: &str,
    account_name: String,
) -> Result<StoredAccount> {
    let auth: AuthDotJson =
        serde_json::from_str(&content).context("Failed to parse auth.json contents")?;

    // Determine auth mode and create account
    if let Some(api_key) = auth.openai_api_key {
        Ok(StoredAccount::new_api_key(account_name, api_key))
    } else if let Some(tokens) = auth.tokens {
        let claims = parse_chatgpt_id_token_claims(&tokens.id_token);

        Ok(StoredAccount::new_chatgpt(
            account_name,
            claims.email,
            claims.plan_type,
            claims.subscription_expires_at,
            tokens.id_token,
            tokens.access_token,
            tokens.refresh_token,
            claims.account_id.or(tokens.account_id),
        ))
    } else {
        anyhow::bail!("auth.json contains neither API key nor tokens");
    }
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
