//! Authentication module

pub(crate) mod atomic_file;
pub(crate) mod dpapi;
pub(crate) mod metadata_store;
pub(crate) mod migration;
pub mod oauth_server;
pub(crate) mod operation_lock;
pub(crate) mod paths;
pub(crate) mod secure_commit;
pub mod storage;
pub mod switcher;
pub mod token_refresh;
pub(crate) mod vault;

pub use oauth_server::*;
pub use storage::*;
pub use switcher::*;
pub use token_refresh::*;
