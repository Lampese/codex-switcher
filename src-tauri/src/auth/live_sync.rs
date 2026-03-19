//! Reconcile the live Codex auth.json with the switcher's account store.

use anyhow::{Context, Result};
use tokio::time::{sleep, Duration};

use super::{get_codex_auth_file, load_accounts, read_current_auth, save_accounts};
use crate::auth::chatgpt::parse_chatgpt_token_claims;
use crate::types::{AuthData, AuthMode, LiveAuthSyncResult, StoredAccount};

const AUTH_READ_RETRIES: usize = 3;
const AUTH_READ_RETRY_DELAY_MS: u64 = 120;

#[derive(Debug, Clone)]
struct LiveChatGptAuth {
    id_token: String,
    access_token: String,
    refresh_token: String,
    account_id: Option<String>,
    email: Option<String>,
    plan_type: Option<String>,
}

#[derive(Debug, Clone)]
enum LiveAuthState {
    None,
    ApiKey { key: String },
    ChatGpt(LiveChatGptAuth),
}

pub async fn sync_live_auth_to_store() -> Result<LiveAuthSyncResult> {
    let live_auth = read_live_auth_with_retry().await?;
    let mut store = load_accounts()?;
    let previous_active_id = store.active_account_id.clone();
    let mut created_account_id = None;
    let mut updated_account_id = None;
    let mut cleared_active_account = false;
    let mut changed = false;

    match live_auth {
        LiveAuthState::None => {
            if store.active_account_id.take().is_some() {
                cleared_active_account = true;
                changed = true;
            }
        }
        LiveAuthState::ApiKey { key } => {
            let active_id = if let Some(index) =
                find_matching_api_key_account(&store.accounts, &key)
            {
                store.accounts[index].id.clone()
            } else {
                let account =
                    StoredAccount::new_api_key(build_unique_name(&store.accounts, "api-key"), key);
                let account_id = account.id.clone();
                store.accounts.push(account);
                created_account_id = Some(account_id.clone());
                changed = true;
                account_id
            };

            if store.active_account_id.as_deref() != Some(active_id.as_str()) {
                store.active_account_id = Some(active_id);
                changed = true;
            }
        }
        LiveAuthState::ChatGpt(live) => {
            let active_id =
                if let Some(index) = find_matching_chatgpt_account(&store.accounts, &live) {
                    let account = &mut store.accounts[index];
                    let mut account_changed = false;

                    match &mut account.auth_data {
                        AuthData::ChatGPT {
                            id_token,
                            access_token,
                            refresh_token,
                            account_id,
                        } => {
                            if *id_token != live.id_token {
                                *id_token = live.id_token.clone();
                                account_changed = true;
                            }
                            if *access_token != live.access_token {
                                *access_token = live.access_token.clone();
                                account_changed = true;
                            }
                            if *refresh_token != live.refresh_token {
                                *refresh_token = live.refresh_token.clone();
                                account_changed = true;
                            }
                            if *account_id != live.account_id {
                                *account_id = live.account_id.clone();
                                account_changed = true;
                            }
                        }
                        AuthData::ApiKey { .. } => {
                            anyhow::bail!("Matched a non-ChatGPT account while syncing live auth");
                        }
                    }

                    if account.email != live.email {
                        account.email = live.email.clone();
                        account_changed = true;
                    }

                    if account.plan_type != live.plan_type {
                        account.plan_type = live.plan_type.clone();
                        account_changed = true;
                    }

                    if account_changed {
                        updated_account_id = Some(account.id.clone());
                        changed = true;
                    }

                    account.id.clone()
                } else {
                    let account = StoredAccount::new_chatgpt(
                        build_chatgpt_account_name(&store.accounts, live.email.as_deref()),
                        live.email.clone(),
                        live.plan_type.clone(),
                        live.id_token,
                        live.access_token,
                        live.refresh_token,
                        live.account_id,
                    );
                    let account_id = account.id.clone();
                    store.accounts.push(account);
                    created_account_id = Some(account_id.clone());
                    changed = true;
                    account_id
                };

            if store.active_account_id.as_deref() != Some(active_id.as_str()) {
                store.active_account_id = Some(active_id);
                changed = true;
            }
        }
    }

    if previous_active_id != store.active_account_id {
        changed = true;
    }

    if changed {
        save_accounts(&store)?;
    }

    Ok(LiveAuthSyncResult {
        changed,
        active_account_id: store.active_account_id,
        created_account_id,
        updated_account_id,
        cleared_active_account,
    })
}

