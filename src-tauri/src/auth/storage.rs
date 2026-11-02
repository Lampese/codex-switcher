//! Account storage module - manages reading and writing accounts.json

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};

use crate::types::{
    parse_chatgpt_id_token_claims, AccountsStore, AppSettings, AuthData, AuthDotJson, StoredAccount,
};

pub fn sync_active_account_tokens(store: &mut AccountsStore, auth: &AuthDotJson) -> bool {
    let Some(active_id) = store.active_account_id.as_deref() else {
        return false;
    };
    let Some(tokens) = auth.tokens.as_ref() else {
        return false;
    };
    let Some(account) = store
        .accounts
        .iter_mut()
        .find(|account| account.id == active_id)
    else {
        return false;
    };
    let AuthData::ChatGPT {
        id_token,
        access_token,
        refresh_token,
        account_id,
    } = &mut account.auth_data
    else {
        return false;
    };

    let stored_account_id = parse_chatgpt_id_token_claims(id_token)
        .account_id
        .or_else(|| account_id.clone());
    let current_account_id = parse_chatgpt_id_token_claims(&tokens.id_token)
        .account_id
        .or_else(|| tokens.account_id.clone());
    let (Some(stored_account_id), Some(current_account_id)) =
        (stored_account_id, current_account_id)
    else {
        return false;
    };
    if stored_account_id != current_account_id {
        return false;
    }

    let changed = *id_token != tokens.id_token
        || *access_token != tokens.access_token
        || *refresh_token != tokens.refresh_token
        || account_id.as_ref() != Some(&current_account_id);
    if !changed {
        return false;
    }

    id_token.clone_from(&tokens.id_token);
    access_token.clone_from(&tokens.access_token);
    refresh_token.clone_from(&tokens.refresh_token);
    *account_id = Some(current_account_id);
    true
}

/// Get the path to the codex-switcher config directory
pub fn get_config_dir() -> Result<PathBuf> {
    #[cfg(test)]
    {
        Ok(super::test_support::home_dir()?.join(".codex-switcher"))
    }

    #[cfg(not(test))]
    {
        let home = dirs::home_dir().context("Could not find home directory")?;
        Ok(home.join(".codex-switcher"))
    }
}

/// Get the path to accounts.json
pub fn get_accounts_file() -> Result<PathBuf> {
    Ok(get_config_dir()?.join("accounts.json"))
}

pub fn get_settings_file() -> Result<PathBuf> {
    Ok(get_config_dir()?.join("settings.json"))
}

/// Load the accounts store from disk
pub fn load_accounts() -> Result<AccountsStore> {
    let path = get_accounts_file()?;

    if !path.exists() {
        return Ok(AccountsStore::default());
    }

    let content = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read accounts file: {}", path.display()))?;

    let store: AccountsStore = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse accounts file: {}", path.display()))?;

    Ok(store)
}

pub fn load_app_settings() -> Result<AppSettings> {
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

pub fn save_app_settings(settings: &AppSettings) -> Result<()> {
    let path = get_settings_file()?;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create config directory: {}", parent.display()))?;
    }

    let content = serde_json::to_string_pretty(settings).context("Failed to serialize settings")?;
    fs::write(&path, content)
        .with_context(|| format!("Failed to write settings file: {}", path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o600);
        fs::set_permissions(&path, perms)?;
    }

    Ok(())
}

/// Save the accounts store to disk
pub fn save_accounts(store: &AccountsStore) -> Result<()> {
    let path = get_accounts_file()?;

    // Ensure the config directory exists
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create config directory: {}", parent.display()))?;
    }

    let content =
        serde_json::to_string_pretty(store).context("Failed to serialize accounts store")?;

    fs::write(&path, content)
        .with_context(|| format!("Failed to write accounts file: {}", path.display()))?;

    // Set restrictive permissions on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o600);
        fs::set_permissions(&path, perms)?;
    }

    Ok(())
}

/// Add a new account to the store
pub fn add_account(account: StoredAccount) -> Result<StoredAccount> {
    let mut store = load_accounts()?;

    let stored = add_account_to_store(&mut store, account)?;
    save_accounts(&store)?;
    Ok(stored)
}

