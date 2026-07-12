//! Account switching logic - writes credentials to ~/.codex/auth.json

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;

use crate::auth::storage::write_file_atomically;
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

/// Switch to a specific account by writing its credentials to ~/.codex/auth.json.
///
/// All auth modes — including `CodexAccessToken` — are materialized by writing
/// the same `auth.json` the official Codex CLI stores after a successful login.
/// We write it directly (atomically) rather than shelling out to
/// `codex login --with-access-token`: that command gates login on a network
/// fetch of the agent-identity JWKS which can hang or fail, and the file it
/// ultimately writes is byte-for-byte what `create_auth_json` produces here.
pub fn switch_to_account(account: &StoredAccount) -> Result<()> {
    let codex_home = get_codex_home()?;
    write_account_auth_json(&codex_home, account)
}

/// Write an account's credentials to `<codex_home>/auth.json` atomically,
/// creating the codex home directory if it does not exist.
fn write_account_auth_json(codex_home: &Path, account: &StoredAccount) -> Result<()> {
    fs::create_dir_all(codex_home)
        .with_context(|| format!("Failed to create codex home: {}", codex_home.display()))?;

    let auth_json = create_auth_json(account)?;

    let auth_path = codex_home.join("auth.json");
    let content =
        serde_json::to_string_pretty(&auth_json).context("Failed to serialize auth.json")?;

    write_file_atomically(&auth_path, &content)
        .with_context(|| format!("Failed to write auth.json: {}", auth_path.display()))?;

    Ok(())
}

/// Create an AuthDotJson structure from a StoredAccount
fn create_auth_json(account: &StoredAccount) -> Result<AuthDotJson> {
    match &account.auth_data {
        AuthData::ApiKey { key } => Ok(AuthDotJson {
            auth_mode: None,
            openai_api_key: Some(key.clone()),
            tokens: None,
            last_refresh: None,
            agent_identity: None,
            personal_access_token: None,
        }),
        AuthData::ChatGPT {
            id_token,
            access_token,
            refresh_token,
            account_id,
        } => Ok(AuthDotJson {
            auth_mode: None,
            openai_api_key: None,
            tokens: Some(TokenData {
                id_token: id_token.clone(),
                access_token: access_token.clone(),
                refresh_token: refresh_token.clone(),
                account_id: account_id.clone(),
            }),
            last_refresh: Some(Utc::now()),
            agent_identity: None,
            personal_access_token: None,
        }),
        AuthData::CodexAccessToken { token, .. } => create_access_token_auth_json(token),
    }
}

fn create_access_token_auth_json(token: &str) -> Result<AuthDotJson> {
    let trimmed = token.trim();
    if trimmed.is_empty() {
        anyhow::bail!("Codex access token is empty");
    }

    if trimmed.starts_with("at-") {
        Ok(AuthDotJson {
            auth_mode: None,
            openai_api_key: None,
            tokens: None,
            last_refresh: None,
            agent_identity: None,
            personal_access_token: Some(trimmed.to_string()),
        })
    } else {
        Ok(AuthDotJson {
            auth_mode: Some("agentIdentity".to_string()),
            openai_api_key: None,
            tokens: None,
            last_refresh: None,
            agent_identity: Some(serde_json::Value::String(trimmed.to_string())),
            personal_access_token: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{create_access_token_auth_json, write_account_auth_json};
    use crate::types::StoredAccount;

    fn unique_codex_home(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "codex-switcher-switch-test-{}-{}",
            std::process::id(),
            label
        ));
        std::fs::remove_dir_all(&dir).ok();
        dir
    }

    #[test]
    fn switching_jwt_access_token_account_writes_agent_identity_auth_json() {
        // A successful `codex login --with-access-token` writes exactly this
        // file; we produce it directly instead of shelling out to the CLI (whose
        // login gates on an agent-identity JWKS fetch that can hang/fail).
        let token = ["header", "payload", "signature"].join(".");
        let account = StoredAccount::new_codex_access_token("K12".to_string(), token.clone());
        let codex_home = unique_codex_home("jwt");

        write_account_auth_json(&codex_home, &account).expect("switch should write auth.json");

        let written = std::fs::read_to_string(codex_home.join("auth.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&written).unwrap();
        std::fs::remove_dir_all(&codex_home).ok();

        assert_eq!(parsed["auth_mode"], "agentIdentity");
        assert_eq!(parsed["agent_identity"], token);
        assert!(parsed.get("tokens").is_none());
    }

    #[test]
    fn switching_personal_access_token_account_writes_personal_access_token() {
        let account =
            StoredAccount::new_codex_access_token("K12".to_string(), "at-secret-123".to_string());
        let codex_home = unique_codex_home("pat");

        write_account_auth_json(&codex_home, &account).expect("switch should write auth.json");

        let written = std::fs::read_to_string(codex_home.join("auth.json")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&written).unwrap();
        std::fs::remove_dir_all(&codex_home).ok();

        assert_eq!(parsed["personal_access_token"], "at-secret-123");
        assert!(parsed.get("auth_mode").is_none());
    }

    #[test]
    fn creates_agent_identity_auth_json_for_codex_access_token_jwt() {
        let sample_access_token = ["header", "payload", "signature"].join(".");
        let auth = create_access_token_auth_json(&sample_access_token).unwrap();

        assert_eq!(auth.auth_mode.as_deref(), Some("agentIdentity"));
        assert_eq!(
            auth.agent_identity
                .as_ref()
                .and_then(|value| value.as_str()),
            Some(sample_access_token.as_str())
        );
        assert!(auth.tokens.is_none());
        assert!(auth.personal_access_token.is_none());
    }

    #[test]
    fn rejects_empty_codex_access_token() {
        let error = create_access_token_auth_json(" \n\t ").unwrap_err();

        assert!(error.to_string().contains("access token is empty"));
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
    } else if let Some(agent_identity) = auth.agent_identity {
        let token = match agent_identity {
            serde_json::Value::String(token) => token,
            _ => anyhow::bail!("auth.json agent_identity has an unsupported shape"),
        };

        Ok(StoredAccount::new_codex_access_token(account_name, token))
    } else if let Some(personal_access_token) = auth.personal_access_token {
        Ok(StoredAccount::new_codex_access_token(
            account_name,
            personal_access_token,
        ))
    } else {
        anyhow::bail!("auth.json contains neither API key, tokens, nor access-token auth");
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
        Some(auth) => Ok(auth.openai_api_key.is_some()
            || auth.tokens.is_some()
            || auth.agent_identity.is_some()
            || auth.personal_access_token.is_some()),
        None => Ok(false),
    }
}
