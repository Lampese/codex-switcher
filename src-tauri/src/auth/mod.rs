//! Authentication module

pub mod oauth_server;
pub mod storage;
pub mod switcher;
pub mod token_refresh;

#[cfg(test)]
pub(crate) mod test_support;

// ponytail: refreshes are rare; one global lock keeps auth.json and accounts.json ordered.
pub(crate) static AUTH_OPERATION_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub use oauth_server::*;
pub use storage::*;
pub use switcher::*;
pub use token_refresh::*;

/// Background quota checks should yield to a user-requested login or switch.
/// An occupied lock means no refresh request has been sent, so the caller can
/// retry its usage query at the next poll without replaying an OAuth POST.
pub(crate) fn try_acquire_background_auth_operation_lock(
) -> anyhow::Result<tokio::sync::MutexGuard<'static, ()>> {
    AUTH_OPERATION_LOCK
        .try_lock()
        .map_err(|_| anyhow::anyhow!("Background OAuth refresh busy; retry next poll"))
}

#[cfg(test)]
mod background_auth_lock_tests {
    use super::{try_acquire_background_auth_operation_lock, AUTH_OPERATION_LOCK};

    #[tokio::test(flavor = "current_thread")]
    async fn background_refresh_does_not_queue_behind_user_operation() {
        let guard = AUTH_OPERATION_LOCK.lock().await;
        let busy = try_acquire_background_auth_operation_lock()
            .err()
            .expect("busy background operation should be skipped");
        assert!(busy.to_string().contains("Background OAuth refresh busy"));
        drop(guard);
        assert!(try_acquire_background_auth_operation_lock().is_ok());
    }
}