/// A ChatGPT workspace is identified by its workspace ID, not its email,
/// plan or user-editable display name. Prefer the current ID token claim.
fn chatgpt_workspace_id(account: &StoredAccount) -> Option<String> {
    let AuthData::ChatGPT {
        id_token,
        account_id,
        ..
    } = &account.auth_data
    else {
        return None;
    };

    parse_chatgpt_id_token_claims(id_token)
        .account_id
        .or_else(|| account_id.clone())
        .filter(|id| !id.trim().is_empty())
}

fn add_account_to_store(
    store: &mut AccountsStore,
    mut account: StoredAccount,
) -> Result<StoredAccount> {
    // Do not store the same known workspace twice, even if the user assigns a
    // different name or its subscription plan changes.
    let workspace_id = chatgpt_workspace_id(&account);
    if let Some(ref workspace_id) = workspace_id {
        if store
            .accounts
            .iter()
            .any(|saved| chatgpt_workspace_id(saved).as_ref() == Some(workspace_id))
        {
            anyhow::bail!("This ChatGPT workspace is already saved");
        }
    }

    if store
        .accounts
        .iter()
        .any(|saved| saved.name == account.name)
    {
        // Retain the existing duplicate-name behavior for API keys and for
        // ChatGPT credentials with no trustworthy workspace ID.
        if workspace_id.is_none() {
            anyhow::bail!("An account with name '{}' already exists", account.name);
        }

        // Different workspaces can legitimately share an email (including
        // two Business workspaces with the same subscription plan). Keep
        // display names unique without conflating them with identity.
        let base = account.name.clone();
        let suffix = account
            .plan_type
            .as_deref()
            .map(str::trim)
            .filter(|plan| !plan.is_empty())
            .unwrap_or("workspace");
        let mut candidate = format!("{base} ({suffix})");
        let mut n = 2;
        while store.accounts.iter().any(|saved| saved.name == candidate) {
            candidate = format!("{base} ({suffix} {n})");
            n += 1;
        }
        account.name = candidate;
    }

    let saved = account.clone();
    store.accounts.push(account);
    Ok(saved)
}

/// Remove an account by ID
pub fn remove_account(account_id: &str) -> Result<()> {
    let mut store = load_accounts()?;

    let initial_len = store.accounts.len();
    store.accounts.retain(|a| a.id != account_id);

    if store.accounts.len() == initial_len {
        anyhow::bail!("Account not found: {account_id}");
    }

    // If we removed the active account, clear it or set to first available
    if store.active_account_id.as_deref() == Some(account_id) {
        store.active_account_id = store.accounts.first().map(|a| a.id.clone());
    }

    save_accounts(&store)?;
    Ok(())
}

/// Update the active account ID
pub fn set_active_account(account_id: &str) -> Result<()> {
    let mut store = load_accounts()?;

    // Verify the account exists
    if !store.accounts.iter().any(|a| a.id == account_id) {
        anyhow::bail!("Account not found: {account_id}");
    }

    store.active_account_id = Some(account_id.to_string());
    save_accounts(&store)?;
    Ok(())
}

/// Get an account by ID
pub fn get_account(account_id: &str) -> Result<Option<StoredAccount>> {
    let store = load_accounts()?;
    Ok(store.accounts.into_iter().find(|a| a.id == account_id))
}

/// Get the currently active account
pub fn get_active_account() -> Result<Option<StoredAccount>> {
    let store = load_accounts()?;
    let active_id = match &store.active_account_id {
        Some(id) => id,
        None => return Ok(None),
    };
    Ok(store.accounts.into_iter().find(|a| a.id == *active_id))
}

/// Update an account's last_used_at timestamp
pub fn touch_account(account_id: &str) -> Result<()> {
    let mut store = load_accounts()?;

    if let Some(account) = store.accounts.iter_mut().find(|a| a.id == account_id) {
        account.last_used_at = Some(chrono::Utc::now());
        save_accounts(&store)?;
    }

    Ok(())
}

