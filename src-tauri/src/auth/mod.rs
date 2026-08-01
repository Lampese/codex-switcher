//! Authentication module

mod account_store;
pub mod oauth_server;
pub mod storage;
pub mod switcher;
pub mod token_refresh;

pub use oauth_server::*;
pub use storage::*;
pub use switcher::*;
pub use token_refresh::*;

pub(crate) use account_store::{
    default_client, initialize_default_store, AccountStoreRuntimeGuard,
};
