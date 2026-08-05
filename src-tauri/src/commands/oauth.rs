//! OAuth login Tauri commands

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

use crate::auth::account_repository::AccountRepository;
use crate::auth::oauth_server::{start_oauth_login, wait_for_oauth_login, OAuthLoginResult};
use crate::auth::paths::AppPaths;
use crate::types::{AccountInfo, OAuthLoginInfo};

struct PendingOAuth {
    rx: oneshot::Receiver<anyhow::Result<OAuthLoginResult>>,
    cancelled: Arc<AtomicBool>,
}

// Global state for pending OAuth login
static PENDING_OAUTH: Mutex<Option<PendingOAuth>> = Mutex::new(None);

/// Start the OAuth login flow
#[tauri::command]
pub async fn start_login(account_name: String) -> Result<OAuthLoginInfo, String> {
    // Cancel any previous pending flow so it does not keep the callback port occupied.
    if let Some(previous) = {
        let mut pending = PENDING_OAUTH.lock().unwrap();
        pending.take()
    } {
        previous.cancelled.store(true, Ordering::Relaxed);
    }

    let (info, rx, cancelled) = start_oauth_login(account_name.trim().to_string())
        .await
        .map_err(|e| e.to_string())?;

    // Store the receiver for later
    {
        let mut pending = PENDING_OAUTH.lock().unwrap();
        *pending = Some(PendingOAuth { rx, cancelled });
    }

    Ok(info)
}

async fn complete_pending_login_with_repository(
    repository: &AccountRepository,
    pending: PendingOAuth,
) -> Result<AccountInfo, String> {
    let account = wait_for_oauth_login(pending.rx)
        .await
        .map_err(|_| "OAuth login failed".to_string())?;

    crate::commands::account::add_stored_account_with_repository(repository, account).await
}

async fn complete_login_with_repository(
    repository: &AccountRepository,
) -> Result<AccountInfo, String> {
    let pending = {
        let mut pending = PENDING_OAUTH.lock().unwrap();
        pending
            .take()
            .ok_or_else(|| "No pending OAuth login".to_string())?
    };

    complete_pending_login_with_repository(repository, pending).await
}

/// Standalone adapter used by the web dispatcher, which does not have Tauri State.
pub async fn complete_login() -> Result<AccountInfo, String> {
    let paths = AppPaths::production()
        .map_err(|_| "Failed to resolve account storage paths".to_string())?;
    let repository = AccountRepository::from_paths(paths);
    complete_login_with_repository(&repository).await
}

pub(crate) mod secure_oauth_tauri_commands {
    use super::*;

    #[tauri::command]
    pub(crate) async fn complete_login(
        repository: tauri::State<'_, AccountRepository>,
    ) -> Result<AccountInfo, String> {
        complete_login_with_repository(&repository).await
    }
}