/// Update an account's metadata (name, email, plan_type, subscription expiry)
pub fn update_account_metadata(
    account_id: &str,
    name: Option<String>,
    email: Option<String>,
    plan_type: Option<String>,
    subscription_expires_at: Option<Option<DateTime<Utc>>>,
) -> Result<StoredAccount> {
    let mut store = load_accounts()?;

    // Check for duplicate names first (if renaming)
    if let Some(ref new_name) = name {
        if store
            .accounts
            .iter()
            .any(|a| a.id != account_id && a.name == *new_name)
        {
            anyhow::bail!("An account with name '{new_name}' already exists");
        }
    }

    // Now find and update the account
    let account = store
        .accounts
        .iter_mut()
        .find(|a| a.id == account_id)
        .context("Account not found")?;

    let mut changed = false;

    if let Some(new_name) = name {
        if account.name != new_name {
            account.name = new_name;
            changed = true;
        }
    }

    if let Some(new_email) = email {
        if account.email.as_ref() != Some(&new_email) {
            account.email = Some(new_email);
            changed = true;
        }
    }

    if let Some(new_plan_type) = plan_type {
        if account.plan_type.as_ref() != Some(&new_plan_type) {
            account.plan_type = Some(new_plan_type);
            changed = true;
        }
    }

    if let Some(subscription_expires_at) = subscription_expires_at {
        if account.subscription_expires_at != subscription_expires_at {
            account.subscription_expires_at = subscription_expires_at;
            changed = true;
        }
    }

    let updated = account.clone();
    if changed {
        save_accounts(&store)?;
        println!("[Account] Saved updated metadata for: {}", updated.name);
    }
    Ok(updated)
}

/// Update ChatGPT OAuth tokens for an account and return the updated account.
pub fn update_account_chatgpt_tokens(
    account_id: &str,
    id_token: String,
    access_token: String,
    refresh_token: String,
    chatgpt_account_id: Option<String>,
    email: Option<String>,
    plan_type: Option<String>,
    subscription_expires_at: Option<DateTime<Utc>>,
) -> Result<StoredAccount> {
    let mut store = load_accounts()?;

    let account = store
        .accounts
        .iter_mut()
        .find(|a| a.id == account_id)
        .context("Account not found")?;

    match &mut account.auth_data {
        AuthData::ChatGPT {
            id_token: stored_id_token,
            access_token: stored_access_token,
            refresh_token: stored_refresh_token,
            account_id: stored_account_id,
        } => {
            *stored_id_token = id_token;
            *stored_access_token = access_token;
            *stored_refresh_token = refresh_token;
            if let Some(new_account_id) = chatgpt_account_id {
                *stored_account_id = Some(new_account_id);
            }
        }
        AuthData::ApiKey { .. } => {
            anyhow::bail!("Cannot update OAuth tokens for an API key account");
        }
    }

    if let Some(new_email) = email {
        account.email = Some(new_email);
    }

    if let Some(new_plan_type) = plan_type {
        account.plan_type = Some(new_plan_type);
    }

    if let Some(subscription_expires_at) = subscription_expires_at {
        account.subscription_expires_at = Some(subscription_expires_at);
    }

    let updated = account.clone();
    save_accounts(&store)?;
    Ok(updated)
}

/// Get the list of masked account IDs
pub fn get_masked_account_ids() -> Result<Vec<String>> {
    let store = load_accounts()?;
    Ok(store.masked_account_ids.clone())
}

