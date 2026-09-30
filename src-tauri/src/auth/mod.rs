//! Authentication module

pub mod oauth_server;
pub mod storage;
pub mod switcher;
pub mod token_refresh;

// ponytail: refreshes are rare; one global lock keeps auth.json and accounts.json ordered.
pub(crate) static AUTH_OPERATION_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub use oauth_server::*;
pub use storage::*;
pub use switcher::*;
pub use token_refresh::*;
