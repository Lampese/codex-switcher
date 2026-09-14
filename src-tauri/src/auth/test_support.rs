//! Thread-local filesystem isolation for current-thread async command tests.

use anyhow::{Context, Result};
use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;

thread_local! {
    static TEST_HOME: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

pub(crate) fn home_dir() -> Result<PathBuf> {
    TEST_HOME
        .with(|home| home.borrow().clone())
        .context("TestHome must be installed before filesystem access")
}

pub(crate) struct TestHome {
    root: PathBuf,
}

impl TestHome {
    pub(crate) fn new() -> Self {
        assert!(home_dir().is_err(), "test homes must not be nested");
        let root =
            std::env::temp_dir().join(format!("codex-switcher-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        TEST_HOME.with(|home| *home.borrow_mut() = Some(root.clone()));
        Self { root }
    }
}

impl Drop for TestHome {
    fn drop(&mut self) {
        TEST_HOME.with(|home| *home.borrow_mut() = None);
        // Only remove the unique temporary directory created by this fixture.
        let _ = fs::remove_dir_all(&self.root);
    }
}