/// Cancel a pending OAuth login
#[tauri::command]
pub async fn cancel_login() -> Result<(), String> {
    let mut pending = PENDING_OAUTH.lock().unwrap();
    if let Some(pending_oauth) = pending.take() {
        pending_oauth.cancelled.store(true, Ordering::Relaxed);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::metadata_store::{MetadataAuthKind, MetadataFileStore};
    use crate::auth::paths::AppPaths;
    use crate::auth::vault::{SecretRecord, VaultPayloadV1, VaultStore};
    use crate::types::{AccountsStore, AuthData, AuthMode, StoredAccount};
    use chrono::{TimeZone, Utc};
    use std::path::{Path, PathBuf};
    use std::sync::OnceLock;

    const ID_TOKEN_A: &str = "synthetic-id-token-A";
    const ACCESS_TOKEN_A: &str = "synthetic-access-token-A";
    const REFRESH_TOKEN_A: &str = "synthetic-refresh-token-A";
    const CHATGPT_ACCOUNT_A: &str = "synthetic-chatgpt-account-A";
    const ACCOUNT_ID_A: &str = "oauth-account-A";
    const ACCOUNT_ID_B: &str = "oauth-account-B";
    const DISPLAY_NAME_A: &str = "OAuth Account A";
    const DISPLAY_NAME_B: &str = "OAuth Account B";
    const EMAIL_A: &str = "oauth-account-A@example.test";
    const EMAIL_B: &str = "oauth-account-B@example.test";

    fn test_paths(label: &str) -> (PathBuf, AppPaths) {
        let root = std::env::temp_dir().join(format!(
            "codex_switcher_oauth_test_{label}_{}",
            rand::random::<u64>()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let paths = AppPaths::for_test(&root);
        (root, paths)
    }

    fn cleanup(root: PathBuf) {
        std::fs::remove_dir_all(root).unwrap();
    }

    fn timestamp(hour: u32) -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 5, hour, 0, 0)
            .single()
            .unwrap()
    }

    fn synthetic_account(id: &str, name: &str, email: &str) -> StoredAccount {
        StoredAccount {
            id: id.to_string(),
            name: name.to_string(),
            email: Some(email.to_string()),
            plan_type: Some("plus".to_string()),
            subscription_expires_at: None,
            auth_mode: AuthMode::ChatGPT,
            auth_data: AuthData::ChatGPT {
                id_token: ID_TOKEN_A.to_string(),
                access_token: ACCESS_TOKEN_A.to_string(),
                refresh_token: REFRESH_TOKEN_A.to_string(),
                account_id: Some(CHATGPT_ACCOUNT_A.to_string()),
            },
            created_at: timestamp(12),
            last_used_at: None,
        }
    }

    fn pending_success(account: StoredAccount) -> PendingOAuth {
        let (tx, rx) = oneshot::channel();
        assert!(tx.send(Ok(OAuthLoginResult { account })).is_ok());
        PendingOAuth {
            rx,
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    fn pending_provider_failure(message: String) -> PendingOAuth {
        let (tx, rx) = oneshot::channel();
        assert!(tx.send(Err(anyhow::anyhow!(message))).is_ok());
        PendingOAuth {
            rx,
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    fn pending_receiver_failure() -> PendingOAuth {
        let (tx, rx) = oneshot::channel::<anyhow::Result<OAuthLoginResult>>();
        drop(tx);
        PendingOAuth {
            rx,
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    fn write_legacy_store(paths: &AppPaths) -> Vec<u8> {
        std::fs::create_dir_all(&paths.switcher_dir).unwrap();
        let store = AccountsStore {
            version: 1,
            accounts: vec![synthetic_account(ACCOUNT_ID_A, DISPLAY_NAME_A, EMAIL_A)],
            active_account_id: Some(ACCOUNT_ID_A.to_string()),
            masked_account_ids: Vec::new(),
        };
        let bytes = serde_json::to_vec(&store).unwrap();
        std::fs::write(&paths.metadata_file, &bytes).unwrap();
        bytes
    }

    fn write_orphan_vault(paths: &AppPaths) -> Vec<u8> {
        let bytes = b"synthetic oauth orphan vault".to_vec();
        std::fs::create_dir_all(&paths.switcher_dir).unwrap();
        std::fs::write(&paths.vault_file, &bytes).unwrap();
        bytes
    }

    fn read_pair(paths: &AppPaths) -> (Vec<u8>, Vec<u8>) {
        (
            std::fs::read(&paths.metadata_file).unwrap(),
            std::fs::read(&paths.vault_file).unwrap(),
        )
    }

    fn load_metadata(paths: &AppPaths) -> crate::auth::metadata_store::MetadataStoreV2 {
        MetadataFileStore::from_paths(paths).load().unwrap()
    }

    fn load_vault(paths: &AppPaths) -> VaultPayloadV1 {
        VaultStore::from_paths(paths).load().unwrap()
    }

    fn assert_sanitized(error: &str, root: &Path) {
        for secret in [
            ID_TOKEN_A,
            ACCESS_TOKEN_A,
            REFRESH_TOKEN_A,
            CHATGPT_ACCOUNT_A,
        ] {
            assert!(!error.contains(secret), "error leaked synthetic secret");
        }
        for metadata_value in [
            ACCOUNT_ID_A,
            ACCOUNT_ID_B,
            DISPLAY_NAME_A,
            DISPLAY_NAME_B,
            EMAIL_A,
            EMAIL_B,
        ] {
            assert!(
                !error.contains(metadata_value),
                "error leaked synthetic metadata value"
            );
        }
        let root_text = root.to_string_lossy();
        assert!(!error.contains(root_text.as_ref()));
    }

    fn pending_test_serial() -> &'static tokio::sync::Mutex<()> {
        static SERIAL: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
        SERIAL.get_or_init(|| tokio::sync::Mutex::new(()))
    }

    fn clear_pending() {
        let mut pending = PENDING_OAUTH.lock().unwrap();
        *pending = None;
    }

    struct PendingReset;

    impl Drop for PendingReset {
        fn drop(&mut self) {
            clear_pending();
        }
    }

    #[tokio::test]
    async fn test_successful_pending_oauth_creates_secure_pair() {
        let (root, paths) = test_paths("success_pair");
        let repository = AccountRepository::for_test(paths.clone());

        let info = complete_pending_login_with_repository(
            &repository,
            pending_success(synthetic_account(ACCOUNT_ID_A, DISPLAY_NAME_A, EMAIL_A)),
        )
        .await
        .unwrap();

        assert_eq!(info.id, ACCOUNT_ID_A);
        assert!(info.is_active);
        assert_eq!(info.auth_mode, AuthMode::ChatGPT);
        assert!(paths.metadata_file.exists());
        assert!(paths.vault_file.exists());

        let metadata = load_metadata(&paths);
        assert_eq!(metadata.accounts.len(), 1);
        assert_eq!(metadata.active_account_id.as_deref(), Some(ACCOUNT_ID_A));
        assert_eq!(metadata.accounts[0].auth_kind, MetadataAuthKind::ChatGpt);

        let vault = load_vault(&paths);
        assert_eq!(vault.len(), 1);
        assert!(matches!(
            vault.get(ACCOUNT_ID_A),
            Some(SecretRecord::ChatGpt { .. })
        ));

        drop(vault);
        drop(metadata);
        drop(info);
        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_oauth_tokens_exist_only_in_decrypted_vault() {
        let (root, paths) = test_paths("vault_secrets");
        let repository = AccountRepository::for_test(paths.clone());

        complete_pending_login_with_repository(
            &repository,
            pending_success(synthetic_account(ACCOUNT_ID_A, DISPLAY_NAME_A, EMAIL_A)),
        )
        .await
        .unwrap();

        let metadata_bytes = std::fs::read(&paths.metadata_file).unwrap();
        for secret in [
            ID_TOKEN_A,
            ACCESS_TOKEN_A,
            REFRESH_TOKEN_A,
            CHATGPT_ACCOUNT_A,
        ] {
            assert!(!metadata_bytes
                .windows(secret.len())
                .any(|window| window == secret.as_bytes()));
        }

        let vault = load_vault(&paths);
        match vault.get(ACCOUNT_ID_A).unwrap() {
            SecretRecord::ChatGpt {
                id_token,
                access_token,
                refresh_token,
                account_id,
            } => {
                assert_eq!(id_token, ID_TOKEN_A);
                assert_eq!(access_token, ACCESS_TOKEN_A);
                assert_eq!(refresh_token, REFRESH_TOKEN_A);
                assert_eq!(account_id.as_deref(), Some(CHATGPT_ACCOUNT_A));
            }
            SecretRecord::ApiKey { .. } => panic!("OAuth account was stored as API key"),
        }

        drop(vault);
        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_oauth_metadata_contains_no_token_fields() {
        let (root, paths) = test_paths("metadata_fields");
        let repository = AccountRepository::for_test(paths.clone());

        complete_pending_login_with_repository(
            &repository,
            pending_success(synthetic_account(ACCOUNT_ID_A, DISPLAY_NAME_A, EMAIL_A)),
        )
        .await
        .unwrap();

        let metadata_text =
            String::from_utf8(std::fs::read(&paths.metadata_file).unwrap()).unwrap();
        for field in [
            "id_token",
            "access_token",
            "refresh_token",
            "account_id",
            "api_key",
            "tokens",
        ] {
            assert!(!metadata_text.contains(&format!("\"{field}\"")));
        }

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_oauth_account_info_contains_no_credentials() {
        let (root, paths) = test_paths("account_info_secrets");
        let repository = AccountRepository::for_test(paths.clone());

        let info = complete_pending_login_with_repository(
            &repository,
            pending_success(synthetic_account(ACCOUNT_ID_A, DISPLAY_NAME_A, EMAIL_A)),
        )
        .await
        .unwrap();
        let serialized = serde_json::to_string(&info).unwrap();

        for secret in [
            ID_TOKEN_A,
            ACCESS_TOKEN_A,
            REFRESH_TOKEN_A,
            CHATGPT_ACCOUNT_A,
        ] {
            assert!(!serialized.contains(secret));
        }

        drop(info);
        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_first_oauth_account_uses_repository_active_semantics() {
        let (root, paths) = test_paths("first_active");
        let repository = AccountRepository::for_test(paths.clone());

        let info = complete_pending_login_with_repository(
            &repository,
            pending_success(synthetic_account(ACCOUNT_ID_A, DISPLAY_NAME_A, EMAIL_A)),
        )
        .await
        .unwrap();

        assert!(info.is_active);
        assert!(info.last_used_at.is_none());
        let metadata = load_metadata(&paths);
        assert_eq!(metadata.active_account_id.as_deref(), Some(ACCOUNT_ID_A));
        assert!(metadata.accounts[0].last_used_at.is_none());

        drop(metadata);
        drop(info);
        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_second_oauth_account_preserves_existing_active_account() {
        let (root, paths) = test_paths("second_active");
        let repository = AccountRepository::for_test(paths.clone());

        let first = complete_pending_login_with_repository(
            &repository,
            pending_success(synthetic_account(ACCOUNT_ID_A, DISPLAY_NAME_A, EMAIL_A)),
        )
        .await
        .unwrap();
        let second = complete_pending_login_with_repository(
            &repository,
            pending_success(synthetic_account(ACCOUNT_ID_B, DISPLAY_NAME_B, EMAIL_B)),
        )
        .await
        .unwrap();

        assert!(first.is_active);
        assert!(!second.is_active);
        let metadata = load_metadata(&paths);
        assert_eq!(metadata.active_account_id.as_deref(), Some(ACCOUNT_ID_A));
        assert_eq!(metadata.accounts.len(), 2);
        assert!(metadata
            .accounts
            .iter()
            .all(|account| account.last_used_at.is_none()));

        drop(metadata);
        drop(first);
        drop(second);
        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_oauth_insertion_order_is_preserved() {
        let (root, paths) = test_paths("insertion_order");
        let repository = AccountRepository::for_test(paths.clone());

        complete_pending_login_with_repository(
            &repository,
            pending_success(synthetic_account(ACCOUNT_ID_A, DISPLAY_NAME_A, EMAIL_A)),
        )
        .await
        .unwrap();
        complete_pending_login_with_repository(
            &repository,
            pending_success(synthetic_account(ACCOUNT_ID_B, DISPLAY_NAME_B, EMAIL_B)),
        )
        .await
        .unwrap();

        let metadata = load_metadata(&paths);
        let ids: Vec<_> = metadata
            .accounts
            .iter()
            .map(|account| account.id.as_str())
            .collect();
        assert_eq!(ids, vec![ACCOUNT_ID_A, ACCOUNT_ID_B]);

        drop(metadata);
        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_duplicate_oauth_display_name_returns_sanitized_repository_error() {
        let (root, paths) = test_paths("duplicate_name_error");
        let repository = AccountRepository::for_test(paths.clone());

        complete_pending_login_with_repository(
            &repository,
            pending_success(synthetic_account(ACCOUNT_ID_A, DISPLAY_NAME_A, EMAIL_A)),
        )
        .await
        .unwrap();
        let error = complete_pending_login_with_repository(
            &repository,
            pending_success(synthetic_account(ACCOUNT_ID_B, DISPLAY_NAME_A, EMAIL_B)),
        )
        .await
        .unwrap_err();

        assert_eq!(error, "Duplicate display name");
        assert_sanitized(&error, &root);

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_duplicate_oauth_display_name_preserves_pair_bytes() {
        let (root, paths) = test_paths("duplicate_name_bytes");
        let repository = AccountRepository::for_test(paths.clone());

        complete_pending_login_with_repository(
            &repository,
            pending_success(synthetic_account(ACCOUNT_ID_A, DISPLAY_NAME_A, EMAIL_A)),
        )
        .await
        .unwrap();
        let before = read_pair(&paths);

        let result = complete_pending_login_with_repository(
            &repository,
            pending_success(synthetic_account(ACCOUNT_ID_B, DISPLAY_NAME_A, EMAIL_B)),
        )
        .await;
        assert_eq!(result.unwrap_err(), "Duplicate display name");
        assert_eq!(read_pair(&paths), before);

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_legacy_oauth_completion_returns_migration_required() {
        let (root, paths) = test_paths("legacy_rejected");
        let legacy_bytes = write_legacy_store(&paths);
        let repository = AccountRepository::for_test(paths.clone());

        let error = complete_pending_login_with_repository(
            &repository,
            pending_success(synthetic_account(ACCOUNT_ID_B, DISPLAY_NAME_B, EMAIL_B)),
        )
        .await
        .unwrap_err();

        assert_eq!(error, "Legacy account storage requires secure migration");
        assert_eq!(std::fs::read(&paths.metadata_file).unwrap(), legacy_bytes);
        assert!(!paths.vault_file.exists());

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_legacy_oauth_rejection_preserves_metadata_bytes() {
        let (root, paths) = test_paths("legacy_metadata_bytes");
        let legacy_bytes = write_legacy_store(&paths);
        let repository = AccountRepository::for_test(paths.clone());

        let result = complete_pending_login_with_repository(
            &repository,
            pending_success(synthetic_account(ACCOUNT_ID_B, DISPLAY_NAME_B, EMAIL_B)),
        )
        .await;
        assert_eq!(
            result.unwrap_err(),
            "Legacy account storage requires secure migration"
        );
        assert_eq!(std::fs::read(&paths.metadata_file).unwrap(), legacy_bytes);

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_legacy_oauth_rejection_preserves_orphan_vault_bytes() {
        let (root, paths) = test_paths("legacy_orphan_vault");
        write_legacy_store(&paths);
        let orphan_bytes = write_orphan_vault(&paths);
        let repository = AccountRepository::for_test(paths.clone());

        let result = complete_pending_login_with_repository(
            &repository,
            pending_success(synthetic_account(ACCOUNT_ID_B, DISPLAY_NAME_B, EMAIL_B)),
        )
        .await;
        assert_eq!(
            result.unwrap_err(),
            "Legacy account storage requires secure migration"
        );
        assert_eq!(std::fs::read(&paths.vault_file).unwrap(), orphan_bytes);

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_failed_oauth_provider_result_creates_no_files() {
        let (root, paths) = test_paths("provider_failure_files");
        let repository = AccountRepository::for_test(paths.clone());
        let error_message = format!(
            "provider failure {ID_TOKEN_A} {ACCESS_TOKEN_A} {REFRESH_TOKEN_A} {CHATGPT_ACCOUNT_A}"
        );

        let error = complete_pending_login_with_repository(
            &repository,
            pending_provider_failure(error_message),
        )
        .await
        .unwrap_err();

        assert_eq!(error, "OAuth login failed");
        assert!(!paths.metadata_file.exists());
        assert!(!paths.vault_file.exists());
        assert!(!paths.operation_lock_file.exists());

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_oauth_provider_failure_error_is_exactly_sanitized() {
        let (root, paths) = test_paths("provider_failure_error");
        let repository = AccountRepository::for_test(paths.clone());

        let error = complete_pending_login_with_repository(
            &repository,
            pending_provider_failure(format!(
                "failure includes {DISPLAY_NAME_A} {EMAIL_A} {ID_TOKEN_A}"
            )),
        )
        .await
        .unwrap_err();

        assert_eq!(error, "OAuth login failed");
        assert_sanitized(&error, &root);

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_oauth_receiver_failure_error_excludes_synthetic_values() {
        let (root, paths) = test_paths("receiver_failure");
        let repository = AccountRepository::for_test(paths.clone());

        let error = complete_pending_login_with_repository(&repository, pending_receiver_failure())
            .await
            .unwrap_err();

        assert_eq!(error, "OAuth login failed");
        assert_sanitized(&error, &root);
        assert!(!paths.metadata_file.exists());
        assert!(!paths.vault_file.exists());

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_no_pending_oauth_returns_exact_error() {
        let _serial = pending_test_serial().lock().await;
        let _reset = PendingReset;
        clear_pending();

        let (root, paths) = test_paths("no_pending");
        let repository = AccountRepository::for_test(paths.clone());

        let error = complete_login_with_repository(&repository)
            .await
            .unwrap_err();
        assert_eq!(error, "No pending OAuth login");
        assert!(!paths.metadata_file.exists());
        assert!(!paths.vault_file.exists());

        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_pending_oauth_is_consumed_once() {
        let _serial = pending_test_serial().lock().await;
        let _reset = PendingReset;
        clear_pending();

        let (root, paths) = test_paths("pending_once");
        let repository = AccountRepository::for_test(paths.clone());
        {
            let mut pending = PENDING_OAUTH.lock().unwrap();
            *pending = Some(pending_success(synthetic_account(
                ACCOUNT_ID_A,
                DISPLAY_NAME_A,
                EMAIL_A,
            )));
        }

        let first = complete_login_with_repository(&repository).await.unwrap();
        assert_eq!(first.id, ACCOUNT_ID_A);
        let second = complete_login_with_repository(&repository).await;
        assert_eq!(second.unwrap_err(), "No pending OAuth login");

        drop(first);
        drop(repository);
        cleanup(root);
    }

    #[tokio::test]
    async fn test_concurrent_unique_oauth_additions_do_not_lose_accounts() {
        let (root, paths) = test_paths("concurrent_adds");
        let repository_a = AccountRepository::for_test(paths.clone());
        let repository_b = AccountRepository::for_test(paths.clone());

        let (result_a, result_b) = tokio::join!(
            complete_pending_login_with_repository(
                &repository_a,
                pending_success(synthetic_account(ACCOUNT_ID_A, DISPLAY_NAME_A, EMAIL_A)),
            ),
            complete_pending_login_with_repository(
                &repository_b,
                pending_success(synthetic_account(ACCOUNT_ID_B, DISPLAY_NAME_B, EMAIL_B)),
            ),
        );
        assert!(result_a.is_ok(), "first concurrent OAuth add failed");
        assert!(result_b.is_ok(), "second concurrent OAuth add failed");

        let metadata = load_metadata(&paths);
        let vault = load_vault(&paths);
        let ids: Vec<_> = metadata
            .accounts
            .iter()
            .map(|account| account.id.as_str())
            .collect();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&ACCOUNT_ID_A));
        assert!(ids.contains(&ACCOUNT_ID_B));
        assert_eq!(vault.len(), 2);
        assert!(vault.contains(ACCOUNT_ID_A));
        assert!(vault.contains(ACCOUNT_ID_B));

        drop(vault);
        drop(metadata);
        drop(repository_a);
        drop(repository_b);
        cleanup(root);
    }
}
