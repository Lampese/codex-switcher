//! Codex CLI app-server daemon support.
//! CLI sessions share one daemon that reads auth.json only when it starts.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context};

use crate::auth::get_codex_home;

/// Whether the managed CLI daemon for the current Codex home is running.
pub(crate) fn is_cli_daemon_running() -> bool {
    get_codex_home()
        .ok()
        .and_then(|codex_home| running_daemon_pid(&codex_home))
        .is_some()
}

/// Stop the managed CLI daemon so it cannot keep the previous account.
/// The next CLI session starts a new daemon from its own environment.
/// Returns false when no daemon is running.
pub(crate) fn stop_cli_daemon_if_running() -> anyhow::Result<bool> {
    let codex_home = get_codex_home()?;
    if running_daemon_pid(&codex_home).is_none() {
        return Ok(false);
    }

    let output = Command::new(managed_codex_path(&codex_home))
        .args(["app-server", "daemon", "stop"])
        .env("CODEX_HOME", &codex_home)
        .output()
        .context("Failed to run codex app-server daemon stop")?;
    if !output.status.success() {
        bail!(
            "codex app-server daemon stop failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    Ok(true)
}

fn running_daemon_pid(codex_home: &Path) -> Option<u32> {
    let pid_file = codex_home.join("app-server-daemon").join("daemon.pid");
    let pid = parse_daemon_pid(&std::fs::read_to_string(pid_file).ok()?)?;
    let command = read_process_command(pid)?;
    is_managed_daemon_command(&command).then_some(pid)
}

fn managed_codex_path(codex_home: &Path) -> PathBuf {
    codex_home
        .join("packages")
        .join("app-server-daemon")
        .join("current")
        .join("bin")
        .join("codex")
}

#[cfg(unix)]
fn read_process_command(pid: u32) -> Option<String> {
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "command="])
        .output()
        .ok()?;
    let command = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (output.status.success() && !command.is_empty()).then_some(command)
}

// The daemon listens on a Unix socket; other platforms are not detected yet.
#[cfg(not(unix))]
fn read_process_command(_pid: u32) -> Option<String> {
    None
}

fn parse_daemon_pid(contents: &str) -> Option<u32> {
    let value: serde_json::Value = serde_json::from_str(contents).ok()?;
    let pid = u32::try_from(value.get("pid")?.as_u64()?).ok()?;
    (pid > 0).then_some(pid)
}

fn is_managed_daemon_command(command: &str) -> bool {
    let mut tokens = command.split_whitespace();
    tokens.clone().any(|token| token == "app-server")
        && tokens.any(|token| token == "--managed-daemon")
}

#[cfg(test)]
mod tests {
    use super::{is_managed_daemon_command, parse_daemon_pid};

    const RELEASE_BIN: &str =
        "/Users/me/.codex/packages/app-server-daemon/releases/0.160.0-aarch64-apple-darwin/bin/codex";

    #[test]
    fn reads_the_pid_from_the_daemon_pid_file() {
        let contents = r#"{"pid":99744,"processStartTime":"Fri Oct  2 03:58:07 2026","processIdentity":{"uniqueId":10620751}}"#;
        assert_eq!(parse_daemon_pid(contents), Some(99744));
    }

    #[test]
    fn rejects_an_unreadable_daemon_pid_file() {
        for contents in [
            "",
            "not json",
            r#"{"pid":"99744"}"#,
            r#"{"pid":0}"#,
            r#"{}"#,
        ] {
            assert_eq!(parse_daemon_pid(contents), None, "{contents}");
        }
    }

    #[test]
    fn detects_the_managed_daemon_command() {
        let command = format!("{RELEASE_BIN} app-server --listen unix:// --managed-daemon");
        assert!(is_managed_daemon_command(&command));
    }

    #[test]
    fn ignores_other_codex_commands() {
        for command in [
            format!("{RELEASE_BIN} app-server daemon pid-update-loop"),
            "/opt/codex-acp/node_modules/@openai/codex-darwin-arm64/vendor/aarch64-apple-darwin/bin/codex app-server".to_string(),
            "/Applications/ChatGPT.app/Contents/Resources/codex app-server".to_string(),
            "codex --managed-daemon-docs".to_string(),
            "codex".to_string(),
            String::new(),
        ] {
            assert!(!is_managed_daemon_command(&command), "{command}");
        }
    }
}
