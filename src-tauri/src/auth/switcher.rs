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

/// Switch credentials and provider configuration together, rolling back on I/O errors.
pub fn switch_to_account(account: &StoredAccount) -> Result<()> {
    switch_in_home(&get_codex_home()?, account)
}

/// Refresh credentials without changing the active provider selection.
pub fn write_account_credentials(account: &StoredAccount) -> Result<()> {
    let home = get_codex_home()?;
    fs::create_dir_all(&home)?;
    atomic_write(
        &home.join("auth.json"),
        Some(&serde_json::to_vec_pretty(&create_auth_json(account)?)?),
    )
}

fn read_optional(path: &std::path::Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("Could not read {}", path.display())),
    }
}

/// Replacing a symlink would silently disconnect a user-managed credential/config file.
fn reject_symlink(path: &std::path::Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => anyhow::ensure!(
            !metadata.file_type().is_symlink(),
            "Refusing to modify symlink: {}",
            path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => {
            return Err(error).with_context(|| format!("Could not inspect {}", path.display()))
        }
    }
    Ok(())
}

fn atomic_write(path: &std::path::Path, content: Option<&[u8]>) -> Result<()> {
    use std::io::Write;
    reject_symlink(path)?;
    if let Some(content) = content {
        let mut temp =
            tempfile::NamedTempFile::new_in(path.parent().context("Missing parent directory")?)?;
        // NamedTempFile is created with 0600 on Unix, before secrets are written.
        temp.write_all(content)?;
        temp.as_file().sync_all()?;
        temp.persist(path)
            .map_err(|error| error.error)
            .with_context(|| format!("Could not replace {}", path.display()))?;
    } else {
        match fs::remove_file(path) {
            Ok(()) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn switch_in_home(home: &std::path::Path, account: &StoredAccount) -> Result<()> {
    switch_in_home_with_writer(home, account, atomic_write)
}

fn switch_in_home_with_writer(
    home: &std::path::Path,
    account: &StoredAccount,
    mut write: impl FnMut(&std::path::Path, Option<&[u8]>) -> Result<()>,
) -> Result<()> {
    account.validate_custom_provider()?;
    let auth = serde_json::to_vec_pretty(&create_auth_json(account)?)?;
    fs::create_dir_all(home)?;
    let auth_path = home.join("auth.json");
    let config_path = home.join("config.toml");
    let snapshot_path = home.join(super::provider_config::SNAPSHOT_FILE);
    reject_symlink(&auth_path)?;
    reject_symlink(&snapshot_path)?;
    let old_auth = read_optional(&auth_path)?;
    let old_snapshot = read_optional(&snapshot_path)?;
    // Ordinary switching does not parse or read config unless restoration is needed.
    if account.custom_provider.is_none() && old_snapshot.is_none() {
        return write(&auth_path, Some(&auth));
    }
    reject_symlink(&config_path)?;
    let old_config = read_optional(&config_path)?;
    let (config, snapshot) = super::provider_config::prepare(
        old_config.as_deref(),
        old_snapshot.as_deref(),
        account.custom_provider.as_ref(),
    )?
    .context("Missing provider changes")?;
    let changes = [
        (&snapshot_path, snapshot.as_deref(), old_snapshot.as_deref()),
        (&config_path, config.as_deref(), old_config.as_deref()),
        (&auth_path, Some(auth.as_slice()), old_auth.as_deref()),
    ];
    for (index, (path, new, _)) in changes.iter().enumerate() {
        if let Err(error) = write(path, *new) {
            let mut failures = Vec::new();
            for (path, _, old) in changes[..index].iter().rev() {
                if let Err(rollback) = atomic_write(path, *old) {
                    failures.push(rollback.to_string());
                }
            }
            if !failures.is_empty() {
                anyhow::bail!(
                    "Account switch failed: {error}; rollback failed: {}",
                    failures.join("; ")
                );
            }
            return Err(error.context("Account switch failed; previous files restored"));
        }
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
    use super::import_from_auth_json_contents;
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
}

#[cfg(test)]
mod provider_switch_tests {
    use super::*;
    use crate::types::CustomProvider;
    fn gateway(name: &str) -> StoredAccount {
        let mut account = StoredAccount::new_api_key(name.into(), format!("test-{name}"));
        account.custom_provider = Some(CustomProvider {
            name: name.into(),
            base_url: format!("https://{name}.example/v1"),
            model: format!("model-{name}"),
        });
        account
    }
    #[test]
    fn normal_custom_custom_normal_preserves_settings_and_restores_selection() {
        let home = tempfile::tempdir().unwrap();
        let config = home.path().join("config.toml");
        fs::write(&config, "# My configuration\nmodel = 'original' # chosen model\nmodel_provider = 'other'\nreview_model = 'review'\nprofile = 'work'\nopenai_base_url = 'https://original.example'\ncli_auth_credentials_store = 'keyring'\n[model_providers.other]\nname = 'Existing'\n[profiles.work]\nmodel = 'profile-model'\n").unwrap();
        let normal = StoredAccount::new_chatgpt(
            "Normal".into(),
            Some("fake@example.com".into()),
            None,
            None,
            "fake-id-token".into(),
            "fake-access-token".into(),
            "fake-refresh-token".into(),
            Some("fake-account".into()),
        );
        switch_in_home(home.path(), &normal).unwrap();
        let original = fs::read_to_string(&config).unwrap();
        switch_in_home(home.path(), &gateway("one")).unwrap();
        let first = fs::read_to_string(&config)
            .unwrap()
            .parse::<toml_edit::DocumentMut>()
            .unwrap();
        assert_eq!(first["model"].as_str(), Some("model-one"));
        assert_eq!(first["cli_auth_credentials_store"].as_str(), Some("file"));
        assert!(first.get("profile").is_none());
        assert!(first.get("review_model").is_none());
        assert!(first.get("openai_base_url").is_none());
        let id = first["model_provider"].as_str().unwrap().to_owned();
        assert_eq!(
            first["model_providers"][&id]["requires_openai_auth"].as_bool(),
            Some(true)
        );
        let snapshot = fs::read(
            home.path()
                .join(super::super::provider_config::SNAPSHOT_FILE),
        )
        .unwrap();
        switch_in_home(home.path(), &gateway("two")).unwrap();
        let before: serde_json::Value = serde_json::from_slice(&snapshot).unwrap();
        let after: serde_json::Value = serde_json::from_slice(
            &fs::read(
                home.path()
                    .join(super::super::provider_config::SNAPSHOT_FILE),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(before["original_config"], after["original_config"]);
        let mut second = fs::read_to_string(&config)
            .unwrap()
            .parse::<toml_edit::DocumentMut>()
            .unwrap();
        let second_id = second["model_provider"].as_str().unwrap().to_owned();
        assert_ne!(second_id, id);
        assert!(second["model_providers"]
            .as_table()
            .unwrap()
            .get(&id)
            .is_none());
        assert_eq!(
            second["model_providers"][&second_id]["base_url"].as_str(),
            Some("https://two.example/v1")
        );
        second["added_setting"] = toml_edit::value(true);
        fs::write(&config, second.to_string()).unwrap();
        switch_in_home(home.path(), &normal).unwrap();
        let restored_text = fs::read_to_string(&config).unwrap();
        let restored = restored_text.parse::<toml_edit::DocumentMut>().unwrap();
        let original = original.parse::<toml_edit::DocumentMut>().unwrap();
        for key in [
            "model",
            "model_provider",
            "review_model",
            "profile",
            "openai_base_url",
        ] {
            assert_eq!(restored[key].to_string(), original[key].to_string());
        }
        assert_eq!(
            restored["cli_auth_credentials_store"].as_str(),
            Some("file")
        );
        assert!(restored_text.contains("# chosen model"));
        assert!(restored_text.contains("# My configuration"));
        assert_eq!(restored["added_setting"].as_bool(), Some(true));
        assert!(restored["model_providers"]
            .as_table()
            .unwrap()
            .get(&id)
            .is_none());
        assert!(!home
            .path()
            .join(super::super::provider_config::SNAPSHOT_FILE)
            .exists());
        let auth: AuthDotJson =
            serde_json::from_slice(&fs::read(home.path().join("auth.json")).unwrap()).unwrap();
        assert!(auth.openai_api_key.is_none());
        let tokens = auth.tokens.unwrap();
        assert_eq!(tokens.id_token, "fake-id-token");
        assert_eq!(tokens.access_token, "fake-access-token");
        assert_eq!(tokens.refresh_token, "fake-refresh-token");
        assert_eq!(tokens.account_id.as_deref(), Some("fake-account"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(home.path().join("auth.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
    #[cfg(unix)]
    #[test]
    fn symlink_auth_is_refused_for_auth_only_and_direct_writes() {
        let home = tempfile::tempdir().unwrap();
        let target = home.path().join("managed-auth.json");
        let link = home.path().join("auth.json");
        fs::write(&target, "original credentials").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let account = StoredAccount::new_api_key("Normal".into(), "fake-key".into());
        assert!(switch_in_home(home.path(), &account)
            .unwrap_err()
            .to_string()
            .contains("symlink"));
        assert!(atomic_write(&link, Some(b"replacement")).is_err());
        assert!(atomic_write(&link, None).is_err());
        assert_eq!(fs::read_link(&link).unwrap(), target);
        assert_eq!(fs::read_to_string(&target).unwrap(), "original credentials");
    }

    #[cfg(unix)]
    #[test]
    fn symlink_config_refusal_preserves_active_snapshot_and_credentials() {
        let home = tempfile::tempdir().unwrap();
        switch_in_home(home.path(), &gateway("one")).unwrap();
        let config = home.path().join("config.toml");
        let target = home.path().join("managed-config.toml");
        fs::rename(&config, &target).unwrap();
        std::os::unix::fs::symlink(&target, &config).unwrap();
        let previous_config = fs::read(&target).unwrap();
        let previous_auth = fs::read(home.path().join("auth.json")).unwrap();
        let snapshot_path = home
            .path()
            .join(super::super::provider_config::SNAPSHOT_FILE);
        let previous_snapshot = fs::read(&snapshot_path).unwrap();
        assert!(switch_in_home(home.path(), &gateway("two"))
            .unwrap_err()
            .to_string()
            .contains("symlink"));
        assert_eq!(fs::read_link(&config).unwrap(), target);
        assert_eq!(fs::read(&target).unwrap(), previous_config);
        assert_eq!(
            fs::read(home.path().join("auth.json")).unwrap(),
            previous_auth
        );
        assert_eq!(fs::read(snapshot_path).unwrap(), previous_snapshot);
    }

    #[test]
    fn invalid_config_does_not_change_credentials() {
        let home = tempfile::tempdir().unwrap();
        fs::write(home.path().join("auth.json"), "original credentials").unwrap();
        fs::write(home.path().join("config.toml"), "invalid = [").unwrap();
        assert!(switch_in_home(home.path(), &gateway("one")).is_err());
        assert_eq!(
            fs::read_to_string(home.path().join("auth.json")).unwrap(),
            "original credentials"
        );
        assert!(!home
            .path()
            .join(super::super::provider_config::SNAPSHOT_FILE)
            .exists());
        // Ordinary account switching does not interpret unrelated config.
        switch_in_home(
            home.path(),
            &StoredAccount::new_api_key("Normal".into(), "normal-key".into()),
        )
        .unwrap();
    }
    #[test]
    fn auth_write_failure_rolls_back_config_and_snapshot() {
        let home = tempfile::tempdir().unwrap();
        fs::write(home.path().join("auth.json"), "old-auth").unwrap();
        fs::write(home.path().join("config.toml"), "model = 'original'\n").unwrap();
        let result = switch_in_home_with_writer(home.path(), &gateway("one"), |path, data| {
            if path.ends_with("auth.json") {
                anyhow::bail!("injected auth write failure");
            }
            atomic_write(path, data)
        });
        assert!(result.is_err());
        assert_eq!(
            fs::read_to_string(home.path().join("config.toml")).unwrap(),
            "model = 'original'\n"
        );
        assert_eq!(
            fs::read_to_string(home.path().join("auth.json")).unwrap(),
            "old-auth"
        );
        assert!(!home
            .path()
            .join(super::super::provider_config::SNAPSHOT_FILE)
            .exists());
    }
    #[test]
    fn actual_config_write_failure_rolls_back_snapshot() {
        let home = tempfile::tempdir().unwrap();
        fs::write(home.path().join("auth.json"), "old-auth").unwrap();
        // An actual failed replacement, after the snapshot was successfully persisted.
        let result = switch_in_home_with_writer(home.path(), &gateway("one"), |path, data| {
            if path.ends_with("config.toml") {
                fs::create_dir(path)?;
            }
            atomic_write(path, data)
        });
        assert!(result.is_err());
        assert_eq!(
            fs::read_to_string(home.path().join("auth.json")).unwrap(),
            "old-auth"
        );
        assert!(!home
            .path()
            .join(super::super::provider_config::SNAPSHOT_FILE)
            .exists());
    }
    #[test]
    fn missing_config_retains_file_credentials_after_restoration() {
        let home = tempfile::tempdir().unwrap();
        switch_in_home(home.path(), &gateway("one")).unwrap();
        switch_in_home(
            home.path(),
            &StoredAccount::new_api_key("Normal".into(), "normal".into()),
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(home.path().join("config.toml"))
                .unwrap()
                .parse::<toml_edit::DocumentMut>()
                .unwrap()["cli_auth_credentials_store"]
                .as_str(),
            Some("file")
        );
    }
}
