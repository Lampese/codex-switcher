use std::path::{Path, PathBuf};

/// Error returned when application paths cannot be resolved safely.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub(crate) enum AppPathsError {
    #[error("The current user's home directory could not be resolved")]
    HomeDirectoryUnavailable,
}

/// Application path resolution structure for Codex Switcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AppPaths {
    pub(crate) switcher_dir: PathBuf,
    pub(crate) metadata_file: PathBuf,
    pub(crate) vault_file: PathBuf,
    pub(crate) operation_lock_file: PathBuf,
    pub(crate) codex_home: PathBuf,
    pub(crate) codex_auth_file: PathBuf,
}

impl AppPaths {
    /// Production path resolution.
    ///
    /// Resolves paths based on the current user's home directory and `%CODEX_HOME%`.
    /// Fails closed if the user's home directory cannot be resolved.
    pub(crate) fn production() -> Result<Self, AppPathsError> {
        let user_home = dirs::home_dir();
        let codex_home_env = std::env::var("CODEX_HOME").ok();
        Self::resolve_from_parts(user_home.as_deref(), codex_home_env.as_deref())
    }

    /// Construct `AppPaths` deterministically isolated under a test root directory.
    pub(crate) fn for_test(root: &Path) -> Self {
        let switcher_dir = root.join(".codex-switcher");
        let metadata_file = switcher_dir.join("accounts.json");
        let vault_file = switcher_dir.join("vault.dat");
        let operation_lock_file = switcher_dir.join("operation.lock");
        let codex_home = root.join(".codex");
        let codex_auth_file = codex_home.join("auth.json");

        Self {
            switcher_dir,
            metadata_file,
            vault_file,
            operation_lock_file,
            codex_home,
            codex_auth_file,
        }
    }

    /// Deterministic path resolution with injected home directory and optional CODEX_HOME environment string.
    pub(crate) fn resolve_from_parts(
        user_home: Option<&Path>,
        codex_home_env: Option<&str>,
    ) -> Result<Self, AppPathsError> {
        let home = user_home.ok_or(AppPathsError::HomeDirectoryUnavailable)?;

        let switcher_dir = home.join(".codex-switcher");
        let metadata_file = switcher_dir.join("accounts.json");
        let vault_file = switcher_dir.join("vault.dat");
        let operation_lock_file = switcher_dir.join("operation.lock");

        let codex_home = match codex_home_env {
            Some(env_val) => {
                let trimmed = env_val.trim();
                if trimmed.is_empty() {
                    home.join(".codex")
                } else {
                    PathBuf::from(trimmed)
                }
            }
            None => home.join(".codex"),
        };
        let codex_auth_file = codex_home.join("auth.json");

        Ok(Self {
            switcher_dir,
            metadata_file,
            vault_file,
            operation_lock_file,
            codex_home,
            codex_auth_file,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_missing_home_source_fails_closed() {
        let res = AppPaths::resolve_from_parts(None, None);
        assert_eq!(res, Err(AppPathsError::HomeDirectoryUnavailable));
    }

    #[test]
    fn test_no_helper_returns_dot_as_user_home_fallback() {
        let res = AppPaths::resolve_from_parts(None, Some("C:\\Custom\\Codex"));
        assert_eq!(res, Err(AppPathsError::HomeDirectoryUnavailable));
    }

    #[test]
    fn test_whitespace_only_codex_home_uses_default() {
        let home = Path::new("C:\\Users\\testuser");
        let paths = AppPaths::resolve_from_parts(Some(home), Some("   \t  \n  ")).unwrap();

        assert_eq!(paths.codex_home, home.join(".codex"));
        assert_eq!(paths.codex_auth_file, home.join(".codex").join("auth.json"));
    }

    #[test]
    fn test_surrounding_whitespace_on_codex_home_is_removed() {
        let home = Path::new("C:\\Users\\testuser");
        let raw_env = "   C:\\Custom\\CodexHome   ";
        let paths = AppPaths::resolve_from_parts(Some(home), Some(raw_env)).unwrap();

        assert_eq!(paths.codex_home, PathBuf::from("C:\\Custom\\CodexHome"));
        assert_eq!(
            paths.codex_auth_file,
            PathBuf::from("C:\\Custom\\CodexHome").join("auth.json")
        );
    }

    #[test]
    fn test_paths_remain_under_injected_temp_root() {
        let temp_dir = std::env::temp_dir().join("codex_switcher_test_paths_root");
        let paths = AppPaths::for_test(&temp_dir);

        assert!(paths.switcher_dir.starts_with(&temp_dir));
        assert!(paths.metadata_file.starts_with(&temp_dir));
        assert!(paths.vault_file.starts_with(&temp_dir));
        assert!(paths.operation_lock_file.starts_with(&temp_dir));
        assert!(paths.codex_home.starts_with(&temp_dir));
        assert!(paths.codex_auth_file.starts_with(&temp_dir));
    }

    #[test]
    fn test_codex_auth_path_derived_from_injected_test_root() {
        let temp_dir = std::env::temp_dir().join("codex_switcher_test_auth_derived");
        let paths = AppPaths::for_test(&temp_dir);

        assert_eq!(paths.codex_home, temp_dir.join(".codex"));
        assert_eq!(
            paths.codex_auth_file,
            temp_dir.join(".codex").join("auth.json")
        );
    }

    #[test]
    fn test_no_test_path_points_at_user_home() {
        let temp_dir = std::env::temp_dir().join("codex_switcher_test_no_user_home");
        let paths = AppPaths::for_test(&temp_dir);

        if let Some(home) = dirs::home_dir() {
            assert_ne!(paths.switcher_dir, home.join(".codex-switcher"));
            assert_ne!(paths.codex_home, home.join(".codex"));
            assert_ne!(
                paths.metadata_file,
                home.join(".codex-switcher").join("accounts.json")
            );
        }
    }
}
