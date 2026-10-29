use super::*;
use crate::auth::test_support::TestHome;
use crate::auth::{get_accounts_file, get_codex_auth_file, save_accounts, switch_to_account};
use crate::types::{AccountsStore, AuthData, StoredAccount};
use std::fs;

// PENDING_OAUTH is process-wide; each test keeps its filesystem on its own thread.
static OAUTH_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn account(name: &str) -> StoredAccount {
    StoredAccount::new_chatgpt(
        name.into(),
        None,
        None,
        None,
        format!("test-id-{name}"),
        format!("test-access-{name}"),
        format!("test-refresh-{name}"),
        Some(format!("test-workspace-{name}")),
    )
}

fn complete_with(result: anyhow::Result<OAuthLoginResult>) {
    let (tx, rx) = oneshot::channel();
    assert!(tx.send(result).is_ok());
    *PENDING_OAUTH.lock().unwrap() = Some(PendingOAuth {
        rx: Some(rx),
        reauth_account_id: None,
        cancelled: Arc::new(AtomicBool::new(false)),
    });
}

#[tokio::test(flavor = "current_thread")]
async fn adding_oauth_account_preserves_active_account_and_live_auth() {
    let _lock = OAUTH_TEST_LOCK.lock().await;
    let _home = TestHome::new();
    let mut active = account("A");
    active.last_used_at = Some("2026-01-01T00:00:00Z".parse().unwrap());
    save_accounts(&AccountsStore {
        accounts: vec![active.clone()],
        active_account_id: Some(active.id.clone()),
        ..AccountsStore::default()
    })
    .unwrap();

    // The live session may contain newer tokens than the saved account snapshot.
    let mut live = active.clone();
    if let AuthData::ChatGPT {
        access_token,
        refresh_token,
        ..
    } = &mut live.auth_data
    {
        *access_token = "test-access-A-rotated".into();
        *refresh_token = "test-refresh-A-rotated".into();
    }
    switch_to_account(&live).unwrap();
    let auth_path = get_codex_auth_file().unwrap();
    let auth_before = fs::read(&auth_path).unwrap();
    complete_with(Ok(OAuthLoginResult {
        account: account("B"),
    }));

    let added = complete_login().await.unwrap();
    let store = load_accounts().unwrap();
    assert_eq!(store.active_account_id.as_deref(), Some(active.id.as_str()));
    assert_eq!(fs::read(auth_path).unwrap(), auth_before);
    assert_eq!(store.accounts.len(), 2);
    assert_eq!(
        serde_json::to_value(&store.accounts[0]).unwrap(),
        serde_json::to_value(active).unwrap()
    );
    assert_eq!(store.accounts[1].id, added.id);
    assert!(!added.is_active);
    assert!(store.accounts[1].last_used_at.is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn adding_first_oauth_account_does_not_activate_or_create_auth() {
    let _lock = OAUTH_TEST_LOCK.lock().await;
    let _home = TestHome::new();
    complete_with(Ok(OAuthLoginResult {
        account: account("first"),
    }));

    let added = complete_login().await.unwrap();
    let store = load_accounts().unwrap();
    assert!(store.active_account_id.is_none());
    assert!(!get_codex_auth_file().unwrap().exists());
    assert_eq!(store.accounts.len(), 1);
    assert_eq!(store.accounts[0].id, added.id);
    assert!(!added.is_active);
    assert!(store.accounts[0].last_used_at.is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn adding_oauth_account_preserves_no_active_account_and_external_auth() {
    let _lock = OAUTH_TEST_LOCK.lock().await;
    let _home = TestHome::new();
    save_accounts(&AccountsStore {
        accounts: vec![account("saved")],
        ..AccountsStore::default()
    })
    .unwrap();
    switch_to_account(&account("external")).unwrap();
    let auth_path = get_codex_auth_file().unwrap();
    let auth_before = fs::read(&auth_path).unwrap();
    complete_with(Ok(OAuthLoginResult {
        account: account("new"),
    }));

    let added = complete_login().await.unwrap();
    let store = load_accounts().unwrap();
    assert!(store.active_account_id.is_none());
    assert_eq!(fs::read(auth_path).unwrap(), auth_before);
    assert_eq!(store.accounts.len(), 2);
    assert!(!added.is_active);
    assert!(store.accounts[1].last_used_at.is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn duplicate_or_failed_oauth_login_leaves_storage_and_auth_unchanged() {
    let _lock = OAUTH_TEST_LOCK.lock().await;
    let _home = TestHome::new();
    let active = account("A");
    save_accounts(&AccountsStore {
        accounts: vec![active.clone()],
        active_account_id: Some(active.id.clone()),
        ..AccountsStore::default()
    })
    .unwrap();
    switch_to_account(&active).unwrap();
    let store_path = get_accounts_file().unwrap();
    let auth_path = get_codex_auth_file().unwrap();
    let store_before = fs::read(&store_path).unwrap();
    let auth_before = fs::read(&auth_path).unwrap();

    for result in [
        Ok(OAuthLoginResult {
            account: account("A"),
        }),
        Err(anyhow::anyhow!("synthetic OAuth failure")),
    ] {
        complete_with(result);
        assert!(complete_login().await.is_err());
        assert_eq!(fs::read(&store_path).unwrap(), store_before);
        assert_eq!(fs::read(&auth_path).unwrap(), auth_before);
    }
}

fn relogin_result(target: &StoredAccount, mut fresh: StoredAccount) {
    // Fixtures use non-JWT tokens, so the identity check falls back to email.
    fresh.email = Some("same@example.test".into());
    complete_with(Ok(OAuthLoginResult { account: fresh }));
    PENDING_OAUTH
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .reauth_account_id = Some(target.id.clone());
}

#[tokio::test(flavor = "current_thread")]
async fn relogin_updates_in_place_without_touching_other_accounts_or_live_auth() {
    let _lock = OAUTH_TEST_LOCK.lock().await;
    let _home = TestHome::new();
    let active = account("active");
    let mut old = account("renew");
    old.email = Some("same@example.test".into());
    old.last_used_at = Some("2026-01-01T00:00:00Z".parse().unwrap());
    save_accounts(&AccountsStore {
        accounts: vec![active.clone(), old.clone()],
        active_account_id: Some(active.id.clone()),
        masked_account_ids: vec![old.id.clone()],
        ..AccountsStore::default()
    })
    .unwrap();
    switch_to_account(&active).unwrap();
    let auth_before = fs::read(get_codex_auth_file().unwrap()).unwrap();
    let mut fresh = account("renew");
    fresh.plan_type = Some("pro".into());
    if let AuthData::ChatGPT { refresh_token, .. } = &mut fresh.auth_data {
        *refresh_token = "renewed-test-refresh".into();
    }
    relogin_result(&old, fresh.clone());
    let info = complete_login().await.unwrap();
    let store = load_accounts().unwrap();
    assert_eq!(info.id, old.id);
    assert_eq!(store.accounts.len(), 2);
    assert_eq!(store.active_account_id, Some(active.id.clone()));
    assert_eq!(store.masked_account_ids, vec![old.id.clone()]);
    let updated = &store.accounts[1];
    assert_eq!(updated.name, old.name);
    assert_eq!(updated.created_at, old.created_at);
    assert_eq!(updated.last_used_at, old.last_used_at);
    assert_eq!(updated.plan_type, fresh.plan_type);
    assert_eq!(
        serde_json::to_value(&updated.auth_data).unwrap(),
        serde_json::to_value(&fresh.auth_data).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&store.accounts[0]).unwrap(),
        serde_json::to_value(&active).unwrap()
    );
    assert_eq!(
        fs::read(get_codex_auth_file().unwrap()).unwrap(),
        auth_before
    );
}

#[tokio::test(flavor = "current_thread")]
async fn relogin_rejects_wrong_workspace_missing_account_and_cancelled_result() {
    let _lock = OAUTH_TEST_LOCK.lock().await;
    let _home = TestHome::new();
    let mut old = account("renew");
    old.email = Some("same@example.test".into());
    save_accounts(&AccountsStore {
        accounts: vec![old.clone()],
        ..AccountsStore::default()
    })
    .unwrap();
    let before = fs::read(get_accounts_file().unwrap()).unwrap();
    relogin_result(&old, account("wrong-workspace"));
    assert!(complete_login()
        .await
        .unwrap_err()
        .contains("does not match"));
    assert_eq!(fs::read(get_accounts_file().unwrap()).unwrap(), before);
    relogin_result(&account("deleted"), account("renew"));
    assert!(complete_login()
        .await
        .unwrap_err()
        .contains("no longer exists"));
    assert_eq!(fs::read(get_accounts_file().unwrap()).unwrap(), before);

    // Cancellation must remain reachable after complete_login takes the receiver,
    // including when the OAuth result is waiting for another auth operation.
    relogin_result(&old, account("renew"));
    let guard = AUTH_OPERATION_LOCK.lock().await;
    let completion = complete_login();
    tokio::pin!(completion);
    tokio::select! {
        biased;
        _ = &mut completion => panic!("must wait for auth lock"),
        _ = tokio::task::yield_now() => {},
    }
    cancel_login().await.unwrap();
    drop(guard);
    assert!(completion.await.unwrap_err().contains("cancelled"));
    assert_eq!(fs::read(get_accounts_file().unwrap()).unwrap(), before);
    assert!(!get_codex_auth_file().unwrap().exists());
}

#[test]
fn relogin_matches_user_and_workspace_and_fails_closed_without_identity() {
    use base64::Engine;
    fn user(subject: &str, workspace: &str) -> StoredAccount {
        let mut a = account("same");
        a.email = Some("same@example.test".into());
        if let AuthData::ChatGPT {
            id_token,
            account_id,
            ..
        } = &mut a.auth_data
        {
            let payload = serde_json::json!({"sub": subject});
            *id_token = format!(
                "e30.{}.signature",
                base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.to_string())
            );
            *account_id = Some(workspace.into());
        }
        a
    }
    assert!(validate_relogin_identity(&user("u1", "w1"), &user("u1", "w1")).is_ok());
    assert!(validate_relogin_identity(&user("u1", "w1"), &user("u2", "w1")).is_err());
    assert!(validate_relogin_identity(&user("u1", "w1"), &user("u1", "w2")).is_err());
    assert!(validate_relogin_identity(&user("u1", ""), &user("u1", "")).is_err());
    assert!(validate_relogin_identity(&account("same"), &account("same")).is_err());
    let mut old = account("same");
    old.email = Some("old@example.test".into());
    let mut new = account("same");
    new.email = Some("new@example.test".into());
    assert!(validate_relogin_identity(&old, &new).is_err());
    assert!(validate_relogin_identity(
        &old,
        &StoredAccount::new_api_key("key".into(), "test".into())
    )
    .is_err());
}

#[test]
fn active_relogin_checks_process_safety_and_syncs_live_auth() {
    let _home = TestHome::new();
    let mut old = account("active");
    old.email = Some("same@example.test".into());
    save_accounts(&AccountsStore {
        accounts: vec![old.clone()],
        active_account_id: Some(old.id.clone()),
        ..AccountsStore::default()
    })
    .unwrap();
    switch_to_account(&old).unwrap();
    let auth_before = fs::read(get_codex_auth_file().unwrap()).unwrap();
    let store_before = fs::read(get_accounts_file().unwrap()).unwrap();
    let mut fresh = old.clone();
    if let AuthData::ChatGPT { refresh_token, .. } = &mut fresh.auth_data {
        *refresh_token = "renewed-active-refresh".into();
    }
    assert!(
        save_relogin_with_check(&old.id, fresh.clone(), || Err("Codex running".into())).is_err()
    );
    assert_eq!(
        fs::read(get_codex_auth_file().unwrap()).unwrap(),
        auth_before
    );
    assert_eq!(
        fs::read(get_accounts_file().unwrap()).unwrap(),
        store_before
    );
    let saved = save_relogin_with_check(&old.id, fresh, || Ok(())).unwrap();
    assert_eq!(saved.id, old.id);
    let store = load_accounts().unwrap();
    assert_eq!(store.active_account_id, Some(old.id));
    let auth = crate::auth::read_current_auth().unwrap().unwrap();
    assert_eq!(auth.tokens.unwrap().refresh_token, "renewed-active-refresh");
    let mut reconciled = store.clone();
    assert!(!crate::auth::sync_active_account_tokens(
        &mut reconciled,
        &crate::auth::read_current_auth().unwrap().unwrap()
    ));
}

#[test]
fn active_relogin_auth_write_failure_leaves_saved_credentials_unchanged() {
    let _home = TestHome::new();
    let mut old = account("active");
    old.email = Some("same@example.test".into());
    save_accounts(&AccountsStore {
        accounts: vec![old.clone()],
        active_account_id: Some(old.id.clone()),
        ..AccountsStore::default()
    })
    .unwrap();
    let before = fs::read(get_accounts_file().unwrap()).unwrap();
    fs::create_dir_all(get_codex_auth_file().unwrap()).unwrap();
    assert!(save_relogin_with_check(&old.id, old.clone(), || Ok(()))
        .unwrap_err()
        .contains("saved account was not changed"));
    assert_eq!(fs::read(get_accounts_file().unwrap()).unwrap(), before);
}

#[test]
fn inactive_relogin_never_requires_closing_codex() {
    let _home = TestHome::new();
    let mut old = account("inactive");
    old.email = Some("same@example.test".into());
    save_accounts(&AccountsStore {
        accounts: vec![old.clone()],
        ..AccountsStore::default()
    })
    .unwrap();
    save_relogin_with_check(&old.id, old.clone(), || {
        panic!("inactive account must not check running Codex")
    })
    .unwrap();
    assert!(!get_codex_auth_file().unwrap().exists());
}