async fn read_live_auth_with_retry() -> Result<LiveAuthState> {
    let auth_path = get_codex_auth_file()?;
    if !auth_path.exists() {
        return Ok(LiveAuthState::None);
    }

    let mut last_error = None;

    for attempt in 0..AUTH_READ_RETRIES {
        match read_current_auth() {
            Ok(auth) => return auth_dot_json_to_live_state(auth),
            Err(err) => {
                last_error = Some(err);
                if attempt + 1 < AUTH_READ_RETRIES {
                    sleep(Duration::from_millis(AUTH_READ_RETRY_DELAY_MS)).await;
                }
            }
        }
    }

    Err(last_error
        .context("Failed to read live auth.json after retries")?
        .into())
}

fn auth_dot_json_to_live_state(auth: Option<crate::types::AuthDotJson>) -> Result<LiveAuthState> {
    let Some(auth) = auth else {
        return Ok(LiveAuthState::None);
    };

    if let Some(tokens) = auth.tokens {
        if tokens.id_token.trim().is_empty()
            || tokens.access_token.trim().is_empty()
            || tokens.refresh_token.trim().is_empty()
        {
            anyhow::bail!("Live auth.json contains incomplete ChatGPT tokens");
        }

        let claims = parse_chatgpt_token_claims(&tokens.id_token);
        return Ok(LiveAuthState::ChatGpt(LiveChatGptAuth {
            id_token: tokens.id_token,
            access_token: tokens.access_token,
            refresh_token: tokens.refresh_token,
            account_id: claims.account_id.or(tokens.account_id),
            email: claims.email,
            plan_type: claims.plan_type,
        }));
    }

    if let Some(api_key) = auth.openai_api_key {
        if api_key.trim().is_empty() {
            anyhow::bail!("Live auth.json contains an empty API key");
        }

        return Ok(LiveAuthState::ApiKey { key: api_key });
    }

    Ok(LiveAuthState::None)
}

fn find_matching_api_key_account(accounts: &[StoredAccount], live_key: &str) -> Option<usize> {
    accounts
        .iter()
        .position(|account| match &account.auth_data {
            AuthData::ApiKey { key } => key == live_key,
            AuthData::ChatGPT { .. } => false,
        })
}

fn find_matching_chatgpt_account(
    accounts: &[StoredAccount],
    live: &LiveChatGptAuth,
) -> Option<usize> {
    if let Some(account_id) = live.account_id.as_deref() {
        if let Some(index) = accounts
            .iter()
            .position(|account| match &account.auth_data {
                AuthData::ChatGPT {
                    account_id: Some(stored_account_id),
                    ..
                } => stored_account_id == account_id,
                _ => false,
            })
        {
            return Some(index);
        }
    }

    if let Some(email) = live.email.as_deref() {
        if let Some(index) = accounts.iter().position(|account| {
            matches!(account.auth_mode, AuthMode::ChatGPT)
                && account
                    .email
                    .as_deref()
                    .is_some_and(|stored_email| stored_email.eq_ignore_ascii_case(email))
        }) {
            return Some(index);
        }
    }

    accounts
        .iter()
        .position(|account| match &account.auth_data {
            AuthData::ChatGPT {
                refresh_token,
                access_token,
                ..
            } => refresh_token == &live.refresh_token || access_token == &live.access_token,
            AuthData::ApiKey { .. } => false,
        })
}

fn build_chatgpt_account_name(accounts: &[StoredAccount], email: Option<&str>) -> String {
    let preferred = email
        .and_then(|value| value.split('@').next())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("chatgpt");
    build_unique_name(accounts, preferred)
}

fn build_unique_name(accounts: &[StoredAccount], preferred: &str) -> String {
    let base = preferred.trim();
    if base.is_empty() {
        return build_unique_name(accounts, "account");
    }

    if accounts.iter().all(|account| account.name != base) {
        return base.to_string();
    }

    let mut suffix = 2usize;
    loop {
        let candidate = format!("{base}-{suffix}");
        if accounts.iter().all(|account| account.name != candidate) {
            return candidate;
        }
        suffix += 1;
    }
}
