//! Account switching logic - writes credentials to ~/.codex/auth.json

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;

use crate::types::{
    parse_chatgpt_id_token_claims, AuthData, AuthDotJson, StoredAccount, TokenData,
};

/// Get the default Codex home directory, ignoring Codex Switcher instances.
pub fn get_default_codex_home() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not find home directory")?;
    Ok(home.join(".codex"))
}

fn resolve_codex_home(
    codex_home_env: Option<PathBuf>,
    active_instance_dir: Option<PathBuf>,
    user_home: &Path,
) -> PathBuf {
    codex_home_env
        .or(active_instance_dir)
        .unwrap_or_else(|| user_home.join(".codex"))
}

/// Get the effective Codex home directory.
pub fn get_codex_home() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not find home directory")?;
    let codex_home_env = std::env::var("CODEX_HOME").ok().map(PathBuf::from);
    let active_instance_dir = super::instance_manager::active_instance_data_dir()?;

    Ok(resolve_codex_home(
        codex_home_env,
        active_instance_dir,
        &home,
    ))
}

/// Get the path to the official auth.json file
pub fn get_codex_auth_file() -> Result<PathBuf> {
    Ok(get_codex_home()?.join("auth.json"))
}

/// Remove the effective Codex auth.json file if it exists.
pub fn clear_codex_auth() -> Result<()> {
    let auth_path = get_codex_auth_file()?;
    clear_codex_auth_file_at(&auth_path)
}

fn clear_codex_auth_file_at(auth_path: &Path) -> Result<()> {
    if auth_path.exists() {
        fs::remove_file(auth_path)
            .with_context(|| format!("Failed to remove auth.json: {}", auth_path.display()))?;
    }

    Ok(())
}

/// Switch to a specific account by writing its credentials to ~/.codex/auth.json
pub fn switch_to_account(account: &StoredAccount) -> Result<()> {
    let codex_home = get_codex_home()?;
    switch_to_account_in_dir(account, &codex_home)
}

/// Switch to a specific account by writing its credentials to a Codex home directory.
pub fn switch_to_account_in_dir(account: &StoredAccount, codex_home: &Path) -> Result<()> {
    // Ensure the codex home directory exists
    fs::create_dir_all(&codex_home)
        .with_context(|| format!("Failed to create codex home: {}", codex_home.display()))?;

    let auth_json = create_auth_json(account)?;

    let auth_path = codex_home.join("auth.json");
    let content =
        serde_json::to_string_pretty(&auth_json).context("Failed to serialize auth.json")?;

    // Atomic write: temp file → rename, with auto .bak backup
    super::atomic_write::write_string_atomic(&auth_path, &content)
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

#[cfg(test)]
mod tests {
    use super::{clear_codex_auth_file_at, resolve_codex_home, switch_to_account_in_dir};
    use crate::types::{AuthDotJson, StoredAccount};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn resolve_codex_home_prefers_env_then_active_instance_then_default_home() {
        let env_home = PathBuf::from("C:/codex/env");
        let active_instance = PathBuf::from("C:/codex/instance");
        let user_home = PathBuf::from("C:/Users/example");

        assert_eq!(
            resolve_codex_home(
                Some(env_home.clone()),
                Some(active_instance.clone()),
                &user_home
            ),
            env_home
        );
        assert_eq!(
            resolve_codex_home(None, Some(active_instance.clone()), &user_home),
            active_instance
        );
        assert_eq!(
            resolve_codex_home(None, None, &user_home),
            user_home.join(".codex")
        );
    }

    #[test]
    fn switch_to_account_in_dir_writes_auth_json_to_selected_instance_dir() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after unix epoch")
            .as_nanos();
        let codex_home = std::env::temp_dir().join(format!(
            "codex-switcher-instance-auth-test-{}-{unique}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&codex_home);

        let account = StoredAccount::new_api_key("work".to_string(), "sk-test".to_string());
        switch_to_account_in_dir(&account, &codex_home).expect("account auth should be written");

        let auth_path = codex_home.join("auth.json");
        let auth: AuthDotJson =
            serde_json::from_str(&fs::read_to_string(&auth_path).expect("auth.json should exist"))
                .expect("auth.json should parse");

        assert_eq!(auth.openai_api_key.as_deref(), Some("sk-test"));
        assert!(auth.tokens.is_none());

        fs::remove_dir_all(&codex_home).expect("test temp dir should be removable");
    }

    #[test]
    fn clear_codex_auth_file_removes_existing_auth_json() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after unix epoch")
            .as_nanos();
        let codex_home = std::env::temp_dir().join(format!(
            "codex-switcher-clear-auth-test-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&codex_home).expect("test temp dir should be created");
        let auth_path = codex_home.join("auth.json");
        fs::write(&auth_path, "{}").expect("auth.json should be written");

        clear_codex_auth_file_at(&auth_path).expect("auth.json should be cleared");

        assert!(!auth_path.exists());
        fs::remove_dir_all(&codex_home).expect("test temp dir should be removable");
    }
}
