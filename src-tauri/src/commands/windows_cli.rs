//! Desktop launches do not inherit the CLI path injected into Codex agent shells.
//! Prefer the running backend's version, then installed CLI builds (newest first).

use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub(super) fn resolve(local_data: &Path, running: &[PathBuf]) -> Option<PathBuf> {
    let bin = local_data.join("OpenAI/Codex/bin");
    for path in running {
        if is_installed_cli(&bin, path) && path.is_file() {
            return Some(path.clone());
        }
    }

    let mut installed = Vec::new();
    let direct = bin.join("codex.exe");
    if direct.is_file() {
        installed.push(direct);
    }
    if let Ok(entries) = std::fs::read_dir(&bin) {
        for entry in entries.flatten() {
            // Only the documented one-level version cache, not arbitrary executables.
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                let path = entry.path().join("codex.exe");
                if path.is_file() {
                    installed.push(path);
                }
            }
        }
    }
    installed.sort_by_cached_key(|path| {
        std::cmp::Reverse((
            std::fs::metadata(path)
                .and_then(|meta| meta.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH),
            path.clone(),
        ))
    });
    installed.into_iter().next()
}

fn is_installed_cli(bin: &Path, path: &Path) -> bool {
    // Windows process snapshots may differ in case or separator spelling.
    let normalize = |path: &Path| {
        path.to_string_lossy()
            .replace('\\', "/")
            .to_ascii_lowercase()
    };
    let bin = normalize(bin);
    let path = normalize(path);
    let Some(relative) = path.strip_prefix(&format!("{}/", bin.trim_end_matches('/'))) else {
        return false;
    };
    let parts: Vec<_> = relative.split('/').collect();
    parts.last() == Some(&"codex.exe")
        && (parts.len() == 1 || parts.len() == 2)
        && parts.iter().all(|part| !matches!(*part, "" | "." | ".."))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (crate::auth::test_support::TestHome, PathBuf) {
        let home = crate::auth::test_support::TestHome::new();
        let local = crate::auth::test_support::home_dir()
            .unwrap()
            .join("Local App Data");
        std::fs::create_dir_all(&local).unwrap();
        (home, local)
    }

    fn cli(local: &Path, version: &str, modified: u64) -> PathBuf {
        let path = local
            .join("OpenAI/Codex/bin")
            .join(version)
            .join("codex.exe");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let file = std::fs::File::create(&path).unwrap();
        file.set_times(
            std::fs::FileTimes::new()
                .set_modified(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(modified)),
        )
        .unwrap();
        path
    }

    #[test]
    fn finds_versioned_cli_without_path_and_skips_incomplete_builds() {
        let (_home, local) = fixture();
        let old = cli(&local, "old-build", 10);
        let current = cli(&local, "current-build", 20);
        std::fs::create_dir_all(local.join("OpenAI/Codex/bin/incomplete-build")).unwrap();
        assert_eq!(resolve(&local, &[]), Some(current.clone()));
        assert_eq!(resolve(&local, &[old.clone()]), Some(old));
        assert_eq!(
            resolve(&local, &[local.join("missing/codex.exe")]),
            Some(current)
        );
    }

    #[test]
    fn supports_direct_cli_and_absent_installations() {
        let (_home, local) = fixture();
        assert_eq!(resolve(&local, &[]), None);
        let direct = cli(&local, "", 10);
        assert_eq!(resolve(&local, &[]), Some(direct));
    }

    #[test]
    fn running_candidates_cannot_select_gui_helpers_or_unrelated_executables() {
        let bin = Path::new(r"C:\Users\Test\AppData\Local\OpenAI\Codex\bin");
        assert!(is_installed_cli(
            bin,
            Path::new(r"c:/users/test/appdata/local/openai/codex/bin/build/codex.exe")
        ));
        for path in [
            r"C:\Other\codex.exe",
            r"C:\Users\Test\AppData\Local\OpenAI\Codex\bin-other\codex.exe",
            r"C:\Users\Test\AppData\Local\OpenAI\Codex\bin\build\helper.exe",
            r"C:\Users\Test\AppData\Local\OpenAI\Codex\bin\..\codex.exe",
            r"C:\Users\Test\AppData\Local\OpenAI\Codex\bin\nested\build\codex.exe",
        ] {
            assert!(!is_installed_cli(bin, Path::new(path)), "{path}");
        }
    }
}