/// Set the list of masked account IDs
pub fn set_masked_account_ids(ids: Vec<String>) -> Result<()> {
    let mut store = load_accounts()?;
    store.masked_account_ids = ids;
    save_accounts(&store)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{add_account_to_store, sync_active_account_tokens};
    use crate::types::{AccountsStore, AuthData, AuthDotJson, StoredAccount, TokenData};
    use base64::Engine;

    fn account(name: &str, account_id: &str, suffix: &str) -> StoredAccount {
        StoredAccount::new_chatgpt(
            name.into(),
            None,
            None,
            None,
            format!("id-{suffix}"),
            format!("access-{suffix}"),
            format!("refresh-{suffix}"),
            Some(account_id.into()),
        )
    }

    fn auth(account_id: &str, suffix: &str) -> AuthDotJson {
        AuthDotJson {
            openai_api_key: None,
            tokens: Some(TokenData {
                id_token: format!("id-{suffix}"),
                access_token: format!("access-{suffix}"),
                refresh_token: format!("refresh-{suffix}"),
                account_id: Some(account_id.into()),
            }),
            last_refresh: None,
        }
    }

    fn refresh_token(account: &StoredAccount) -> &str {
        match &account.auth_data {
            AuthData::ChatGPT { refresh_token, .. } => refresh_token,
            AuthData::ApiKey { .. } => panic!("expected ChatGPT account"),
        }
    }

    fn id_token_with_account_id(account_id: &str, suffix: &str) -> String {
        let payload =
            format!(r#"{{"https://api.openai.com/auth":{{"chatgpt_account_id":"{account_id}"}}}}"#);
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload);
        format!("header.{encoded}.{suffix}")
    }

    #[test]
    fn adding_account_does_not_change_the_active_account() {
        let active = StoredAccount::new_api_key("A".into(), "sk-a".into());
        let active_id = active.id.clone();
        let stores = [
            AccountsStore::default(),
            AccountsStore {
                accounts: vec![active],
                active_account_id: Some(active_id),
                ..AccountsStore::default()
            },
        ];

        for mut store in stores {
            let expected_active_id = store.active_account_id.clone();

            add_account_to_store(
                &mut store,
                StoredAccount::new_api_key("B".into(), "sk-b".into()),
            )
            .unwrap();

            assert_eq!(store.active_account_id, expected_active_id);
        }
    }

    #[test]
    fn same_email_personal_and_business_workspaces_remain_separate() {
        let mut store = AccountsStore::default();
        let mut personal = account("person@example.com", "personal-ws", "p");
        personal.email = Some("person@example.com".into());
        personal.plan_type = Some("plus".into());
        let mut business = account("person@example.com", "business-ws", "b");
        business.email = personal.email.clone();
        business.plan_type = Some("business".into());

        let personal = add_account_to_store(&mut store, personal).unwrap();
        let business = add_account_to_store(&mut store, business).unwrap();
        assert_eq!(store.accounts.len(), 2);
        assert_ne!(personal.id, business.id);
        assert_eq!(business.name, "person@example.com (business)");
        assert_eq!(refresh_token(&store.accounts[0]), "refresh-p");
        assert_eq!(refresh_token(&store.accounts[1]), "refresh-b");
    }

    #[test]
    fn multiple_business_workspaces_get_unique_names() {
        let mut store = AccountsStore::default();
        for n in 1..=3 {
            let mut candidate = account(
                "shared@example.com",
                &format!("workspace-{n}"),
                &n.to_string(),
            );
            candidate.plan_type = Some("business".into());
            add_account_to_store(&mut store, candidate).unwrap();
        }
        assert_eq!(store.accounts[0].name, "shared@example.com");
        assert_eq!(store.accounts[1].name, "shared@example.com (business)");
        assert_eq!(store.accounts[2].name, "shared@example.com (business 2)");
    }

    #[test]
    fn same_workspace_with_different_display_name_is_not_duplicated() {
        let mut store = AccountsStore::default();
        add_account_to_store(&mut store, account("Personal", "workspace-one", "old")).unwrap();
        assert!(add_account_to_store(
            &mut store,
            account("Renamed account", "workspace-one", "new")
        )
        .is_err());
        assert_eq!(store.accounts.len(), 1);
        assert_eq!(refresh_token(&store.accounts[0]), "refresh-old");
    }

    #[test]
    fn current_token_claim_takes_precedence_over_stale_cached_workspace_id() {
        let mut store = AccountsStore::default();
        let mut saved = account("A", "stale-cache", "old");
        if let AuthData::ChatGPT { id_token, .. } = &mut saved.auth_data {
            *id_token = id_token_with_account_id("actual-workspace", "sig");
        }
        add_account_to_store(&mut store, saved).unwrap();
        assert!(add_account_to_store(&mut store, account("B", "actual-workspace", "new")).is_err());
    }

    #[test]
    fn unknown_workspace_keeps_duplicate_name_protection() {
        let mut store = AccountsStore::default();
        let mut unknown = account("same@example.com", "unused", "a");
        if let AuthData::ChatGPT {
            id_token,
            account_id,
            ..
        } = &mut unknown.auth_data
        {
            *id_token = "opaque".into();
            *account_id = None;
        }
        add_account_to_store(&mut store, unknown).unwrap();
        let mut other = account("same@example.com", "unused", "b");
        if let AuthData::ChatGPT {
            id_token,
            account_id,
            ..
        } = &mut other.auth_data
        {
            *id_token = "opaque".into();
            *account_id = None;
        }
        assert!(add_account_to_store(&mut store, other).is_err());
        assert_eq!(store.accounts.len(), 1);
    }

    #[test]
    fn api_key_duplicate_names_still_fail() {
        let mut store = AccountsStore::default();
        add_account_to_store(
            &mut store,
            StoredAccount::new_api_key("API".into(), "key-1".into()),
        )
        .unwrap();
        assert!(add_account_to_store(
            &mut store,
            StoredAccount::new_api_key("API".into(), "key-2".into())
        )
        .is_err());
    }

    #[test]
    fn preserves_rotated_tokens_before_switching_away_and_back() {
        let account_a = account("A", "workspace-a", "a1");
        let account_a_id = account_a.id.clone();
        let account_b = account("B", "workspace-b", "b1");
        let account_b_id = account_b.id.clone();
        let mut store = AccountsStore {
            accounts: vec![account_a, account_b],
            active_account_id: Some(account_a_id.clone()),
            ..AccountsStore::default()
        };

        assert!(!sync_active_account_tokens(
            &mut store,
            &auth("workspace-b", "wrong-account")
        ));
        assert_eq!(refresh_token(&store.accounts[0]), "refresh-a1");

        let mut auth_without_top_level_id = auth("workspace-b", "missing-id");
        let tokens = auth_without_top_level_id.tokens.as_mut().unwrap();
        tokens.id_token = id_token_with_account_id("workspace-b", "signature");
        tokens.account_id = None;
        assert!(!sync_active_account_tokens(
            &mut store,
            &auth_without_top_level_id
        ));
        assert_eq!(refresh_token(&store.accounts[0]), "refresh-a1");

        let mut auth_without_identity = auth("workspace-a", "unknown");
        auth_without_identity.tokens.as_mut().unwrap().account_id = None;
        assert!(!sync_active_account_tokens(
            &mut store,
            &auth_without_identity
        ));
        assert_eq!(refresh_token(&store.accounts[0]), "refresh-a1");

        assert!(sync_active_account_tokens(
            &mut store,
            &auth("workspace-a", "a2")
        ));
        store.active_account_id = Some(account_b_id);

        let restored_a = store
            .accounts
            .iter()
            .find(|account| account.id == account_a_id)
            .unwrap();
        let AuthData::ChatGPT { refresh_token, .. } = &restored_a.auth_data else {
            panic!("expected ChatGPT account");
        };
        assert_eq!(refresh_token, "refresh-a2");
    }

    #[test]
    fn rejects_live_tokens_when_stored_account_identity_is_unknown() {
        let mut account = account("A", "workspace-a", "a1");
        let account_id = account.id.clone();
        let AuthData::ChatGPT {
            id_token,
            account_id: chatgpt_account_id,
            ..
        } = &mut account.auth_data
        else {
            panic!("expected ChatGPT account");
        };
        *id_token = "opaque-id-token".into();
        *chatgpt_account_id = None;

        let mut store = AccountsStore {
            accounts: vec![account],
            active_account_id: Some(account_id),
            ..AccountsStore::default()
        };

        assert!(!sync_active_account_tokens(
            &mut store,
            &auth("workspace-a", "a2")
        ));
        assert_eq!(refresh_token(&store.accounts[0]), "refresh-a1");
    }

    #[test]
    fn derives_stored_identity_from_id_token_and_backfills_account_id() {
        let mut account = account("A", "workspace-a", "a1");
        let account_id = account.id.clone();
        let AuthData::ChatGPT {
            id_token,
            account_id: chatgpt_account_id,
            ..
        } = &mut account.auth_data
        else {
            panic!("expected ChatGPT account");
        };
        *id_token = id_token_with_account_id("workspace-a", "stored");
        *chatgpt_account_id = None;

        let mut store = AccountsStore {
            accounts: vec![account],
            active_account_id: Some(account_id),
            ..AccountsStore::default()
        };

        assert!(sync_active_account_tokens(
            &mut store,
            &auth("workspace-a", "a2")
        ));
        let AuthData::ChatGPT { account_id, .. } = &store.accounts[0].auth_data else {
            panic!("expected ChatGPT account");
        };
        assert_eq!(account_id.as_deref(), Some("workspace-a"));
        assert_eq!(refresh_token(&store.accounts[0]), "refresh-a2");
    }
}
