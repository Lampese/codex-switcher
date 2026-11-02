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
        rx,
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
