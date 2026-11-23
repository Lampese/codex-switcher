//! Account switching logic - writes credentials to ~/.codex/auth.json

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;

use crate::types::{
    parse_chatgpt_id_token_claims, AuthData, AuthDotJson, AuthMode, StoredAccount, TokenData,
};

/// Get the official Codex home directory
pub fn get_codex_home() -> Result<PathBuf> {
    #[cfg(test)]
    {
        Ok(super::test_support::home_dir()?.join(".codex"))
    }

    #[cfg(not(test))]
    {
        // Check for CODEX_HOME environment variable first
        if let Ok(codex_home) = std::env::var("CODEX_HOME") {
            return Ok(PathBuf::from(codex_home));
        }

        // Windows users can point the app at their WSL Codex install (#55).
        if let Some(wsl_home) = crate::wsl::codex_home_override() {
            return Ok(wsl_home);
        }

        let home = dirs::home_dir().context("Could not find home directory")?;
        Ok(home.join(".codex"))
    }
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

    // Keep Codex's preferred auth method aligned with the credentials we just
    // wrote so an API-key account is not used under a leftover "chatgpt"
    // preference or vice versa (#64). A missing or unparsable config.toml is
    // non-fatal for the switch.
    if let Err(error) = sync_config_auth_method(&codex_home, account) {
        eprintln!(
            "Failed to update preferred_auth_method in config.toml: {error}"
        );
    }

    Ok(())
}

/// The `preferred_auth_method` value Codex expects for each account kind.
fn preferred_auth_method(account: &StoredAccount) -> &'static str {
    match account.auth_mode {
        AuthMode::ApiKey => "apikey",
        AuthMode::ChatGPT => "chatgpt",
    }
}

/// Write the account's auth method into `~/.codex/config.toml`, preserving
/// every other setting the user has configured.
fn sync_config_auth_method(codex_home: &Path, account: &StoredAccount) -> Result<()> {
    let config_path = codex_home.join("config.toml");
    let existing = match fs::read_to_string(&config_path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(error).with_context(|| {
                format!("Failed to read config.toml: {}", config_path.display())
            })
        }
    };

    let updated = set_preferred_auth_method(&existing, preferred_auth_method(account));
    if updated == existing {
        return Ok(());
    }

    fs::write(&config_path, updated)
        .with_context(|| format!("Failed to write config.toml: {}", config_path.display()))
}

/// Set the root-level `preferred_auth_method` key in TOML content: replace an
/// existing root assignment, insert before the first table, or append when the
/// file has no tables. Comments and assignments inside `[table]` sections are
/// left untouched.
fn set_preferred_auth_method(content: &str, method: &str) -> String {
    let assignment = format!("preferred_auth_method = \"{method}\"");
    let mut result = String::with_capacity(content.len() + assignment.len() + 1);
    let mut done = false;

    for line in content.lines() {
        let trimmed = line.trim();
        let is_table_header = trimmed.starts_with('[');

        if !done && !is_table_header {
            if let Some((key, _)) = trimmed.split_once('=') {
                if key.trim() == "preferred_auth_method" {
                    result.push_str(&assignment);
                    result.push('\n');
                    done = true;
                    continue;
                }
            }
        }
        if !done && is_table_header {
            // Root section ended without the key; insert before the table.
            result.push_str(&assignment);
            result.push('\n');
            done = true;
        }

        result.push_str(line);
        result.push('\n');
    }

    if !done {
        result.push_str(&assignment);
        result.push('\n');
    }

    result
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
    let account_name = account_name.trim().to_string();

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
    use super::{import_from_auth_json_contents, set_preferred_auth_method};
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use serde_json::json;

    fn auth_json(payload: serde_json::Value, account_id: &str) -> String {
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
        serde_json::json!({
            "tokens": {
                "id_token": format!("header.{payload}.signature"),
                "access_token": "access",
                "refresh_token": "refresh",
                "account_id": account_id
            }
        })
        .to_string()
    }

    #[test]
    fn import_blank_name_uses_email() {
        let account = import_from_auth_json_contents(
            &auth_json(json!({"email": "imported@example.com"}), "acct-import"),
            "".into(),
        )
        .unwrap();
        assert_eq!(account.name, "imported@example.com");
    }

    #[test]
    fn import_explicit_name_is_trimmed() {
        let account = import_from_auth_json_contents(
            &auth_json(json!({"email": "imported@example.com"}), "acct-import"),
            "  Imported Account  ".into(),
        )
        .unwrap();
        assert_eq!(account.name, "Imported Account");
    }

    #[test]
    fn import_without_email_uses_account_id_fallback() {
        let account =
            import_from_auth_json_contents(&auth_json(json!({}), "acct-87654321"), "".into())
                .unwrap();
        assert_eq!(account.name, "ChatGPT account (87654321)");
    }

    #[test]
    fn preferred_auth_method_replaces_root_level_assignment() {
        let updated = set_preferred_auth_method(
            "model = \"gpt-5\"\npreferred_auth_method = \"chatgpt\"\nnotify = true\n",
            "apikey",
        );
        assert_eq!(
            updated,
            "model = \"gpt-5\"\npreferred_auth_method = \"apikey\"\nnotify = true\n"
        );
    }

    #[test]
    fn preferred_auth_method_inserts_before_first_table() {
        let updated =
            set_preferred_auth_method("model = \"gpt-5\"\n\n[tui]\ntheme = \"dark\"\n", "apikey");
        assert_eq!(
            updated,
            "model = \"gpt-5\"\n\npreferred_auth_method = \"apikey\"\n[tui]\ntheme = \"dark\"\n"
        );
    }

    #[test]
    fn preferred_auth_method_appends_to_table_free_file() {
        let updated = set_preferred_auth_method("model = \"gpt-5\"\n", "chatgpt");
        assert_eq!(
            updated,
            "model = \"gpt-5\"\npreferred_auth_method = \"chatgpt\"\n"
        );
    }

    #[test]
    fn preferred_auth_method_works_on_empty_config() {
        assert_eq!(
            set_preferred_auth_method("", "apikey"),
            "preferred_auth_method = \"apikey\"\n"
        );
    }

    #[test]
    fn preferred_auth_method_ignores_keys_inside_tables() {
        // A key inside a [table] is not the root setting and must be left alone.
        let updated = set_preferred_auth_method(
            "[profiles.work]\npreferred_auth_method = \"chatgpt\"\n",
            "apikey",
        );
        assert_eq!(
            updated,
            "preferred_auth_method = \"apikey\"\n[profiles.work]\npreferred_auth_method = \"chatgpt\"\n"
        );
    }

    #[test]
    fn preferred_auth_method_ignores_commented_assignments() {
        let updated = set_preferred_auth_method(
            "# preferred_auth_method = \"chatgpt\"\nmodel = \"gpt-5\"\n",
            "apikey",
        );
        assert_eq!(
            updated,
            "# preferred_auth_method = \"chatgpt\"\nmodel = \"gpt-5\"\npreferred_auth_method = \"apikey\"\n"
        );
    }
}
