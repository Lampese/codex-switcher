//! Auto Recovery and Session Automation for Codex CLI
//!
//! Automatically handles:
//! 1. Model capacity / server overloaded errors via progressive `codex queue` retries
//!    and escalation to account switching.
//! 2. Usage limit reached errors via smart account selection, graceful session termination,
//!    account credential swap, and terminal session relaunch.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::process::Stdio;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::auth::{
    ensure_chatgpt_tokens_fresh_locked, load_accounts, load_app_settings, read_current_auth,
    save_accounts, save_app_settings, switch_to_account, sync_active_account_tokens,
    AUTH_OPERATION_LOCK,
};
use crate::commands::account_stats::AccountResetCredits;
use crate::types::{
    AppSettings, AutoSwitchStrategy, StoredAccount, UsageInfo,
};

/// Type of error detected in a Codex session
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionErrorKind {
    UsageLimitExceeded,
    ServerOverloaded,
}

/// Detected error details from a session log
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectedSessionError {
    pub kind: SessionErrorKind,
    pub session_id: String,
    pub turn_id: Option<String>,
    pub message: String,
    pub detected_at: DateTime<Utc>,
}

/// Information about an active Codex session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveCodexSession {
    pub session_id: String,
    pub pid: u32,
    pub cwd: Option<String>,
    pub rollout_path: Option<String>,
    pub last_error: Option<DetectedSessionError>,
    pub is_managed: bool,
    pub is_desktop: bool,
}

/// Status of the auto-recovery monitor
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoRecoveryStatus {
    pub active_sessions_count: usize,
    pub monitored_sessions: Vec<ActiveCodexSession>,
    pub last_recovery_event: Option<RecoveryEventNotification>,
}

/// Recovery notification sent to the UI
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryEventNotification {
    pub event_type: String,
    pub session_id: String,
    pub message: String,
    pub timestamp: DateTime<Utc>,
}

/// Session tracking state
struct SessionTrackerState {
    /// Tracks retry attempts for a session: session_id -> (turn_id, attempt_count, last_attempt_time)
    capacity_retries: HashMap<String, (Option<String>, u32, Instant)>,
    /// Tracks handled usage limits to prevent duplicate triggers: session_id -> handled_turn_id
    handled_usage_limits: HashMap<String, String>,
    /// PIDs spawned directly by switcher
    managed_pids: Vec<u32>,
    /// Last recovery event notification
    last_event: Option<RecoveryEventNotification>,
    /// Last account switch timestamp, target account ID, was_reset_credit, and the session_id that triggered it
    last_account_switch: Option<(Instant, String, bool, String)>,
}

static TRACKER: std::sync::LazyLock<Mutex<SessionTrackerState>> =
    std::sync::LazyLock::new(|| {
        Mutex::new(SessionTrackerState {
            capacity_retries: HashMap::new(),
            handled_usage_limits: HashMap::new(),
            managed_pids: Vec::new(),
            last_event: None,
            last_account_switch: None,
        })
    });

fn which_cmd(cmd: &str) -> Option<PathBuf> {
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let full = dir.join(cmd);
            if full.is_file() {
                return Some(full);
            }
        }
    }
    None
}

fn escape_shell_arg(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(target_os = "macos")]
fn macos_resume_command(
    codex_bin: &Path,
    cwd: &Path,
    session_id: &str,
    phrase: &str,
    restart_file: &Path,
) -> String {
    let cwd = escape_shell_arg(&cwd.to_string_lossy());
    let binary = escape_shell_arg(&codex_bin.to_string_lossy());
    let session = escape_shell_arg(session_id);
    let phrase_arg = if phrase == "/goal resume" || phrase.is_empty() {
        String::new()
    } else {
        format!(" {}", escape_shell_arg(phrase))
    };
    let restart = escape_shell_arg(&restart_file.to_string_lossy());
    format!(
        "cd {cwd} && while true; do {binary} resume {session}{phrase_arg}; if [ -f {restart} ]; then rm -f {restart}; sleep 1; continue; fi; break; done; exit"
    )
}

/// Find the codex CLI executable in standard and user environments
pub fn find_codex_binary() -> PathBuf {
    if let Some(path) = which_cmd("codex") {
        return path;
    }

    if let Some(home) = dirs::home_dir() {
        // Common NVM locations - sort descending so newest node/codex (e.g. v24 > v22) is chosen
        let nvm_pattern = home.join(".nvm/versions/node");
        if let Ok(entries) = fs::read_dir(&nvm_pattern) {
            let mut dirs: Vec<PathBuf> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
            dirs.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
            for dir in dirs {
                let bin = dir.join("bin/codex");
                if bin.is_file() {
                    return bin;
                }
            }
        }

        // Local bin
        let local_bin = home.join(".local/bin/codex");
        if local_bin.is_file() {
            return local_bin;
        }

        // NPM global bin
        let npm_bin = home.join(".npm-global/bin/codex");
        if npm_bin.is_file() {
            return npm_bin;
        }
    }

    // Standard Unix fallbacks
    for path in &["/usr/local/bin/codex", "/usr/bin/codex", "/opt/homebrew/bin/codex"] {
        let p = PathBuf::from(path);
        if p.is_file() {
            return p;
        }
    }

    PathBuf::from("codex")
}

/// Find local Codex sessions by inspecting thread-writer-locks and running processes.
pub fn find_active_sessions() -> Result<Vec<ActiveCodexSession>> {
    let mut sessions = Vec::new();
    let home = dirs::home_dir().context("Could not determine home directory")?;
    let locks_dir = home.join(".codex/thread-writer-locks");

    if !locks_dir.exists() {
        return Ok(sessions);
    }

    let Ok(entries) = fs::read_dir(&locks_dir) else {
        return Ok(sessions);
    };

    let managed_pids = {
        let Ok(state) = TRACKER.lock() else {
            return Ok(sessions);
        };
        state.managed_pids.clone()
    };

    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();

        if !name.ends_with(".lock") || name.starts_with('.') {
            continue;
        }

        let session_id = name.trim_end_matches(".lock").to_string();
        if session_id.is_empty() {
            continue;
        }

        // Determine PID holding or associated with this session
        let cli_pid = find_pid_for_session(&session_id, &path);
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        let desktop_pid = find_desktop_pid_for_session(&path);
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        let desktop_pid: Option<u32> = None;
        let is_desktop = cli_pid.is_none() && desktop_pid.is_some();
        let pid = cli_pid.or(desktop_pid).unwrap_or(0);
        if pid == 0 {
            continue;
        }
        let rollout_path = locate_rollout_file(&home, &session_id);

        // Inspect rollout metadata: if this is a subagent (child thread of another session),
        // skip it! Only top-level root sessions should be monitored and resumed as terminals.
        let rollout_meta = rollout_path.as_deref().and_then(inspect_session_meta);
        if let Some(ref meta) = rollout_meta {
            if meta.parent_thread_id.is_some() {
                // Subagent session - owned and managed by parent session, do not manage directly
                continue;
            }
        }

        // Canonical working directory: prefer rollout metadata, fallback to process cwd
        let cwd = rollout_meta
            .as_ref()
            .and_then(|m| m.cwd.clone())
            .or_else(|| if pid > 0 { get_process_cwd(pid) } else { None });

        let is_managed = pid > 0 && managed_pids.contains(&pid);

        let mut session = ActiveCodexSession {
            session_id: session_id.clone(),
            pid,
            cwd,
            rollout_path: rollout_path.as_ref().map(|p| p.to_string_lossy().to_string()),
            last_error: None,
            is_managed,
            is_desktop,
        };

        if let Some(ref r_path) = rollout_path {
            session.last_error = check_rollout_for_errors(r_path, &session_id);
        }

        sessions.push(session);
    }

    Ok(sessions)
}

/// Metadata extracted from session rollout file
#[derive(Debug, Default)]
struct SessionRolloutMeta {
    parent_thread_id: Option<String>,
    cwd: Option<String>,
}

/// Inspect the initial lines of a rollout to determine if it is a subagent and extract its true project cwd
fn inspect_session_meta(rollout_path: &Path) -> Option<SessionRolloutMeta> {
    use std::io::{BufRead, BufReader};
    let file = fs::File::open(rollout_path).ok()?;
    let reader = BufReader::new(file);

    let mut parent_thread_id = None;
    let mut cwd = None;

    for line in reader.lines().take(50).filter_map(Result::ok) {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&line) {
            let payload = val.get("payload");
            if parent_thread_id.is_none() {
                if let Some(pid) = payload
                    .and_then(|p| p.get("parent_thread_id"))
                    .and_then(|id| id.as_str())
                    .filter(|id| !id.is_empty())
                {
                    parent_thread_id = Some(pid.to_string());
                }
            }

            if cwd.is_none() {
                if let Some(c) = payload
                    .and_then(|p| p.get("cwd"))
                    .or_else(|| payload.and_then(|p| p.get("thread_settings")).and_then(|ts| ts.get("cwd")))
                    .or_else(|| {
                        payload
                            .and_then(|p| p.get("state"))
                            .and_then(|s| s.get("environments"))
                            .and_then(|e| e.get("environments"))
                            .and_then(|e| e.get("local"))
                            .and_then(|l| l.get("cwd"))
                    })
                    .and_then(|c| c.as_str())
                    .filter(|c| !c.is_empty())
                {
                    cwd = Some(c.to_string());
                }
            }

            if parent_thread_id.is_some() && cwd.is_some() {
                break;
            }
        }
    }

    Some(SessionRolloutMeta {
        parent_thread_id,
        cwd,
    })
}

/// Locate rollout jsonl file for a session ID in ~/.codex/sessions/
pub fn locate_rollout_file(codex_home: &Path, session_id: &str) -> Option<PathBuf> {
    let base = codex_home.join(".codex/sessions");
    if !base.exists() {
        return None;
    }

    let target_needle = format!("{session_id}.jsonl");

    // Scan years/months/days backwards from current date to minimize disk I/O
    let mut years = fs::read_dir(&base).ok()?.filter_map(Result::ok).collect::<Vec<_>>();
    years.sort_by_key(|e| e.file_name());
    years.reverse();

    for year in years {
        let mut months = fs::read_dir(year.path()).ok()?.filter_map(Result::ok).collect::<Vec<_>>();
        months.sort_by_key(|e| e.file_name());
        months.reverse();

        for month in months {
            let mut days = fs::read_dir(month.path()).ok()?.filter_map(Result::ok).collect::<Vec<_>>();
            days.sort_by_key(|e| e.file_name());
            days.reverse();

            for day in days {
                if let Ok(files) = fs::read_dir(day.path()) {
                    for file in files.filter_map(Result::ok) {
                        let name = file.file_name().to_string_lossy().to_string();
                        if name.contains(&target_needle) {
                            return Some(file.path());
                        }
                    }
                }
            }
        }
    }

    None
}

/// Find PID associated with a session ID via /proc or lsof
fn find_pid_for_session(session_id: &str, file_path: &Path) -> Option<u32> {
    #[cfg(target_os = "linux")]
    {
        // Check /proc/[pid]/cmdline for session ID without needing root permissions
        if let Ok(entries) = fs::read_dir("/proc") {
            for entry in entries.filter_map(Result::ok) {
                let name = entry.file_name();
                if let Ok(pid) = name.to_string_lossy().parse::<u32>() {
                    let cmdline_path = entry.path().join("cmdline");
                    if let Ok(cmdline_bytes) = fs::read(&cmdline_path) {
                        let cmdline = String::from_utf8_lossy(&cmdline_bytes);
                        if cmdline.contains(session_id)
                            && is_supported_cli_command(&cmdline.replace('\0', " "))
                        {
                            return Some(pid);
                        }
                    }
                }
            }
        }
    }

    #[cfg(unix)]
    {
        if let Ok(output) = Command::new("lsof").arg("-t").arg(file_path).output() {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    if let Ok(pid) = line.trim().parse::<u32>() {
                        let Ok(output) = Command::new("ps")
                            .args(["-p", &pid.to_string(), "-o", "args="])
                            .output() else { continue };
                        if output.status.success()
                            && is_supported_cli_command(&String::from_utf8_lossy(&output.stdout))
                        {
                            return Some(pid);
                        }
                    }
                }
            }
        }
    }

    None
}

fn is_supported_cli_command(command: &str) -> bool {
    let command = command.trim();
    if command.is_empty() {
        return false;
    }
    let lowercase_command = command.to_ascii_lowercase();
    if lowercase_command.contains("codex-switcher")
        || lowercase_command.contains("app-server")
        || lowercase_command.contains("exec-server")
        || lowercase_command.contains("chatgpt.app")
        || lowercase_command.contains("codex.app")
        || lowercase_command.contains("/usr/lib/chatgpt")
        || lowercase_command.contains("/usr/lib/codex")
    {
        return false;
    }
    let first = lowercase_command.split_whitespace().next().unwrap_or("");
    first == "codex" || first.ends_with("/codex")
}

fn rollout_has_active_turn(path: &Path) -> Result<bool> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = fs::File::open(path)?;
    let size = file.metadata()?.len();
    let offset = size.saturating_sub(1024 * 1024);
    file.seek(SeekFrom::Start(offset))?;
    let mut tail = String::new();
    file.read_to_string(&mut tail)?;
    // A missing lifecycle event is uncertain. The caller defers the handoff.
    for line in tail.lines().rev() {
        if !line.contains("\"task_started\"")
            && !line.contains("\"task_complete\"")
            && !line.contains("\"turn_aborted\"")
        {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        match value.pointer("/payload/type").and_then(|kind| kind.as_str()) {
            Some("task_started") => return Ok(true),
            Some("task_complete") | Some("turn_aborted") => return Ok(false),
            _ => {}
        }
    }
    anyhow::bail!("Cannot determine whether another Codex turn is active")
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn desktop_handoff_is_exclusive(session_id: &str) -> Result<()> {
    for other in find_active_sessions()? {
        if other.session_id == session_id {
            continue;
        }
        if !other.is_desktop {
            anyhow::bail!("Another CLI session is open; deferring desktop account handoff");
        }
        let path = other.rollout_path.as_deref().context("Desktop session history is unavailable")?;
        if rollout_has_active_turn(Path::new(path))? {
            anyhow::bail!("Another desktop turn is active; deferring account handoff");
        }
    }
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
async fn close_desktop_for_handoff(session_id: &str) -> Result<String> {
    desktop_handoff_is_exclusive(session_id)?;
    let processes = crate::commands::process::check_codex_processes()
        .await
        .map_err(anyhow::Error::msg)?;
    if processes.count != 1 || !is_desktop_root_pid(processes.pids[0]) {
        anyhow::bail!("Expected one Codex desktop process; deferring account handoff");
    }
    let closed = crate::commands::process::kill_codex_processes(Some(true), Some(false))
        .await
        .map_err(anyhow::Error::msg)?;
    let token = closed.reopen_token.context("Could not record Codex desktop for reopening")?;
    if !closed.failed_pids.is_empty() {
        let _ = crate::commands::reopen_closed_codex_desktop(token).await;
        anyhow::bail!("Codex desktop did not close cleanly; account was not switched");
    }
    Ok(token)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn is_same_or_related_cli_pid(pid1: u32, pid2: u32) -> bool {
    if pid1 == pid2 {
        return true;
    }
    #[cfg(target_os = "linux")]
    {
        let get_ppid = |p: u32| -> Option<u32> {
            let stat = fs::read_to_string(format!("/proc/{p}/stat")).ok()?;
            stat.split_whitespace().nth(3)?.parse().ok()
        };
        if get_ppid(pid1) == Some(pid2) || get_ppid(pid2) == Some(pid1) {
            return true;
        }
    }
    false
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
async fn close_idle_desktop_for_cli(session: &ActiveCodexSession) -> Result<bool> {
    let processes = crate::commands::process::check_codex_processes()
        .await
        .map_err(anyhow::Error::msg)?;
    let only_this_cli = processes.pids == vec![session.pid]
        || (processes.pids.len() == 1 && is_same_or_related_cli_pid(processes.pids[0], session.pid));
    if only_this_cli {
        return Ok(false);
    }
    desktop_handoff_is_exclusive(&session.session_id)?;
    let contains_session = processes.pids.iter().any(|&p| is_same_or_related_cli_pid(p, session.pid));
    if processes.pids.len() != 2 || !contains_session
        || !processes.pids.iter().copied().any(|pid| pid != session.pid && is_desktop_root_pid(pid)) {
        anyhow::bail!("Another Codex process is running; deferring CLI account handoff");
    }
    #[cfg(target_os = "macos")]
    {
        let status = Command::new("osascript")
            .args(["-e", "tell application id \"com.openai.codex\" to quit"])
            .status()?;
        if !status.success() {
            anyhow::bail!("Could not gracefully close Codex desktop");
        }
    }
    #[cfg(target_os = "linux")]
    {
        let desktop_pid = processes.pids.iter().copied().find(|&pid| pid != session.pid).unwrap();
        let _ = Command::new("kill").args(["-TERM", &desktop_pid.to_string()]).status();
    }
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let current = crate::commands::process::check_codex_processes()
            .await
            .map_err(anyhow::Error::msg)?;
        if current.pids == vec![session.pid] {
            return Ok(true);
        }
    }
    anyhow::bail!("Codex desktop did not close; account was not switched")
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
async fn app_server_response(
    lines: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    id: u64,
) -> Result<serde_json::Value> {
    loop {
        let line = tokio::time::timeout(Duration::from_secs(15), lines.next_line())
            .await
            .context("Codex app-server did not respond")?
            .context("Could not read Codex app-server response")?
            .context("Codex app-server exited before responding")?;
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if value.get("id").and_then(|v| v.as_u64()) != Some(id) {
            continue;
        }
        if let Some(error) = value.get("error") {
            anyhow::bail!("Codex app-server request failed: {error}");
        }
        return value.get("result").cloned().context("Codex app-server response has no result");
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
async fn app_server_request(
    stdin: &mut tokio::process::ChildStdin,
    lines: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    id: u64,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value> {
    let request = serde_json::json!({"id": id, "method": method, "params": params});
    stdin.write_all(format!("{request}\n").as_bytes()).await?;
    stdin.flush().await?;
    app_server_response(lines, id).await
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
async fn resume_desktop_after_handoff(
    token: String,
    sessions: &[ActiveCodexSession],
    settings: &AppSettings,
) -> Result<Vec<String>> {
    // The desktop app owns the writer locks while open. Start real turns on a
    // temporary app-server after closing it, then reopen the desktop UI.
    let mut app_server_cmd = tokio::process::Command::new(find_codex_binary());
    app_server_cmd.env_remove("LD_LIBRARY_PATH");
    let mut child = app_server_cmd
        .arg("app-server")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("Could not start Codex app-server for desktop continuation")?;
    let mut stdin = child.stdin.take().context("Codex app-server has no stdin")?;
    let stdout = child.stdout.take().context("Codex app-server has no stdout")?;
    let mut lines = BufReader::new(stdout).lines();
    let result: Result<Vec<String>> = async {
        app_server_request(
            &mut stdin,
            &mut lines,
            1,
            "initialize",
            serde_json::json!({"clientInfo":{"name":"codex-switcher","version":env!("CARGO_PKG_VERSION")}}),
        )
        .await?;
        stdin.write_all(b"{\"method\":\"initialized\",\"params\":{}}\n").await?;
        stdin.flush().await?;

        let mut started = Vec::new();
        for (index, session) in sessions.iter().enumerate() {
            let id = 2 + index as u64 * 2;
            if let Err(error) = app_server_request(
                &mut stdin,
                &mut lines,
                id,
                "thread/resume",
                serde_json::json!({"threadId": session.session_id}),
            )
            .await {
                eprintln!("[AutoRecovery] Could not resume desktop thread {}: {error}", session.session_id);
                continue;
            }
            let phrase = resolve_session_resume_phrase(&session.session_id, &settings.continue_phrase);
            if let Err(error) = app_server_request(
                &mut stdin,
                &mut lines,
                id + 1,
                "turn/start",
                serde_json::json!({
                    "threadId": session.session_id,
                    "input": [{"type": "text", "text": phrase}],
                }),
            )
            .await {
                eprintln!("[AutoRecovery] Could not start continuation in {}: {error}", session.session_id);
                continue;
            }
            started.push(session.session_id.clone());
        }
        if started.is_empty() {
            anyhow::bail!("Codex could not start a continuation in any desktop session");
        }
        Ok(started)
    }
    .await;

    // Keep the app-server and its writer locks alive until the turns finish.
    if let Ok(started) = &result {
        let remaining = started.len();
        tokio::spawn(async move {
            let _stdin = stdin;
            let mut completed = 0;
            let _ = tokio::time::timeout(Duration::from_secs(4 * 3600), async {
                while completed < remaining {
                    match lines.next_line().await {
                        Ok(Some(line)) => {
                            if serde_json::from_str::<serde_json::Value>(&line)
                                .ok()
                                .and_then(|value| value.get("method").and_then(|m| m.as_str()).map(str::to_owned))
                                .as_deref() == Some("turn/completed")
                            {
                                completed += 1;
                            }
                        }
                        _ => break,
                    }
                }
            }).await;
            let _ = child.kill().await;
        });
    } else {
        let _ = child.kill().await;
    }

    let reopened = crate::commands::reopen_closed_codex_desktop(token)
        .await
        .map_err(anyhow::Error::msg);
    reopened?;
    result
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn is_desktop_app_server_command(command: &str) -> bool {
    let command = command.trim().to_ascii_lowercase();
    (command.contains("/chatgpt.app/contents/resources/codex")
        || command.contains("/codex.app/contents/resources/codex")
        || command.contains("/usr/lib/chatgpt/resources/codex")
        || command.contains("/usr/lib/codex/resources/codex"))
        && command.contains(" app-server")
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn is_desktop_root_pid(pid: u32) -> bool {
    let Ok(output) = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "args="])
        .output() else { return false };
    if !output.status.success() {
        return false;
    }
    is_desktop_root_command(&String::from_utf8_lossy(&output.stdout))
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn is_desktop_root_command(command: &str) -> bool {
    let command = command.trim().to_ascii_lowercase();
    if command.contains("--type=") || command.contains("codex-switcher") {
        return false;
    }
    #[cfg(target_os = "macos")]
    {
        command.starts_with('/')
            && (command.contains("/chatgpt.app/contents/macos/chatgpt")
                || command.contains("/codex.app/contents/macos/codex"))
    }
    #[cfg(target_os = "linux")]
    {
        let first = command.split_whitespace().next().unwrap_or("");
        first.ends_with("/chatgpt")
            || first.ends_with("/codex-desktop")
            || first == "chatgpt"
            || first == "codex-desktop"
            || (command.starts_with('/') && (command.contains("/usr/lib/chatgpt/chatgpt") || command.contains("/usr/lib/codex/codex")))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    false
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn find_desktop_pid_for_session(lock_path: &Path) -> Option<u32> {
    let output = Command::new("lsof").arg("-t").arg(lock_path).output().ok()?;
    if !output.status.success() {
        return None;
    }
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Ok(pid) = line.trim().parse::<u32>() else { continue };
        let Ok(process) = Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "args="])
            .output() else { continue };
        if process.status.success()
            && is_desktop_app_server_command(&String::from_utf8_lossy(&process.stdout))
        {
            return Some(pid);
        }
    }
    None
}

/// Get process current working directory
fn get_process_cwd(pid: u32) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        if let Ok(dest) = fs::read_link(format!("/proc/{pid}/cwd")) {
            return Some(dest.to_string_lossy().to_string());
        }
    }

    #[cfg(unix)]
    {
        let output = Command::new("lsof")
            .args(["-p", &pid.to_string(), "-Fn"])
            .output()
            .ok()?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let mut next_is_cwd = false;
            for line in stdout.lines() {
                if line == "fcwd" {
                    next_is_cwd = true;
                    continue;
                }
                if next_is_cwd && line.starts_with('n') {
                    return Some(line[1..].to_string());
                }
                next_is_cwd = false;
            }
        }
    }

    None
}

/// Inspect the tail of a rollout jsonl file for recent errors
pub fn check_rollout_for_errors(rollout_path: &Path, session_id: &str) -> Option<DetectedSessionError> {
    let file = fs::File::open(rollout_path).ok()?;
    let metadata = file.metadata().ok()?;
    let file_size = metadata.len();
    if file_size == 0 {
        return None;
    }

    // Read up to last 64KB
    let read_size = std::cmp::min(file_size, 65536) as usize;
    let offset = file_size.saturating_sub(read_size as u64);

    use std::io::{Read, Seek, SeekFrom};
    let mut reader = std::io::BufReader::new(file);
    reader.seek(SeekFrom::Start(offset)).ok()?;
    let mut buffer = vec![0u8; read_size];
    reader.read_exact(&mut buffer).ok()?;

    let content = String::from_utf8_lossy(&buffer);

    for line in content.lines().rev() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if !line.contains("\"task_started\"")
            && !line.contains("\"task_complete\"")
            && !line.contains("\"turn_aborted\"")
        {
            continue;
        }

        let Ok(json_val): Result<serde_json::Value, _> = serde_json::from_str(line) else {
            continue;
        };

        let Some(payload) = json_val.get("payload") else {
            continue;
        };

        let event_type = payload.get("type").and_then(|t| t.as_str());

        // If the newest turn event is `task_started`, the session is actively executing a turn.
        // If it is `turn_aborted`, the turn was aborted/cancelled and is not in a rate-limit error state.
        if event_type == Some("task_started") || event_type == Some("turn_aborted") {
            return None;
        }

        if event_type == Some("task_complete") {
            // This is the completion event of the latest turn.
            // If it succeeded without error, the session completed normally and is healthy.
            let Some(error_obj) = payload.get("error").filter(|e| !e.is_null()) else {
                return None;
            };

            let codex_error_info = error_obj.get("codex_error_info").and_then(|v| v.as_str());
            let message = error_obj
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let turn_id = payload.get("turn_id").and_then(|v| v.as_str()).map(String::from);

            let kind = match codex_error_info {
                Some("usage_limit_exceeded") => Some(SessionErrorKind::UsageLimitExceeded),
                Some("server_overloaded") => Some(SessionErrorKind::ServerOverloaded),
                _ => {
                    if message.contains("usage limit") || message.contains("hit your usage limit") {
                        Some(SessionErrorKind::UsageLimitExceeded)
                    } else if message.contains("Selected model is at capacity") {
                        Some(SessionErrorKind::ServerOverloaded)
                    } else {
                        None
                    }
                }
            };

            if let Some(kind) = kind {
                return Some(DetectedSessionError {
                    kind,
                    session_id: session_id.to_string(),
                    turn_id,
                    message,
                    detected_at: Utc::now(),
                });
            } else {
                // Latest task completed with some other unrecoverable error
                return None;
            }
        }
    }

    None
}

/// Score an account for auto-switching candidate evaluation.
/// Higher score means more preferable.
pub fn calculate_account_score(
    account: &StoredAccount,
    strategy: AutoSwitchStrategy,
    usage: Option<&UsageInfo>,
    resets: Option<&AccountResetCredits>,
    warning_days: u32,
    now: DateTime<Utc>,
) -> f64 {
    let mut score = 0.0;

    let primary_used = usage.and_then(|u| u.primary_used_percent).unwrap_or(0.0);
    let secondary_used = usage.and_then(|u| u.secondary_used_percent).unwrap_or(0.0);

    let primary_left = (100.0 - primary_used).clamp(0.0, 100.0);
    let secondary_left = (100.0 - secondary_used).clamp(0.0, 100.0);

    // Immediate remaining quota is constrained by the bottleneck of present session and weekly windows
    let immediate_left = match (
        usage.and_then(|u| u.primary_used_percent),
        usage.and_then(|u| u.secondary_used_percent),
    ) {
        (Some(_), Some(_)) => primary_left.min(secondary_left),
        (Some(_), None) => primary_left,
        (None, Some(_)) => secondary_left,
        (None, None) => 100.0,
    };

    // Evaluate available banked reset credits
    let mut available_reset_count: u32 = 0;
    let mut closest_reset_days: Option<f64> = None;
    let mut has_urgent_reset = false;

    if let Some(credits) = resets {
        for credit in &credits.credits {
            if credit.status.to_lowercase() != "available" {
                continue;
            }
            let mut is_valid = true;
            let mut diff_days: Option<f64> = None;
            if let Some(exp_str) = credit.expires_at.as_deref() {
                if let Ok(exp) = DateTime::parse_from_rfc3339(exp_str) {
                    let exp_utc = exp.with_timezone(&Utc);
                    let diff_secs = (exp_utc - now).num_seconds();
                    if diff_secs <= 0 {
                        is_valid = false;
                    } else {
                        let days = diff_secs as f64 / 86400.0;
                        diff_days = Some(days);
                    }
                }
            }
            if is_valid {
                available_reset_count += 1;
                if let Some(days) = diff_days {
                    if days <= warning_days as f64 {
                        has_urgent_reset = true;
                    }
                    closest_reset_days = Some(
                        closest_reset_days.map_or(days, |curr| curr.min(days)),
                    );
                }
            }
        }
    }

    // Each available reset credit is effectively +100% full quota (deferred full-limit)
    let deferred_reset_quota = (available_reset_count as f64) * 100.0;
    let total_effective_quota = immediate_left + deferred_reset_quota;

    // Hard limit / exhausted check:
    // If an account is completely exhausted in any of its active windows
    // AND has no banked resets to restore it, penalize heavily (-50000)
    let is_exhausted = match (
        usage.and_then(|u| u.primary_used_percent),
        usage.and_then(|u| u.secondary_used_percent),
    ) {
        (Some(p), Some(s)) => p >= 100.0 || s >= 100.0,
        (Some(p), None) => p >= 100.0,
        (None, Some(s)) => s >= 100.0,
        (None, None) => false,
    };
    if is_exhausted && available_reset_count == 0 {
        score -= 50000.0;
    } else {
        score += total_effective_quota;
    }

    // Evaluate weekly reset timing & quota pacing
    if let Some(u) = usage {
        if let Some(resets_at) = u.secondary_resets_at {
            let diff_secs = resets_at - now.timestamp();
            let reset_hours = diff_secs as f64 / 3600.0;

            // Burn-before-reset: if weekly reset is in less than 24h and we still have >= 15% quota,
            // prioritize burning it before it expires and is lost!
            if reset_hours > 0.0 && reset_hours <= 24.0 && secondary_left >= 15.0 {
                score += 3000.0 + (secondary_left * 30.0);
            }

            // Starvation guard: if an account has < 15% weekly quota remaining, no banked resets,
            // and the weekly reset is still far away (> 48h), heavily penalize switching to it
            // so we don't prematurely exhaust it for the rest of the week.
            if available_reset_count == 0 && secondary_left < 15.0 && reset_hours > 48.0 {
                score -= 15000.0;
            }
        }
    }

    // Evaluate subscription expiration
    let mut sub_expired_or_urgent = false;
    let mut sub_diff_days: Option<f64> = None;
    if let Some(exp) = account.subscription_expires_at {
        let diff_secs = (exp - now).num_seconds();
        let days = diff_secs as f64 / 86400.0;
        sub_diff_days = Some(days);
        if days <= 2.0 {
            sub_expired_or_urgent = true;
        }
    }

    // Banked resets bonus:
    // Accounts with banked resets are highly valuable to use and burn first,
    // especially since reset credits can expire.
    let reset_bonus = if available_reset_count > 0 {
        let base_bonus = (available_reset_count as f64) * 20000.0;
        let urgency_bonus = if let Some(days) = closest_reset_days {
            // Earlier expiry gets higher priority (FIFO)
            (60.0 - days).max(0.0) * 100.0 + if has_urgent_reset { 10000.0 } else { 0.0 }
        } else {
            5000.0
        };
        base_bonus + urgency_bonus
    } else {
        0.0
    };

    // Plan tier reserve penalty:
    // More expensive accounts ($100 Pro Lite, $200 ChatGPT Pro) should be held in reserve
    // and spent after standard $20 Plus accounts, unless they have banked reset credits
    // or upcoming weekly resets that should be burned first.
    let tier_reserve_penalty = match account.plan_type.as_deref().map(|s| s.to_lowercase()).as_deref() {
        Some("prolite") => 3000.0,
        Some("pro") => 6000.0,
        Some("enterprise") => 4000.0,
        Some("team") => 2000.0,
        _ => 0.0, // "plus", "free", "api_key", or unknown
    };

    match strategy {
        AutoSwitchStrategy::SmartBalanced => {
            // 1. Prioritize accounts with banked resets (so they get used and refreshed)
            score += reset_bonus;

            // 2. Reserve penalty: keep expensive tiers (Pro Lite / Pro) behind Plus
            score -= tier_reserve_penalty;

            // 3. Prioritize subscriptions expiring soon or in grace period
            if sub_expired_or_urgent {
                if let Some(days) = sub_diff_days {
                    if days <= 0.0 {
                        // Past expiration: urgent burn before access revokes
                        score += 10000.0;
                    } else {
                        score += 8000.0 + (2.0 - days).max(0.0) * 500.0;
                    }
                }
            }
        }
        AutoSwitchStrategy::ResetsFirst => {
            if available_reset_count > 0 {
                let expiry_component =
                    closest_reset_days.map_or(0.0, |days| (60.0 - days).max(0.0) * 150.0);
                score += (available_reset_count as f64) * 35000.0 + expiry_component;
            }
            score -= tier_reserve_penalty;
        }
        AutoSwitchStrategy::ExpiringSubscriptionFirst => {
            if let Some(days) = sub_diff_days {
                if days <= 0.0 {
                    score += 25000.0;
                } else {
                    score += 20000.0 - (days * 50.0);
                }
            }
            score -= tier_reserve_penalty;
        }
        AutoSwitchStrategy::MostRemainingQuota => {
            // Dictated by total_effective_quota (immediate + 100% per banked reset)
            score = total_effective_quota * 10.0;
            if is_exhausted && available_reset_count == 0 {
                score -= 50000.0;
            }
            score -= tier_reserve_penalty;
        }
        AutoSwitchStrategy::RoundRobin => {
            // Neutral scoring, order handled by select_best_account
        }
    }

    score
}

/// Select best candidate account to switch to
pub fn select_best_account(
    accounts: &[StoredAccount],
    current_account_id: Option<&str>,
    strategy: AutoSwitchStrategy,
    usage_map: &HashMap<String, UsageInfo>,
    resets_map: &HashMap<String, AccountResetCredits>,
    warning_days: u32,
) -> Option<StoredAccount> {
    let candidates: Vec<&StoredAccount> = accounts
        .iter()
        .filter(|a| current_account_id.map_or(true, |curr| a.id != curr))
        .collect();

    if candidates.is_empty() {
        return None;
    }

    if strategy == AutoSwitchStrategy::RoundRobin {
        // Pick first candidate with available limit or banked resets
        for candidate in &candidates {
            let u = usage_map.get(&candidate.id);
            let resets = resets_map.get(&candidate.id);
            let has_resets = resets.map_or(0, |r| {
                r.credits
                    .iter()
                    .filter(|c| c.status.to_lowercase() == "available")
                    .count()
            }) > 0;
            let primary_used = u.and_then(|u| u.primary_used_percent).unwrap_or(0.0);
            let secondary_used = u.and_then(|u| u.secondary_used_percent).unwrap_or(0.0);

            let primary_ok = primary_used < 95.0 || has_resets;
            let secondary_ok = secondary_used < 95.0 || has_resets;

            if primary_ok && secondary_ok {
                return Some((*candidate).clone());
            }
        }
        return candidates.first().map(|a| (*a).clone());
    }

    let now = Utc::now();
    let mut scored: Vec<(&StoredAccount, f64)> = candidates
        .into_iter()
        .map(|acc| {
            let usage = usage_map.get(&acc.id);
            let resets = resets_map.get(&acc.id);
            let score = calculate_account_score(acc, strategy, usage, resets, warning_days, now);
            (acc, score)
        })
        .collect();

    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.first().map(|(acc, _)| (*acc).clone())
}

/// Check if a Codex session has an active, paused, or usage-limited goal in ~/.codex/goals_*.sqlite.
/// Unfinished goal statuses are: "active", "paused", "usage_limited", "blocked", "budget_limited".
pub fn is_session_goal_active(session_id: &str) -> bool {
    let Some(home) = dirs::home_dir() else {
        return false;
    };
    let codex_dir = home.join(".codex");
    let primary_db = codex_dir.join("goals_1.sqlite");

    if primary_db.exists() && is_session_goal_active_in_db(&primary_db, session_id) {
        return true;
    }

    // Check any other goals_*.sqlite in ~/.codex/ in case of future schema version bumps
    if let Ok(entries) = fs::read_dir(&codex_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
                if file_name.starts_with("goals_") && file_name.ends_with(".sqlite") && path != primary_db {
                    if is_session_goal_active_in_db(&path, session_id) {
                        return true;
                    }
                }
            }
        }
    }

    false
}

/// Helper to query SQLite db for active goal status
pub fn is_session_goal_active_in_db(db_path: &Path, session_id: &str) -> bool {
    let conn = match rusqlite::Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) {
        Ok(c) => c,
        Err(_) => return false,
    };

    let query = "SELECT status FROM thread_goals WHERE thread_id = ?1";
    let status: Result<String, _> = conn.query_row(query, rusqlite::params![session_id], |row| row.get(0));

    match status {
        Ok(s) => s != "complete",
        Err(_) => false,
    }
}

/// Reset goal status in goals SQLite database to 'active' so Codex CLI immediately
/// continues autonomous execution upon resume.
pub fn activate_session_goal(session_id: &str) -> bool {
    let Some(home) = dirs::home_dir() else {
        return false;
    };
    let codex_dir = home.join(".codex");
    let primary_db = codex_dir.join("goals_1.sqlite");

    let mut updated = false;
    if primary_db.exists() && activate_session_goal_in_db(&primary_db, session_id) {
        updated = true;
    }

    if let Ok(entries) = fs::read_dir(&codex_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
                if file_name.starts_with("goals_") && file_name.ends_with(".sqlite") && path != primary_db {
                    if activate_session_goal_in_db(&path, session_id) {
                        updated = true;
                    }
                }
            }
        }
    }

    updated
}

pub fn activate_session_goal_in_db(db_path: &Path, session_id: &str) -> bool {
    let conn = match rusqlite::Connection::open(db_path) {
        Ok(c) => c,
        Err(_) => return false,
    };

    let now_ms = chrono::Utc::now().timestamp_millis();
    let query = "UPDATE thread_goals SET status = 'active', updated_at_ms = ?1 WHERE thread_id = ?2 AND status IN ('usage_limited', 'paused')";
    match conn.execute(query, rusqlite::params![now_ms, session_id]) {
        Ok(rows) => rows > 0,
        Err(_) => false,
    }
}

/// Resolve the resume phrase for a session:
/// If the session has an active/unfinished goal, returns "/goal resume".
/// Otherwise, returns the user-configured continue phrase (defaulting to "continue" if blank).
pub fn resolve_session_resume_phrase(session_id: &str, user_continue_phrase: &str) -> String {
    if !session_id.is_empty() && is_session_goal_active(session_id) {
        "/goal resume".to_string()
    } else {
        let trimmed = user_continue_phrase.trim();
        if trimmed.is_empty() {
            "continue".to_string()
        } else {
            trimmed.to_string()
        }
    }
}

/// Send resume message via codex queue command
pub async fn send_codex_queue_resume(session_id: &str, phrase: &str) -> Result<()> {
    let codex_bin = find_codex_binary();
    let mut command = Command::new(codex_bin);
    command.env_remove("LD_LIBRARY_PATH");
    command.args(["queue", "--thread", session_id, "--message", phrase]);

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let output = tokio::task::spawn_blocking(move || command.output())
        .await
        .map_err(|e| anyhow::anyhow!("Tokio join error: {e}"))?
        .context("Failed to execute codex queue")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("codex queue failed: {stderr}");
    }

    Ok(())
}

/// Terminate a process cleanly with SIGTERM then SIGKILL
pub fn terminate_process(pid: u32) {
    if pid == 0 {
        return;
    }
    #[cfg(unix)]
    {
        #[cfg(target_os = "linux")]
        {
            if let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) {
                if let Some(ppid_str) = stat.split_whitespace().nth(3) {
                    if let Ok(ppid) = ppid_str.parse::<u32>() {
                        if ppid > 1 {
                            if let Ok(cmd) = fs::read_to_string(format!("/proc/{ppid}/cmdline")) {
                                if cmd.contains("node") && cmd.contains("codex") {
                                    let _ = Command::new("kill").args(["-TERM", &ppid.to_string()]).status();
                                }
                            }
                        }
                    }
                }
            }
        }
        let _ = Command::new("kill").args(["-TERM", &pid.to_string()]).status();
        std::thread::sleep(Duration::from_millis(300));
        let _ = Command::new("kill").args(["-KILL", &pid.to_string()]).status();
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill").args(["/PID", &pid.to_string(), "/F"]).status();
    }
}

/// Send desktop notification (cross-platform helper)
pub fn send_desktop_notification(title: &str, body: &str) {
    #[cfg(target_os = "linux")]
    {
        let mut cmd = Command::new("notify-send");
        cmd.env_remove("LD_LIBRARY_PATH");
        let _ = cmd
            .args(["-a", "Codex Switcher", "-i", "dialog-information", title, body])
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "display notification \"{}\" with title \"{}\"",
            body.replace('"', "\\\""),
            title.replace('"', "\\\"")
        );
        let _ = Command::new("osascript").args(["-e", &script]).spawn();
    }
}

/// Helper to focus/raise the terminal window on Linux
#[cfg(target_os = "linux")]
fn focus_terminal_window(pid: u32) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(500));

        // 1. Try wmctrl by matching PID
        let mut wmctrl_cmd = Command::new("wmctrl");
        wmctrl_cmd.env_remove("LD_LIBRARY_PATH");
        if let Ok(output) = wmctrl_cmd.args(["-l", "-p"]).output() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 3 && parts[2] == pid.to_string() {
                    let win_id = parts[0];
                    let mut act_cmd = Command::new("wmctrl");
                    act_cmd.env_remove("LD_LIBRARY_PATH");
                    let _ = act_cmd.args(["-i", "-a", win_id]).status();
                    return;
                }
            }
        }

        // 2. Fallback: try xdotool by PID
        let mut xdotool_cmd = Command::new("xdotool");
        xdotool_cmd.env_remove("LD_LIBRARY_PATH");
        if let Ok(output) = xdotool_cmd.args(["search", "--pid", &pid.to_string()]).output() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if let Some(win_id) = stdout.lines().last() {
                if !win_id.trim().is_empty() {
                    let mut act_cmd = Command::new("xdotool");
                    act_cmd.env_remove("LD_LIBRARY_PATH");
                    let _ = act_cmd.args(["windowactivate", win_id.trim()]).status();
                }
            }
        }
    });
}

/// Relaunch session inside user terminal emulator
pub fn launch_session_in_terminal(
    session_id: &str,
    cwd: Option<&str>,
    phrase: &str,
    preferred_terminal: Option<&str>,
) -> Result<u32> {
    let codex_bin = find_codex_binary();
    let restart_file = std::env::temp_dir()
        .join(format!("codex-switcher-restart-{}", session_id));
    #[cfg(target_os = "linux")]
    let restart_file_str = restart_file.to_string_lossy().to_string();

    let default_cwd = cwd
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."));

    // Check preferred or detected terminal
    #[cfg(target_os = "linux")]
    {
        let runner_script = r#"
unset LD_LIBRARY_PATH
unset LIBGL_ALWAYS_SOFTWARE
unset GALLIUM_DRIVER
unset WEBKIT_DISABLE_DMABUF_RENDERER

CODEX_BIN="$1"
SESSION_ID="$2"
CURRENT_PHRASE="$3"
RESTART_FILE="$4"
SESSION_CWD="$5"

while true; do
    if [ -n "$SESSION_CWD" ] && [ -d "$SESSION_CWD" ]; then
        if [ "$CURRENT_PHRASE" = "/goal resume" ] || [ -z "$CURRENT_PHRASE" ]; then
            "$CODEX_BIN" resume -C "$SESSION_CWD" "$SESSION_ID"
        else
            "$CODEX_BIN" resume -C "$SESSION_CWD" "$SESSION_ID" "$CURRENT_PHRASE"
        fi
    else
        if [ "$CURRENT_PHRASE" = "/goal resume" ] || [ -z "$CURRENT_PHRASE" ]; then
            "$CODEX_BIN" resume "$SESSION_ID"
        else
            "$CODEX_BIN" resume "$SESSION_ID" "$CURRENT_PHRASE"
        fi
    fi
    if [ -f "$RESTART_FILE" ]; then
        if [ -s "$RESTART_FILE" ]; then
            CURRENT_PHRASE=$(cat "$RESTART_FILE" 2>/dev/null)
        else
            CURRENT_PHRASE="continue"
        fi
        rm -f "$RESTART_FILE"
        printf "\n\033[1;36m========================================================\033[0m\n"
        printf "\033[1;32m [Codex Switcher] Account switched. Resuming in-place...\033[0m\n"
        printf "\033[1;36m========================================================\033[0m\n\n"
        sleep 1
        continue
    fi
    break
done
exec $SHELL
"#;

        let mut candidates = Vec::new();
        if let Some(preferred) = preferred_terminal.and_then(which_cmd) {
            candidates.push(preferred);
        }
        for name in &["ghostty", "alacritty", "kitty", "gnome-terminal", "x-terminal-emulator"] {
            if let Some(p) = which_cmd(name) {
                if !candidates.contains(&p) {
                    candidates.push(p);
                }
            }
        }

        let cwd_arg = cwd.unwrap_or("");
        let args_bundle = [
            "sh",
            "-c",
            runner_script,
            "sh",
            &codex_bin.to_string_lossy(),
            session_id,
            phrase,
            &restart_file_str,
            cwd_arg,
        ];

        for term_path in candidates {
            let term_name = term_path.file_name().unwrap_or_default().to_string_lossy();
            let mut cmd = Command::new(&term_path);

            // Strip AppImage runtime and software rendering workarounds so host binaries link against system libs
            cmd.env_remove("LD_LIBRARY_PATH");
            cmd.env_remove("LIBGL_ALWAYS_SOFTWARE");
            cmd.env_remove("GALLIUM_DRIVER");
            cmd.env_remove("WEBKIT_DISABLE_DMABUF_RENDERER");

            if term_name.contains("ghostty") {
                cmd.arg("--gtk-single-instance=false")
                    .arg(format!("--working-directory={}", default_cwd.display()))
                    .arg("-e")
                    .args(args_bundle);
            } else if term_name.contains("kitty") {
                cmd.arg("--directory")
                    .arg(&default_cwd)
                    .args(args_bundle);
            } else if term_name.contains("alacritty") {
                cmd.arg("--working-directory")
                    .arg(&default_cwd)
                    .arg("-e")
                    .args(args_bundle);
            } else if term_name.contains("gnome-terminal") {
                cmd.arg(format!("--working-directory={}", default_cwd.display()))
                    .arg("--")
                    .args(args_bundle);
            } else {
                cmd.current_dir(&default_cwd)
                    .arg("-e")
                    .args(args_bundle);
            }

            match cmd.spawn() {
                Ok(mut child) => {
                    let pid = child.id();
                    // Give process a small moment to ensure it doesn't immediately crash
                    std::thread::sleep(Duration::from_millis(150));
                    if let Ok(Some(status)) = child.try_wait() {
                        if !status.success() {
                            eprintln!("[AutoRecovery] Terminal {term_name} exited immediately with status {status}, trying next fallback...");
                            continue;
                        }
                    }
                    focus_terminal_window(pid);
                    if let Ok(mut state) = TRACKER.lock() {
                        state.managed_pids.push(pid);
                    }
                    return Ok(pid);
                }
                Err(e) => {
                    eprintln!("[AutoRecovery] Failed to spawn {term_name}: {e}, trying next fallback...");
                }
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        // Pass the command as an AppleScript argument instead of interpolating
        // it into source code. Shell-quote each value separately above.
        let script = "on run argv\n  tell application \"Terminal\" to do script (item 1 of argv)\nend run";
        let command = macos_resume_command(
            &codex_bin,
            &default_cwd,
            session_id,
            phrase,
            &restart_file,
        );
        let mut cmd = Command::new("osascript");
        cmd.arg("-e").arg(script).arg("--").arg(command);
        let child = cmd.spawn().context("Failed to spawn macOS Terminal")?;
        return Ok(child.id());
    }

    #[cfg(windows)]
    {
        let phrase_arg = if phrase == "/goal resume" || phrase.is_empty() {
            String::new()
        } else {
            format!(" {}", escape_shell_arg(phrase))
        };
        let codex_cmd = format!(
            "{} resume {}{}",
            escape_shell_arg(&codex_bin.to_string_lossy()),
            escape_shell_arg(session_id),
            phrase_arg
        );
        use std::os::windows::process::CommandExt;
        let mut cmd = Command::new("cmd.exe");
        cmd.current_dir(&default_cwd);
        cmd.args(["/c", "start", "cmd.exe", "/k", &codex_cmd]);
        let child = cmd.spawn().context("Failed to spawn Windows terminal")?;
        return Ok(child.id());
    }

    anyhow::bail!("No supported terminal emulator found")
}

/// Perform one automated recovery cycle
pub async fn check_and_recover_sessions() -> Result<Option<RecoveryEventNotification>> {
    let settings = load_app_settings().unwrap_or_default();
    let sessions = find_active_sessions()?;

    for session in sessions {
        // If the session has no error detected, skip it.
        // Do NOT wipe capacity_retries here because during active turns (task_started),
        // last_error is None, which would reset the retry counter prematurely!
        if session.last_error.is_none() {
            continue;
        }

        let Some(error) = &session.last_error else {
            continue;
        };

        match error.kind {
            SessionErrorKind::ServerOverloaded => {
                if !settings.auto_retry_capacity_enabled {
                    continue;
                }

                let mut should_retry = false;
                let mut should_escalate = false;
                let mut attempt_number = 1;

                {
                    let mut tracker = TRACKER
                        .lock()
                        .map_err(|_| anyhow::anyhow!("Tracker poisoned"))?;
                    
                    // entry: (last_retried_turn_id, consecutive_attempts, last_attempt_time)
                    let entry = tracker
                        .capacity_retries
                        .entry(session.session_id.clone())
                        .or_insert((None, 0, Instant::now() - Duration::from_secs(300)));

                    // If previous capacity attempt was long ago (>3 mins), start a fresh episode
                    if entry.2.elapsed() > Duration::from_secs(180) {
                        entry.1 = 0;
                    }

                    let current_turn_id = error.turn_id.clone().or_else(|| Some("unknown_turn".to_string()));

                    // If this exact turn error has ALREADY been sent to the queue, do NOT queue duplicate continue messages!
                    if entry.0 == current_turn_id {
                        continue;
                    }

                    // Calculate delay for this consecutive attempt
                    let delay_needed = Duration::from_secs(
                        (settings.auto_retry_capacity_initial_delay_sec as u64)
                            .max(1)
                            * (entry.1 as u64 + 1),
                    );

                    if entry.2.elapsed() >= delay_needed {
                        if entry.1 < settings.auto_retry_capacity_max_attempts {
                            entry.0 = current_turn_id;
                            entry.1 += 1;
                            entry.2 = Instant::now();
                            attempt_number = entry.1;
                            should_retry = true;
                        } else if settings.auto_retry_capacity_escalate_to_switch {
                            // Retries on this account exhausted; mark for escalation!
                            should_escalate = true;
                        } else {
                            // Consecutive retry limit reached without escalation
                            continue;
                        }
                    }
                }

                if should_escalate {
                    if let Ok(mut t) = TRACKER.lock() {
                        t.capacity_retries.remove(&session.session_id);
                    }
                    return handle_account_switch_for_session(&session, &settings).await;
                }

                if should_retry {
                    let phrase = resolve_session_resume_phrase(&session.session_id, &settings.continue_phrase);

                    send_codex_queue_resume(&session.session_id, &phrase).await?;

                    let notification = RecoveryEventNotification {
                        event_type: "capacity_retry".to_string(),
                        session_id: session.session_id.clone(),
                        message: format!(
                            "Server overloaded. Sent automatic retry (attempt {}/{}) with '{}'",
                            attempt_number, settings.auto_retry_capacity_max_attempts, phrase
                        ),
                        timestamp: Utc::now(),
                    };

                    if let Ok(mut tracker) = TRACKER.lock() {
                        tracker.last_event = Some(notification.clone());
                    }
                    return Ok(Some(notification));
                }
            }
            SessionErrorKind::UsageLimitExceeded => {
                if !settings.auto_switch_limit_enabled {
                    continue;
                }

                let already_handled = {
                    let tracker = TRACKER
                        .lock()
                        .map_err(|_| anyhow::anyhow!("Tracker poisoned"))?;
                    tracker
                        .handled_usage_limits
                        .get(&session.session_id)
                        .map(|t| t == error.turn_id.as_deref().unwrap_or("default"))
                        .unwrap_or(false)
                };

                if already_handled {
                    continue;
                }

                return handle_account_switch_for_session(&session, &settings).await;
            }
        }
    }

    Ok(None)
}

/// Switch account and relaunch a session that hit limits
async fn handle_account_switch_for_session(
    session: &ActiveCodexSession,
    settings: &AppSettings,
) -> Result<Option<RecoveryEventNotification>> {
    let store = load_accounts()?;
    let current_id = store.active_account_id.as_deref();

    // Anti-cascade guard: if an account switch happened recently (within 20s),
    // another active session's error is from the OLD account. Don't cascade-switch to yet another account!
    // Instead, simply give this session a `continue` so it runs on the newly active account.
    let recent_switch = {
        let Ok(tracker) = TRACKER.lock() else {
            return Ok(None);
        };
        tracker.last_account_switch.clone()
    };

    if let Some((switch_time, target_id, was_reset_credit, switched_session_id)) = recent_switch {
        if switch_time.elapsed() < Duration::from_secs(20) {
            // Anti-cascade: If this is the exact same session that just triggered the switch,
            // its rollout might still show the previous turn error while the new turn boots up.
            // Do NOT re-terminate or restart it!
            if switched_session_id == session.session_id {
                return Ok(None);
            }

            let phrase = resolve_session_resume_phrase(&session.session_id, &settings.continue_phrase);
            let is_goal = is_session_goal_active(&session.session_id);

            // If the account was NOT changed (for example, limits were restored via reset credit on the same account),
            // we do NOT need to terminate the process or launch a new terminal! The credentials in memory are still valid.
            // We just queue the continue phrase to the existing running session.
            if was_reset_credit || session.is_desktop {
                if is_goal {
                    activate_session_goal(&session.session_id);
                } else {
                    send_codex_queue_resume(&session.session_id, &phrase).await?;
                }

                if let Ok(mut tracker) = TRACKER.lock() {
                    let turn_key = session
                        .last_error
                        .as_ref()
                        .and_then(|e| e.turn_id.clone())
                        .unwrap_or_else(|| "default".to_string());
                    tracker
                        .handled_usage_limits
                        .insert(session.session_id.clone(), turn_key);
                }

                let notification = RecoveryEventNotification {
                    event_type: if was_reset_credit { "reset_credit_redeemed" } else { "account_switched" }.to_string(),
                    session_id: session.session_id.clone(),
                    message: format!(
                        "Resumed session with '{phrase}' on account '{}' after {}s",
                        target_id,
                        switch_time.elapsed().as_secs()
                    ),
                    timestamp: Utc::now(),
                };
                if let Ok(mut tracker) = TRACKER.lock() {
                    tracker.last_event = Some(notification.clone());
                }
                return Ok(Some(notification));
            }

            let restart_file = std::env::temp_dir()
                .join(format!("codex-switcher-restart-{}", session.session_id));
            let _ = fs::write(&restart_file, &phrase);

            // Terminate stale process that still holds old credentials in memory
            if session.pid > 0 {
                terminate_process(session.pid);
            }

            if is_goal {
                activate_session_goal(&session.session_id);
            }

            // Wait briefly to see if an existing terminal runner consumed the restart file
            let mut consumed = false;
            for _ in 0..12 {
                tokio::time::sleep(Duration::from_millis(100)).await;
                if !restart_file.exists() {
                    consumed = true;
                    break;
                }
            }

            if !consumed {
                let _ = fs::remove_file(&restart_file);
                let _ = launch_session_in_terminal(
                    &session.session_id,
                    session.cwd.as_deref(),
                    &phrase,
                    settings.preferred_terminal.as_deref(),
                );
            }

            if !is_goal {
                let session_id_clone = session.session_id.clone();
                let phrase_clone = phrase.clone();
                tokio::spawn(async move {
                    for attempt in 0..10 {
                        tokio::time::sleep(Duration::from_millis(800 + attempt * 400)).await;
                        if send_codex_queue_resume(&session_id_clone, &phrase_clone).await.is_ok() {
                            println!("[AutoRecovery] Successfully queued '{phrase_clone}' to resumed session {session_id_clone}");
                            break;
                        }
                    }
                });
            }

            send_desktop_notification(
                "Codex Session Reconnected",
                &format!("Reconnected with '{phrase}' on newly active account"),
            );

            if let Ok(mut tracker) = TRACKER.lock() {
                let turn_key = session
                    .last_error
                    .as_ref()
                    .and_then(|e| e.turn_id.clone())
                    .unwrap_or_else(|| "default".to_string());
                tracker
                    .handled_usage_limits
                    .insert(session.session_id.clone(), turn_key);
            }

            let notification = RecoveryEventNotification {
                event_type: "account_switched".to_string(),
                session_id: session.session_id.clone(),
                message: if consumed {
                    format!(
                        "Resumed session in-place with '{phrase}' on newly active account (switched {}s ago)",
                        switch_time.elapsed().as_secs()
                    )
                } else {
                    format!(
                        "Relaunched session with '{phrase}' on newly active account (switched {}s ago)",
                        switch_time.elapsed().as_secs()
                    )
                },
                timestamp: Utc::now(),
            };

            return Ok(Some(notification));
        }
    }

    // If auto_redeem_reset_credits is enabled, check if the currently active account has available reset credits
    if settings.auto_redeem_reset_credits {
        if let Some(curr_acc_id) = current_id {
            if let Some(curr_acc) = store.accounts.iter().find(|a| a.id == curr_acc_id) {
                if let Ok(Ok(stats)) = tokio::time::timeout(
                    Duration::from_secs(10),
                    crate::commands::account_stats::get_account_usage_stats(curr_acc.id.clone()),
                )
                .await
                {
                    if let Some(resets) = stats.reset_credits {
                        let mut available_credits: Vec<_> = resets
                            .credits
                            .into_iter()
                            .filter(|c| c.status.to_lowercase() == "available")
                            .collect();

                        // Sort by earliest expires_at (FIFO)
                        available_credits.sort_by(|a, b| match (&a.expires_at, &b.expires_at) {
                            (Some(ea), Some(eb)) => ea.cmp(eb),
                            (Some(_), None) => std::cmp::Ordering::Less,
                            (None, Some(_)) => std::cmp::Ordering::Greater,
                            (None, None) => std::cmp::Ordering::Equal,
                        });

                        if let Some(credit_to_redeem) = available_credits.first() {
                            println!(
                                "[AutoRecovery] Attempting auto-redeem of reset credit {} for account {}",
                                credit_to_redeem.id, curr_acc.name
                            );
                            if let Ok(()) = crate::commands::account_stats::redeem_reset_credit(
                                curr_acc,
                                &credit_to_redeem.id,
                            )
                            .await
                            {
                                println!(
                                    "[AutoRecovery] Successfully redeemed reset credit for account {}",
                                    curr_acc.name
                                );

                                let phrase = resolve_session_resume_phrase(&session.session_id, &settings.continue_phrase);
                                let is_goal = is_session_goal_active(&session.session_id);

                                if is_goal {
                                    activate_session_goal(&session.session_id);
                                } else {
                                    let _ = send_codex_queue_resume(&session.session_id, &phrase).await;
                                }

                                send_desktop_notification(
                                    "Reset Credit Redeemed",
                                    &format!(
                                        "Auto-redeemed 1 reset credit for '{}'. Limits restored to 100%.",
                                        curr_acc.name
                                    ),
                                );

                                if let Ok(mut tracker) = TRACKER.lock() {
                                    let turn_key = session
                                        .last_error
                                        .as_ref()
                                        .and_then(|e| e.turn_id.clone())
                                        .unwrap_or_else(|| "default".to_string());
                                    tracker
                                        .handled_usage_limits
                                        .insert(session.session_id.clone(), turn_key);
                                    // Update last_account_switch with curr_acc.id so other concurrent sessions
                                    // don't immediately trigger a cascade switch to another account!
                                    tracker.last_account_switch = Some((Instant::now(), curr_acc.id.clone(), true, session.session_id.clone()));
                                }

                                let notification = RecoveryEventNotification {
                                    event_type: "reset_credit_redeemed".to_string(),
                                    session_id: session.session_id.clone(),
                                    message: format!(
                                        "Auto-redeemed reset credit on active account '{}' (limits refreshed). Resumed session in-place.",
                                        curr_acc.name
                                    ),
                                    timestamp: Utc::now(),
                                };

                                if let Ok(mut tracker) = TRACKER.lock() {
                                    tracker.last_event = Some(notification.clone());
                                }

                                return Ok(Some(notification));
                            } else {
                                eprintln!(
                                    "[AutoRecovery] Failed to auto-redeem reset credit, falling back to account switch"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    // Fetch usage and stats cache
    let mut usage_map = HashMap::new();
    let mut resets_map = HashMap::new();

    // One stalled profile request must not block every recovery cycle. Fetch
    // candidates concurrently and bound each account's total lookup time.
    let candidates = store.accounts.iter().filter(|acc| Some(acc.id.as_str()) != current_id);
    let snapshots = futures::future::join_all(candidates.map(|acc| async {
        let account_id = acc.id.clone();
        let snapshot = tokio::time::timeout(Duration::from_secs(10), async {
            let usage = crate::commands::usage::fetch_usage(&account_id).await.ok();
            let resets = crate::commands::account_stats::get_account_usage_stats(account_id.clone())
                .await
                .ok()
                .and_then(|stats| stats.reset_credits);
            (usage, resets)
        })
        .await;
        (account_id, snapshot)
    }))
    .await;
    for (account_id, snapshot) in snapshots {
        match snapshot {
            Ok((usage, resets)) => {
                if let Some(usage) = usage {
                    usage_map.insert(account_id.clone(), usage);
                }
                if let Some(resets) = resets {
                    resets_map.insert(account_id, resets);
                }
            }
            Err(_) => eprintln!("[AutoRecovery] Candidate usage lookup timed out"),
        }
    }

    let target_account = select_best_account(
        &store.accounts,
        current_id,
        settings.auto_switch_strategy,
        &usage_map,
        &resets_map,
        settings.reset_credit_warning_days,
    );

    let Some(target) = target_account else {
        anyhow::bail!("No eligible fallback account found with available limits");
    };

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    let desktop_sessions = if session.is_desktop {
        let mut affected: Vec<_> = find_active_sessions()?.into_iter()
            .filter(|candidate| candidate.is_desktop && candidate.last_error.as_ref()
                .is_some_and(|error| error.kind == SessionErrorKind::UsageLimitExceeded))
            .collect();
        if !affected.iter().any(|candidate| candidate.session_id == session.session_id) {
            affected.push(session.clone());
        }
        affected
    } else {
        Vec::new()
    };

    let desktop_reopen_token: Option<String> = if session.is_desktop {
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        { Some(close_desktop_for_handoff(&session.session_id).await?) }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        { anyhow::bail!("Desktop recovery is not supported on this platform") }
    } else {
        None
    };
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    let reopen_desktop_after_cli = if session.is_desktop {
        false
    } else {
        close_idle_desktop_for_cli(session).await?
    };

    // Serialize this with manual switches and token refresh. Codex can rotate
    // its refresh token while running; preserve that live token before replacing
    // auth.json so switching back to this account still works.
    let switch_result: Result<StoredAccount> = async {
        let _auth_guard = AUTH_OPERATION_LOCK.lock().await;
        let mut updated_store = load_accounts()?;
        if updated_store.active_account_id.as_deref() != current_id {
            anyhow::bail!("Active account changed during auto recovery");
        }
        if let Some(auth) = read_current_auth()? {
            if sync_active_account_tokens(&mut updated_store, &auth) {
                save_accounts(&updated_store)?;
            }
        }
        let target = ensure_chatgpt_tokens_fresh_locked(&target).await?;
        switch_to_account(&target)?;
        // Refresh may have saved rotated target credentials to accounts.json.
        updated_store = load_accounts()?;
        updated_store.active_account_id = Some(target.id.clone());
        save_accounts(&updated_store)?;
        crate::commands::account::restart_codex_background_services();
        Ok(target)
    }.await;
    let target = match switch_result {
        Ok(target) => target,
        Err(error) => {
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            if let Some(token) = desktop_reopen_token {
                let _ = crate::commands::reopen_closed_codex_desktop(token).await;
            }
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            if reopen_desktop_after_cli {
                let _ = crate::commands::process::open_codex_app().await;
            }
            return Err(error);
        }
    };

    let phrase = resolve_session_resume_phrase(&session.session_id, &settings.continue_phrase);

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    if let Some(token) = desktop_reopen_token {
        if let Ok(mut tracker) = TRACKER.lock() {
            tracker.last_account_switch = Some((Instant::now(), target.id.clone(), false, session.session_id.clone()));
        }
        let started = resume_desktop_after_handoff(token, &desktop_sessions, settings).await?;
        if let Ok(mut tracker) = TRACKER.lock() {
            for recovered in &desktop_sessions {
                if started.contains(&recovered.session_id) {
                    let turn_key = recovered.last_error.as_ref().and_then(|e| e.turn_id.clone())
                        .unwrap_or_else(|| "default".to_string());
                    tracker.handled_usage_limits.insert(recovered.session_id.clone(), turn_key);
                }
            }
        }
        let notification = RecoveryEventNotification {
            event_type: "account_switched".to_string(),
            session_id: session.session_id.clone(),
            message: format!("Switched to '{}' and started continuations in {} desktop sessions", target.name, started.len()),
            timestamp: Utc::now(),
        };
        if let Ok(mut tracker) = TRACKER.lock() {
            tracker.last_event = Some(notification.clone());
        }
        return Ok(Some(notification));
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    if reopen_desktop_after_cli {
        if let Err(error) = crate::commands::process::open_codex_app().await {
            eprintln!("[AutoRecovery] CLI account switched, but desktop could not reopen: {error}");
        }
    }

    let is_goal = is_session_goal_active(&session.session_id);

    let restart_file = std::env::temp_dir()
        .join(format!("codex-switcher-restart-{}", session.session_id));
    let _ = fs::write(&restart_file, &phrase);

    // Codex CLI caches JWT tokens in process memory (CachedAuth) for the entire lifetime
    // of the process. In-place queue messages on an existing process will reuse the stale token
    // and fail again. We must terminate the old process and relaunch with `codex resume`
    // so the new process initializes fresh AuthManager from the newly written auth.json.
    if session.pid > 0 {
        terminate_process(session.pid);
    }

    if is_goal {
        activate_session_goal(&session.session_id);
    }

    // Check if an existing terminal runner consumed the restart file
    let mut consumed = false;
    for _ in 0..12 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if !restart_file.exists() {
            consumed = true;
            break;
        }
    }

    if !consumed {
        let _ = fs::remove_file(&restart_file);
        let _ = launch_session_in_terminal(
            &session.session_id,
            session.cwd.as_deref(),
            &phrase,
            settings.preferred_terminal.as_deref(),
        );
    }

    if !is_goal {
        let session_id_clone = session.session_id.clone();
        let phrase_clone = phrase.clone();
        tokio::spawn(async move {
            for attempt in 0..10 {
                tokio::time::sleep(Duration::from_millis(800 + attempt * 400)).await;
                if send_codex_queue_resume(&session_id_clone, &phrase_clone).await.is_ok() {
                    println!("[AutoRecovery] Successfully queued '{phrase_clone}' to resumed session {session_id_clone}");
                    break;
                }
            }
        });
    }

    send_desktop_notification(
        "Codex Account Switched",
        &format!("Switched to '{}'. Resuming session...", target.name),
    );

    // Mark handled and record switch timestamp for anti-cascade
    if let Ok(mut tracker) = TRACKER.lock() {
        let turn_key = session
            .last_error
            .as_ref()
            .and_then(|e| e.turn_id.clone())
            .unwrap_or_else(|| "default".to_string());
        tracker
            .handled_usage_limits
            .insert(session.session_id.clone(), turn_key);
        tracker.last_account_switch = Some((Instant::now(), target.id.clone(), false, session.session_id.clone()));
    }

    let notification = RecoveryEventNotification {
        event_type: "account_switched".to_string(),
        session_id: session.session_id.clone(),
        message: if consumed {
            format!(
                "Switched to account '{}' and resumed session in-place with '{}'",
                target.name, phrase
            )
        } else {
            format!(
                "Switched to account '{}' and relaunched session with '{}'",
                target.name, phrase
            )
        },
        timestamp: Utc::now(),
    };

    if let Ok(mut tracker) = TRACKER.lock() {
        tracker.last_event = Some(notification.clone());
    }

    Ok(Some(notification))
}

// ============================================================================
// Tauri Commands
// ============================================================================

/// Get active Codex sessions and auto-recovery status
#[tauri::command]
pub async fn get_auto_recovery_status() -> Result<AutoRecoveryStatus, String> {
    let sessions = find_active_sessions().map_err(|e| e.to_string())?;
    let last_event = TRACKER
        .lock()
        .map_err(|_| "Tracker poisoned".to_string())?
        .last_event
        .clone();

    Ok(AutoRecoveryStatus {
        active_sessions_count: sessions.len(),
        monitored_sessions: sessions,
        last_recovery_event: last_event,
    })
}

/// Trigger an immediate manual recovery check
#[tauri::command]
pub async fn trigger_auto_recovery_check() -> Result<Option<RecoveryEventNotification>, String> {
    check_and_recover_sessions()
        .await
        .map_err(|e| e.to_string())
}

/// Launch a new or resumed Codex session in a terminal
#[tauri::command]
pub async fn launch_codex_session(
    session_id: Option<String>,
    cwd: Option<String>,
    prompt: Option<String>,
) -> Result<u32, String> {
    let settings = load_app_settings().unwrap_or_default();
    let s_id = session_id.unwrap_or_else(|| "".to_string());
    let phrase = prompt.unwrap_or_else(|| {
        if !s_id.is_empty() {
            resolve_session_resume_phrase(&s_id, &settings.continue_phrase)
        } else if settings.continue_phrase.trim().is_empty() {
            "continue".to_string()
        } else {
            settings.continue_phrase.trim().to_string()
        }
    });

    launch_session_in_terminal(
        &s_id,
        cwd.as_deref(),
        &phrase,
        settings.preferred_terminal.as_deref(),
    )
    .map_err(|e| e.to_string())
}

/// Get full application settings
#[tauri::command]
pub fn get_app_settings() -> Result<AppSettings, String> {
    load_app_settings().map_err(|e| e.to_string())
}

/// Toggle limit recovery without overwriting the other auto-recovery settings.
#[tauri::command]
pub fn set_auto_switch_limit_enabled(enabled: bool) -> Result<AppSettings, String> {
    let mut settings = load_app_settings().map_err(|e| e.to_string())?;
    settings.auto_switch_limit_enabled = enabled;
    save_app_settings(&settings).map_err(|e| e.to_string())?;
    Ok(settings)
}

/// Save auto-recovery configuration
#[tauri::command]
pub async fn save_auto_recovery_settings(
    auto_retry_capacity_enabled: bool,
    auto_retry_capacity_max_attempts: u32,
    auto_retry_capacity_initial_delay_sec: u32,
    auto_retry_capacity_escalate_to_switch: bool,
    auto_switch_limit_enabled: bool,
    auto_switch_strategy: AutoSwitchStrategy,
    auto_redeem_reset_credits: bool,
    continue_phrase: String,
    reset_credit_warning_days: u32,
    preferred_terminal: Option<String>,
) -> Result<AppSettings, String> {
    let mut settings = load_app_settings().unwrap_or_default();
    settings.auto_retry_capacity_enabled = auto_retry_capacity_enabled;
    settings.auto_retry_capacity_max_attempts = auto_retry_capacity_max_attempts;
    settings.auto_retry_capacity_initial_delay_sec = auto_retry_capacity_initial_delay_sec;
    settings.auto_retry_capacity_escalate_to_switch = auto_retry_capacity_escalate_to_switch;
    settings.auto_switch_limit_enabled = auto_switch_limit_enabled;
    settings.auto_switch_strategy = auto_switch_strategy;
    settings.auto_redeem_reset_credits = auto_redeem_reset_credits;
    settings.continue_phrase = if continue_phrase.trim().is_empty() {
        "continue".to_string()
    } else {
        continue_phrase.trim().to_string()
    };
    settings.reset_credit_warning_days = reset_credit_warning_days.max(1);
    settings.preferred_terminal = preferred_terminal;

    save_app_settings(&settings).map_err(|e| e.to_string())?;
    Ok(settings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration as ChronoDuration, TimeZone};

    #[test]
    fn desktop_app_server_is_not_a_recoverable_cli_session() {
        assert!(!is_supported_cli_command(
            "/Applications/ChatGPT.app/Contents/Resources/codex app-server --analytics-default-enabled"
        ));
        assert!(!is_supported_cli_command(
            "/usr/lib/chatgpt/resources/codex -c features.code_mode_host=true app-server --analytics-default-enabled"
        ));
        assert!(!is_supported_cli_command("/usr/local/bin/codex app-server"));
        assert!(is_supported_cli_command("/usr/local/bin/codex resume session-id"));
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn recognizes_desktop_app_server() {
        assert!(is_desktop_app_server_command(
            "/Applications/ChatGPT.app/Contents/Resources/codex app-server --analytics-default-enabled"
        ));
        assert!(is_desktop_app_server_command(
            "/usr/lib/chatgpt/resources/codex -c features.code_mode_host=true app-server --analytics-default-enabled"
        ));
        assert!(!is_desktop_app_server_command("/usr/local/bin/codex app-server"));
        assert!(!is_desktop_app_server_command(
            "/Applications/ChatGPT.app/Contents/MacOS/ChatGPT"
        ));
        #[cfg(target_os = "macos")]
        {
            assert!(is_desktop_root_command(
                "/Users/test/Applications With Spaces/ChatGPT.app/Contents/MacOS/ChatGPT"
            ));
            assert!(!is_desktop_root_command("/usr/local/bin/codex resume thread"));
        }
        #[cfg(target_os = "linux")]
        {
            assert!(is_desktop_root_command("/usr/lib/chatgpt/ChatGPT"));
            assert!(is_desktop_root_command("/usr/bin/codex-desktop"));
            assert!(is_desktop_root_command("/usr/bin/chatgpt"));
            assert!(!is_desktop_root_command("/usr/lib/chatgpt/ChatGPT --type=renderer"));
            assert!(!is_desktop_root_command("/usr/local/bin/codex resume thread"));
        }
    }

    #[test]
    fn detects_another_turn_before_desktop_handoff() {
        let path = std::env::temp_dir().join(format!("codex_handoff_{}.jsonl", uuid::Uuid::new_v4()));
        fs::write(&path, "{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\"}}\n").unwrap();
        assert!(rollout_has_active_turn(&path).unwrap());
        fs::write(&path, "{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\"}}\n{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\"}}\n").unwrap();
        assert!(!rollout_has_active_turn(&path).unwrap());
        fs::write(&path, "{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\"}}\n{\"type\":\"event_msg\",\"payload\":{\"type\":\"turn_aborted\"}}\n").unwrap();
        assert!(!rollout_has_active_turn(&path).unwrap());
        let _ = fs::remove_file(path);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_resume_command_quotes_every_dynamic_argument() {
        let command = macos_resume_command(
            Path::new("/tmp/Codex Bin/codex"),
            Path::new("/tmp/my work; touch /tmp/injected"),
            "thread' id",
            "continue $(touch /tmp/injected) 'now'",
            Path::new("/tmp/restart file"),
        );
        assert!(command.contains("cd '/tmp/my work; touch /tmp/injected'"));
        assert!(command.contains("'/tmp/Codex Bin/codex' resume 'thread'\\'' id'"));
        assert!(command.contains("'continue $(touch /tmp/injected) '\\''now'\\'''"));
        assert!(command.contains("[ -f '/tmp/restart file' ]"));
    }

    fn make_test_account(id: &str, name: &str, expires_at: Option<DateTime<Utc>>) -> StoredAccount {
        let mut acc = StoredAccount::new_chatgpt(
            name.into(),
            Some(format!("{name}@example.com")),
            Some("plus".into()),
            expires_at,
            "header.payload.sig".into(),
            "access".into(),
            "refresh".into(),
            Some(id.into()),
        );
        acc.id = id.into();
        acc
    }

    #[test]
    fn test_smart_balanced_prioritizes_urgent_resets() {
        let now = Utc.with_ymd_and_hms(2026, 9, 19, 12, 0, 0).unwrap();
        let acc1 = make_test_account("acc1", "Acc 1", None);
        let acc2 = make_test_account("acc2", "Acc 2", None);

        let resets2 = AccountResetCredits {
            available_count: 1,
            next_expires_at: Some((now + ChronoDuration::days(1)).to_rfc3339()),
            credits: vec![crate::commands::account_stats::AccountResetCredit {
                id: "rc1".into(),
                reset_type: "standard".into(),
                status: "available".into(),
                granted_at: None,
                expires_at: Some((now + ChronoDuration::days(1)).to_rfc3339()),
                redeem_started_at: None,
                redeemed_at: None,
                title: None,
                description: None,
            }],
        };

        let score1 = calculate_account_score(
            &acc1,
            AutoSwitchStrategy::SmartBalanced,
            None,
            None,
            3,
            now,
        );

        let score2 = calculate_account_score(
            &acc2,
            AutoSwitchStrategy::SmartBalanced,
            None,
            Some(&resets2),
            3,
            now,
        );

        assert!(score2 > score1, "Account with urgent reset should score significantly higher");
    }

    #[test]
    fn test_smart_balanced_prioritizes_expired_subscriptions_over_distant() {
        let now = Utc.with_ymd_and_hms(2026, 9, 19, 12, 0, 0).unwrap();
        // Expired yesterday (grace period)
        let acc_expired = make_test_account("acc1", "Expired", Some(now - ChronoDuration::days(1)));
        // Expiring in 30 days
        let acc_future = make_test_account("acc2", "Future", Some(now + ChronoDuration::days(30)));

        let score_exp = calculate_account_score(
            &acc_expired,
            AutoSwitchStrategy::SmartBalanced,
            None,
            None,
            3,
            now,
        );

        let score_fut = calculate_account_score(
            &acc_future,
            AutoSwitchStrategy::SmartBalanced,
            None,
            None,
            3,
            now,
        );

        assert!(score_exp > score_fut, "Expiring/expired subscription should be utilized first");
    }

    #[test]
    fn test_smart_balanced_prioritizes_account_with_banked_reset_over_fresh_account() {
        let now = Utc.with_ymd_and_hms(2026, 9, 23, 14, 0, 0).unwrap();
        // Account with 7% left but with 1 banked reset
        let acc_with_reset = make_test_account("acc1", "WithReset", None);
        let usage_with_reset = UsageInfo {
            account_id: "acc1".into(),
            plan_type: Some("plus".into()),
            primary_used_percent: Some(0.0),
            primary_window_minutes: Some(300),
            primary_resets_at: Some(now.timestamp() + 18000),
            secondary_used_percent: Some(93.0), // 7% left
            secondary_window_minutes: Some(10080),
            secondary_resets_at: Some(now.timestamp() + 104 * 3600),
            has_credits: None,
            unlimited_credits: None,
            credits_balance: None,
            error: None,
        };
        let resets = AccountResetCredits {
            available_count: 1,
            next_expires_at: Some((now + ChronoDuration::days(29)).to_rfc3339()),
            credits: vec![crate::commands::account_stats::AccountResetCredit {
                id: "rc1".into(),
                reset_type: "standard".into(),
                status: "available".into(),
                granted_at: None,
                expires_at: Some((now + ChronoDuration::days(29)).to_rfc3339()),
                redeem_started_at: None,
                redeemed_at: None,
                title: None,
                description: None,
            }],
        };

        // Account with 84% left but NO banked resets
        let acc_fresh = make_test_account("acc2", "Fresh", None);
        let usage_fresh = UsageInfo {
            account_id: "acc2".into(),
            plan_type: Some("plus".into()),
            primary_used_percent: Some(0.0),
            primary_window_minutes: Some(300),
            primary_resets_at: Some(now.timestamp() + 18000),
            secondary_used_percent: Some(16.0), // 84% left
            secondary_window_minutes: Some(10080),
            secondary_resets_at: Some(now.timestamp() + 160 * 3600),
            has_credits: None,
            unlimited_credits: None,
            credits_balance: None,
            error: None,
        };

        let score_reset = calculate_account_score(
            &acc_with_reset,
            AutoSwitchStrategy::SmartBalanced,
            Some(&usage_with_reset),
            Some(&resets),
            3,
            now,
        );

        let score_fresh = calculate_account_score(
            &acc_fresh,
            AutoSwitchStrategy::SmartBalanced,
            Some(&usage_fresh),
            None,
            3,
            now,
        );

        assert!(
            score_reset > score_fresh,
            "Account with banked reset should be prioritized to burn and reset first (score_reset={score_reset}, score_fresh={score_fresh})"
        );
    }

    #[test]
    fn test_starvation_guard_penalizes_low_quota_without_resets() {
        let now = Utc.with_ymd_and_hms(2026, 9, 23, 14, 0, 0).unwrap();
        // Account with 7% left and NO banked resets (reset in 104h)
        let acc_starving = make_test_account("acc1", "Starving", None);
        let usage_starving = UsageInfo {
            account_id: "acc1".into(),
            plan_type: Some("plus".into()),
            primary_used_percent: Some(0.0),
            primary_window_minutes: Some(300),
            primary_resets_at: Some(now.timestamp() + 18000),
            secondary_used_percent: Some(93.0), // 7% left
            secondary_window_minutes: Some(10080),
            secondary_resets_at: Some(now.timestamp() + 104 * 3600),
            has_credits: None,
            unlimited_credits: None,
            credits_balance: None,
            error: None,
        };

        // Fresh account with 84% left (reset in 160h)
        let acc_fresh = make_test_account("acc2", "Fresh", None);
        let usage_fresh = UsageInfo {
            account_id: "acc2".into(),
            plan_type: Some("plus".into()),
            primary_used_percent: Some(0.0),
            primary_window_minutes: Some(300),
            primary_resets_at: Some(now.timestamp() + 18000),
            secondary_used_percent: Some(16.0), // 84% left
            secondary_window_minutes: Some(10080),
            secondary_resets_at: Some(now.timestamp() + 160 * 3600),
            has_credits: None,
            unlimited_credits: None,
            credits_balance: None,
            error: None,
        };

        let score_starving = calculate_account_score(
            &acc_starving,
            AutoSwitchStrategy::SmartBalanced,
            Some(&usage_starving),
            None,
            3,
            now,
        );

        let score_fresh = calculate_account_score(
            &acc_fresh,
            AutoSwitchStrategy::SmartBalanced,
            Some(&usage_fresh),
            None,
            3,
            now,
        );

        assert!(
            score_fresh > score_starving,
            "Starving account without resets should be heavily penalized against fresh account (score_fresh={score_fresh}, score_starving={score_starving})"
        );
    }

    #[test]
    fn test_burn_before_reset_prioritizes_expiring_weekly_quota() {
        let now = Utc.with_ymd_and_hms(2026, 9, 23, 14, 0, 0).unwrap();
        // Account expiring in 10 hours with 50% left
        let acc_soon = make_test_account("acc1", "SoonReset", None);
        let usage_soon = UsageInfo {
            account_id: "acc1".into(),
            plan_type: Some("plus".into()),
            primary_used_percent: Some(0.0),
            primary_window_minutes: Some(300),
            primary_resets_at: Some(now.timestamp() + 18000),
            secondary_used_percent: Some(50.0), // 50% left
            secondary_window_minutes: Some(10080),
            secondary_resets_at: Some(now.timestamp() + 10 * 3600),
            has_credits: None,
            unlimited_credits: None,
            credits_balance: None,
            error: None,
        };

        // Account expiring in 120 hours with same 50% left
        let acc_later = make_test_account("acc2", "LaterReset", None);
        let usage_later = UsageInfo {
            account_id: "acc2".into(),
            plan_type: Some("plus".into()),
            primary_used_percent: Some(0.0),
            primary_window_minutes: Some(300),
            primary_resets_at: Some(now.timestamp() + 18000),
            secondary_used_percent: Some(50.0), // 50% left
            secondary_window_minutes: Some(10080),
            secondary_resets_at: Some(now.timestamp() + 120 * 3600),
            has_credits: None,
            unlimited_credits: None,
            credits_balance: None,
            error: None,
        };

        let score_soon = calculate_account_score(
            &acc_soon,
            AutoSwitchStrategy::SmartBalanced,
            Some(&usage_soon),
            None,
            3,
            now,
        );

        let score_later = calculate_account_score(
            &acc_later,
            AutoSwitchStrategy::SmartBalanced,
            Some(&usage_later),
            None,
            3,
            now,
        );

        assert!(
            score_soon > score_later,
            "Account with weekly reset in <24h should have burn-before-reset priority (score_soon={score_soon}, score_later={score_later})"
        );
    }

    #[test]
    fn test_screenshot_scenario_selects_account_with_banked_reset() {
        let now = Utc.with_ymd_and_hms(2026, 9, 23, 14, 0, 0).unwrap();

        // Exact accounts from user's screenshot
        let active_acc = make_test_account("acc_active", "rjandjf", Some(now + ChronoDuration::days(26)));
        let top_left = make_test_account("acc_tl", "lqslhdcghu", Some(now + ChronoDuration::days(25)));
        let bottom_left = make_test_account("acc_bl", "rqmboyj", Some(now + ChronoDuration::days(25)));
        let bottom_right = make_test_account("acc_br", "sonyamcmillan", Some(now + ChronoDuration::days(27)));

        let accounts = vec![active_acc.clone(), top_left.clone(), bottom_left.clone(), bottom_right.clone()];

        let mut usage_map = HashMap::new();
        let mut resets_map = HashMap::new();

        // Top-left: 84% weekly left, 161h to reset, 0 resets
        usage_map.insert("acc_tl".into(), UsageInfo {
            account_id: "acc_tl".into(),
            plan_type: Some("plus".into()),
            primary_used_percent: Some(0.0),
            primary_window_minutes: Some(300),
            primary_resets_at: Some(now.timestamp() + 18000),
            secondary_used_percent: Some(16.0),
            secondary_window_minutes: Some(10080),
            secondary_resets_at: Some(now.timestamp() + 161 * 3600),
            has_credits: None,
            unlimited_credits: None,
            credits_balance: None,
            error: None,
        });

        // Bottom-left: 7% weekly left, 104h to reset, 1 banked reset (expires in 29d)
        usage_map.insert("acc_bl".into(), UsageInfo {
            account_id: "acc_bl".into(),
            plan_type: Some("plus".into()),
            primary_used_percent: Some(0.0),
            primary_window_minutes: Some(300),
            primary_resets_at: Some(now.timestamp() + 18000),
            secondary_used_percent: Some(93.0),
            secondary_window_minutes: Some(10080),
            secondary_resets_at: Some(now.timestamp() + 104 * 3600),
            has_credits: None,
            unlimited_credits: None,
            credits_balance: None,
            error: None,
        });
        resets_map.insert("acc_bl".into(), AccountResetCredits {
            available_count: 1,
            next_expires_at: Some((now + ChronoDuration::days(29)).to_rfc3339()),
            credits: vec![crate::commands::account_stats::AccountResetCredit {
                id: "rc_bl".into(),
                reset_type: "standard".into(),
                status: "available".into(),
                granted_at: None,
                expires_at: Some((now + ChronoDuration::days(29)).to_rfc3339()),
                redeem_started_at: None,
                redeemed_at: None,
                title: None,
                description: None,
            }],
        });

        // Bottom-right: 84% weekly left, 159h to reset, 0 resets
        usage_map.insert("acc_br".into(), UsageInfo {
            account_id: "acc_br".into(),
            plan_type: Some("plus".into()),
            primary_used_percent: Some(0.0),
            primary_window_minutes: Some(300),
            primary_resets_at: Some(now.timestamp() + 18000),
            secondary_used_percent: Some(16.0),
            secondary_window_minutes: Some(10080),
            secondary_resets_at: Some(now.timestamp() + 159 * 3600),
            has_credits: None,
            unlimited_credits: None,
            credits_balance: None,
            error: None,
        });

        let selected = select_best_account(
            &accounts,
            Some("acc_active"),
            AutoSwitchStrategy::SmartBalanced,
            &usage_map,
            &resets_map,
            3,
        );

        assert!(selected.is_some());
        let target = selected.unwrap();
        assert_eq!(
            target.id, "acc_bl",
            "Must choose Bottom-Left (acc_bl) because it has a banked reset to burn and redeem first!"
        );
    }

    #[test]
    fn test_screenshot_scenario_avoids_starving_account_when_no_resets() {
        let now = Utc.with_ymd_and_hms(2026, 9, 23, 14, 0, 0).unwrap();

        let active_acc = make_test_account("acc_active", "rjandjf", None);
        let top_left = make_test_account("acc_tl", "lqslhdcghu", None);
        let bottom_left = make_test_account("acc_bl", "rqmboyj", None);
        let bottom_right = make_test_account("acc_br", "sonyamcmillan", None);

        let accounts = vec![active_acc.clone(), top_left.clone(), bottom_left.clone(), bottom_right.clone()];

        let mut usage_map = HashMap::new();
        let resets_map = HashMap::new(); // NO resets for any account!

        // Top-left: 84% weekly left, 161h to reset
        usage_map.insert("acc_tl".into(), UsageInfo {
            account_id: "acc_tl".into(),
            plan_type: Some("plus".into()),
            primary_used_percent: Some(0.0),
            primary_window_minutes: Some(300),
            primary_resets_at: Some(now.timestamp() + 18000),
            secondary_used_percent: Some(16.0),
            secondary_window_minutes: Some(10080),
            secondary_resets_at: Some(now.timestamp() + 161 * 3600),
            has_credits: None,
            unlimited_credits: None,
            credits_balance: None,
            error: None,
        });

        // Bottom-left: 7% weekly left, 104h to reset, NO resets!
        usage_map.insert("acc_bl".into(), UsageInfo {
            account_id: "acc_bl".into(),
            plan_type: Some("plus".into()),
            primary_used_percent: Some(0.0),
            primary_window_minutes: Some(300),
            primary_resets_at: Some(now.timestamp() + 18000),
            secondary_used_percent: Some(93.0),
            secondary_window_minutes: Some(10080),
            secondary_resets_at: Some(now.timestamp() + 104 * 3600),
            has_credits: None,
            unlimited_credits: None,
            credits_balance: None,
            error: None,
        });

        // Bottom-right: 84% weekly left, 159h to reset
        usage_map.insert("acc_br".into(), UsageInfo {
            account_id: "acc_br".into(),
            plan_type: Some("plus".into()),
            primary_used_percent: Some(0.0),
            primary_window_minutes: Some(300),
            primary_resets_at: Some(now.timestamp() + 18000),
            secondary_used_percent: Some(16.0),
            secondary_window_minutes: Some(10080),
            secondary_resets_at: Some(now.timestamp() + 159 * 3600),
            has_credits: None,
            unlimited_credits: None,
            credits_balance: None,
            error: None,
        });

        let selected = select_best_account(
            &accounts,
            Some("acc_active"),
            AutoSwitchStrategy::SmartBalanced,
            &usage_map,
            &resets_map,
            3,
        );

        assert!(selected.is_some());
        let target = selected.unwrap();
        assert_ne!(
            target.id, "acc_bl",
            "Must NOT choose starving Bottom-Left (acc_bl) when it has 0 resets and reset is >48h away!"
        );
        assert!(
            target.id == "acc_tl" || target.id == "acc_br",
            "Must choose one of the fresh accounts (acc_tl or acc_br)"
        );
    }

    #[test]
    fn test_fifo_expiry_prioritizes_soonest_expiring_credit() {
        let now = Utc.with_ymd_and_hms(2026, 9, 23, 14, 0, 0).unwrap();

        let acc_urgent = make_test_account("acc1", "UrgentCredit", None);
        let acc_distant = make_test_account("acc2", "DistantCredit", None);

        let resets_urgent = AccountResetCredits {
            available_count: 1,
            next_expires_at: Some((now + ChronoDuration::days(2)).to_rfc3339()),
            credits: vec![crate::commands::account_stats::AccountResetCredit {
                id: "rc_urgent".into(),
                reset_type: "standard".into(),
                status: "available".into(),
                granted_at: None,
                expires_at: Some((now + ChronoDuration::days(2)).to_rfc3339()),
                redeem_started_at: None,
                redeemed_at: None,
                title: None,
                description: None,
            }],
        };

        let resets_distant = AccountResetCredits {
            available_count: 1,
            next_expires_at: Some((now + ChronoDuration::days(25)).to_rfc3339()),
            credits: vec![crate::commands::account_stats::AccountResetCredit {
                id: "rc_distant".into(),
                reset_type: "standard".into(),
                status: "available".into(),
                granted_at: None,
                expires_at: Some((now + ChronoDuration::days(25)).to_rfc3339()),
                redeem_started_at: None,
                redeemed_at: None,
                title: None,
                description: None,
            }],
        };

        let score_urgent = calculate_account_score(
            &acc_urgent,
            AutoSwitchStrategy::SmartBalanced,
            None,
            Some(&resets_urgent),
            3,
            now,
        );

        let score_distant = calculate_account_score(
            &acc_distant,
            AutoSwitchStrategy::SmartBalanced,
            None,
            Some(&resets_distant),
            3,
            now,
        );

        assert!(
            score_urgent > score_distant,
            "Reset credit expiring in 2 days must score higher than credit expiring in 25 days (FIFO)"
        );
    }

    #[test]
    fn test_exhausted_weekly_without_resets_is_heavily_penalized() {
        let now = Utc.with_ymd_and_hms(2026, 9, 23, 14, 0, 0).unwrap();

        // 0% session used, but 100% weekly used (completely locked out)
        let acc_exhausted = make_test_account("acc1", "WeeklyExhausted", None);
        let usage_exhausted = UsageInfo {
            account_id: "acc1".into(),
            plan_type: Some("plus".into()),
            primary_used_percent: Some(0.0),
            primary_window_minutes: Some(300),
            primary_resets_at: Some(now.timestamp() + 18000),
            secondary_used_percent: Some(100.0),
            secondary_window_minutes: Some(10080),
            secondary_resets_at: Some(now.timestamp() + 72 * 3600),
            has_credits: None,
            unlimited_credits: None,
            credits_balance: None,
            error: None,
        };

        let score = calculate_account_score(
            &acc_exhausted,
            AutoSwitchStrategy::SmartBalanced,
            Some(&usage_exhausted),
            None,
            3,
            now,
        );

        assert!(
            score < -40000.0,
            "Account with 100% weekly usage and no resets must receive severe penalty (score={score})"
        );
    }

    #[test]
    fn test_exhausted_weekly_with_banked_reset_is_not_penalized() {
        let now = Utc.with_ymd_and_hms(2026, 9, 23, 14, 0, 0).unwrap();

        let acc_with_reset = make_test_account("acc1", "CanReset", None);
        let usage_exhausted = UsageInfo {
            account_id: "acc1".into(),
            plan_type: Some("plus".into()),
            primary_used_percent: Some(0.0),
            primary_window_minutes: Some(300),
            primary_resets_at: Some(now.timestamp() + 18000),
            secondary_used_percent: Some(100.0),
            secondary_window_minutes: Some(10080),
            secondary_resets_at: Some(now.timestamp() + 72 * 3600),
            has_credits: None,
            unlimited_credits: None,
            credits_balance: None,
            error: None,
        };
        let resets = AccountResetCredits {
            available_count: 1,
            next_expires_at: Some((now + ChronoDuration::days(10)).to_rfc3339()),
            credits: vec![crate::commands::account_stats::AccountResetCredit {
                id: "rc1".into(),
                reset_type: "standard".into(),
                status: "available".into(),
                granted_at: None,
                expires_at: Some((now + ChronoDuration::days(10)).to_rfc3339()),
                redeem_started_at: None,
                redeemed_at: None,
                title: None,
                description: None,
            }],
        };

        let score = calculate_account_score(
            &acc_with_reset,
            AutoSwitchStrategy::SmartBalanced,
            Some(&usage_exhausted),
            Some(&resets),
            3,
            now,
        );

        assert!(
            score > 0.0,
            "Account with 100% weekly usage but having banked reset must NOT be penalized (score={score})"
        );
    }

    #[test]
    fn test_most_remaining_quota_includes_banked_resets() {
        let now = Utc.with_ymd_and_hms(2026, 9, 23, 14, 0, 0).unwrap();

        // Account A: 10% left, but 2 banked resets -> 210% effective
        let acc_a = make_test_account("acc1", "LowWithTwoResets", None);
        let usage_a = UsageInfo {
            account_id: "acc1".into(),
            plan_type: Some("plus".into()),
            primary_used_percent: Some(0.0),
            primary_window_minutes: Some(300),
            primary_resets_at: Some(now.timestamp() + 18000),
            secondary_used_percent: Some(90.0),
            secondary_window_minutes: Some(10080),
            secondary_resets_at: Some(now.timestamp() + 72 * 3600),
            has_credits: None,
            unlimited_credits: None,
            credits_balance: None,
            error: None,
        };
        let resets_a = AccountResetCredits {
            available_count: 2,
            next_expires_at: Some((now + ChronoDuration::days(10)).to_rfc3339()),
            credits: vec![
                crate::commands::account_stats::AccountResetCredit {
                    id: "rc1".into(),
                    reset_type: "standard".into(),
                    status: "available".into(),
                    granted_at: None,
                    expires_at: Some((now + ChronoDuration::days(10)).to_rfc3339()),
                    redeem_started_at: None,
                    redeemed_at: None,
                    title: None,
                    description: None,
                },
                crate::commands::account_stats::AccountResetCredit {
                    id: "rc2".into(),
                    reset_type: "standard".into(),
                    status: "available".into(),
                    granted_at: None,
                    expires_at: Some((now + ChronoDuration::days(20)).to_rfc3339()),
                    redeem_started_at: None,
                    redeemed_at: None,
                    title: None,
                    description: None,
                },
            ],
        };

        // Account B: 90% left, 0 banked resets -> 90% effective
        let acc_b = make_test_account("acc2", "HighNoResets", None);
        let usage_b = UsageInfo {
            account_id: "acc2".into(),
            plan_type: Some("plus".into()),
            primary_used_percent: Some(0.0),
            primary_window_minutes: Some(300),
            primary_resets_at: Some(now.timestamp() + 18000),
            secondary_used_percent: Some(10.0),
            secondary_window_minutes: Some(10080),
            secondary_resets_at: Some(now.timestamp() + 72 * 3600),
            has_credits: None,
            unlimited_credits: None,
            credits_balance: None,
            error: None,
        };

        let score_a = calculate_account_score(
            &acc_a,
            AutoSwitchStrategy::MostRemainingQuota,
            Some(&usage_a),
            Some(&resets_a),
            3,
            now,
        );

        let score_b = calculate_account_score(
            &acc_b,
            AutoSwitchStrategy::MostRemainingQuota,
            Some(&usage_b),
            None,
            3,
            now,
        );

        assert!(
            score_a > score_b,
            "Account A with 2 banked resets (210% effective) must beat Account B (90% effective) under MostRemainingQuota"
        );
    }

    #[test]
    fn test_round_robin_skips_exhausted_weekly_accounts_unless_banked_resets() {
        let acc1 = make_test_account("acc1", "LockedWeekly", None);
        let acc2 = make_test_account("acc2", "HealthyWeekly", None);

        let mut usage_map = HashMap::new();
        let resets_map = HashMap::new();

        // acc1: 99% weekly used, 0 resets -> should be skipped!
        usage_map.insert("acc1".into(), UsageInfo {
            account_id: "acc1".into(),
            plan_type: Some("plus".into()),
            primary_used_percent: Some(0.0),
            primary_window_minutes: Some(300),
            primary_resets_at: None,
            secondary_used_percent: Some(99.0),
            secondary_window_minutes: Some(10080),
            secondary_resets_at: None,
            has_credits: None,
            unlimited_credits: None,
            credits_balance: None,
            error: None,
        });

        // acc2: 20% weekly used -> healthy!
        usage_map.insert("acc2".into(), UsageInfo {
            account_id: "acc2".into(),
            plan_type: Some("plus".into()),
            primary_used_percent: Some(0.0),
            primary_window_minutes: Some(300),
            primary_resets_at: None,
            secondary_used_percent: Some(20.0),
            secondary_window_minutes: Some(10080),
            secondary_resets_at: None,
            has_credits: None,
            unlimited_credits: None,
            credits_balance: None,
            error: None,
        });

        let selected = select_best_account(
            &[acc1.clone(), acc2.clone()],
            None,
            AutoSwitchStrategy::RoundRobin,
            &usage_map,
            &resets_map,
            3,
        );

        assert_eq!(
            selected.unwrap().id,
            "acc2",
            "RoundRobin must skip acc1 because its weekly quota is 99% exhausted and it has no resets"
        );
    }

    #[test]
    fn test_check_rollout_for_errors_lifecycle() {
        use std::io::Write;

        let temp_dir = std::env::temp_dir().join(format!("codex_test_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&temp_dir).unwrap();
        let rollout_file = temp_dir.join("rollout.jsonl");

        // Case 1: Session has a task_complete with capacity error
        {
            let mut f = fs::File::create(&rollout_file).unwrap();
            writeln!(f, r#"{{"type":"event_msg","payload":{{"type":"task_started","turn_id":"turn-1"}}}}"#).unwrap();
            writeln!(f, r#"{{"type":"event_msg","payload":{{"type":"task_complete","turn_id":"turn-1","error":{{"message":"Selected model is at capacity. Please try a different model.","codex_error_info":"server_overloaded"}}}}}}"#).unwrap();
        }

        let detected = check_rollout_for_errors(&rollout_file, "sess-1");
        assert!(detected.is_some());
        let err = detected.unwrap();
        assert_eq!(err.kind, SessionErrorKind::ServerOverloaded);
        assert_eq!(err.turn_id.as_deref(), Some("turn-1"));

        // Case 2: A new turn starts (task_started added) -> session is working, should return None!
        {
            let mut f = fs::OpenOptions::new().append(true).open(&rollout_file).unwrap();
            writeln!(f, r#"{{"type":"event_msg","payload":{{"type":"task_started","turn_id":"turn-2"}}}}"#).unwrap();
            writeln!(f, r#"{{"type":"event_msg","payload":{{"type":"item_completed","turn_id":"turn-2"}}}}"#).unwrap();
        }

        let detected = check_rollout_for_errors(&rollout_file, "sess-1");
        assert!(detected.is_none(), "When a new turn is in progress, check_rollout_for_errors must return None");

        // Case 3: The turn completes successfully (error: null) -> session healthy, should return None!
        {
            let mut f = fs::OpenOptions::new().append(true).open(&rollout_file).unwrap();
            writeln!(f, r#"{{"type":"event_msg","payload":{{"type":"task_complete","turn_id":"turn-2","last_agent_message":"Done!","error":null}}}}"#).unwrap();
        }

        let detected = check_rollout_for_errors(&rollout_file, "sess-1");
        assert!(detected.is_none(), "When latest task completed successfully, check_rollout_for_errors must return None");

        // Case 4: Another turn hits usage limit
        {
            let mut f = fs::OpenOptions::new().append(true).open(&rollout_file).unwrap();
            writeln!(f, r#"{{"type":"event_msg","payload":{{"type":"task_started","turn_id":"turn-3"}}}}"#).unwrap();
            writeln!(f, r#"{{"type":"event_msg","payload":{{"type":"task_complete","turn_id":"turn-3","error":{{"message":"You've hit your usage limit","codex_error_info":"usage_limit_exceeded"}}}}}}"#).unwrap();
        }

        let detected = check_rollout_for_errors(&rollout_file, "sess-1");
        assert!(detected.is_some());
        let err = detected.unwrap();
        assert_eq!(err.kind, SessionErrorKind::UsageLimitExceeded);
        assert_eq!(err.turn_id.as_deref(), Some("turn-3"));

        // Case 5: A turn is started and then aborted by user -> session not in error state, should return None!
        {
            let mut f = fs::OpenOptions::new().append(true).open(&rollout_file).unwrap();
            writeln!(f, r#"{{"type":"event_msg","payload":{{"type":"task_started","turn_id":"turn-4"}}}}"#).unwrap();
            writeln!(f, r#"{{"type":"event_msg","payload":{{"type":"turn_aborted","turn_id":"turn-4"}}}}"#).unwrap();
        }

        let detected = check_rollout_for_errors(&rollout_file, "sess-1");
        assert!(detected.is_none(), "When latest turn was aborted by user, check_rollout_for_errors must return None");

        let _ = fs::remove_dir_all(&temp_dir);
    }

    fn make_test_account_with_plan(id: &str, name: &str, plan: &str, expires_at: Option<DateTime<Utc>>) -> StoredAccount {
        let mut acc = make_test_account(id, name, expires_at);
        acc.plan_type = Some(plan.into());
        acc
    }

    #[test]
    fn test_tier_priority_healthy_plus_beats_prolite_and_pro() {
        let now = Utc.with_ymd_and_hms(2026, 9, 23, 12, 0, 0).unwrap();
        let acc_plus = make_test_account_with_plan("acc_plus", "PlusAcc", "plus", None);
        let acc_prolite = make_test_account_with_plan("acc_prolite", "ProLiteAcc", "prolite", None);
        let acc_pro = make_test_account_with_plan("acc_pro", "ProAcc", "pro", None);

        let usage_healthy = UsageInfo {
            account_id: "any".into(),
            plan_type: None,
            primary_used_percent: Some(0.0),
            primary_window_minutes: Some(300),
            primary_resets_at: Some(now.timestamp() + 5 * 3600),
            secondary_used_percent: Some(0.0),
            secondary_window_minutes: Some(10080),
            secondary_resets_at: Some(now.timestamp() + 120 * 3600),
            has_credits: None,
            unlimited_credits: None,
            credits_balance: None,
            error: None,
        };

        let score_plus = calculate_account_score(&acc_plus, AutoSwitchStrategy::SmartBalanced, Some(&usage_healthy), None, 3, now);
        let score_prolite = calculate_account_score(&acc_prolite, AutoSwitchStrategy::SmartBalanced, Some(&usage_healthy), None, 3, now);
        let score_pro = calculate_account_score(&acc_pro, AutoSwitchStrategy::SmartBalanced, Some(&usage_healthy), None, 3, now);

        assert!(
            score_plus > score_prolite,
            "Plus account ({score_plus}) must have higher score than Pro Lite ({score_prolite}) to preserve expensive tier"
        );
        assert!(
            score_prolite > score_pro,
            "Pro Lite account ({score_prolite}) must have higher score than $200 Pro ({score_pro}) to preserve most expensive tier"
        );
    }

    #[test]
    fn test_tier_priority_prolite_beats_pro_when_plus_exhausted() {
        let now = Utc.with_ymd_and_hms(2026, 9, 23, 12, 0, 0).unwrap();
        let acc_plus = make_test_account_with_plan("acc_plus", "PlusAcc", "plus", None);
        let acc_prolite = make_test_account_with_plan("acc_prolite", "ProLiteAcc", "prolite", None);
        let acc_pro = make_test_account_with_plan("acc_pro", "ProAcc", "pro", None);

        let mut usage_map = HashMap::new();
        let resets_map = HashMap::new();

        // Plus is exhausted (100% 5h limit used)
        usage_map.insert("acc_plus".into(), UsageInfo {
            account_id: "acc_plus".into(),
            plan_type: Some("plus".into()),
            primary_used_percent: Some(100.0),
            primary_window_minutes: Some(300),
            primary_resets_at: Some(now.timestamp() + 3600),
            secondary_used_percent: Some(50.0),
            secondary_window_minutes: Some(10080),
            secondary_resets_at: Some(now.timestamp() + 72 * 3600),
            has_credits: None,
            unlimited_credits: None,
            credits_balance: None,
            error: None,
        });

        // Pro Lite has 80% left on weekly limit (no 5h limit)
        usage_map.insert("acc_prolite".into(), UsageInfo {
            account_id: "acc_prolite".into(),
            plan_type: Some("prolite".into()),
            primary_used_percent: None,
            primary_window_minutes: None,
            primary_resets_at: None,
            secondary_used_percent: Some(20.0),
            secondary_window_minutes: Some(10080),
            secondary_resets_at: Some(now.timestamp() + 158 * 3600),
            has_credits: None,
            unlimited_credits: None,
            credits_balance: None,
            error: None,
        });

        // Pro has 80% left on weekly limit
        usage_map.insert("acc_pro".into(), UsageInfo {
            account_id: "acc_pro".into(),
            plan_type: Some("pro".into()),
            primary_used_percent: None,
            primary_window_minutes: None,
            primary_resets_at: None,
            secondary_used_percent: Some(20.0),
            secondary_window_minutes: Some(10080),
            secondary_resets_at: Some(now.timestamp() + 158 * 3600),
            has_credits: None,
            unlimited_credits: None,
            credits_balance: None,
            error: None,
        });

        let selected = select_best_account(
            &[acc_plus, acc_prolite, acc_pro],
            None,
            AutoSwitchStrategy::SmartBalanced,
            &usage_map,
            &resets_map,
            3,
        );

        assert_eq!(
            selected.unwrap().id,
            "acc_prolite",
            "When Plus accounts are exhausted, Pro Lite should be chosen as the next reserve before $200 Pro"
        );
    }

    #[test]
    fn test_tier_priority_prolite_with_banked_reset_beats_healthy_plus() {
        let now = Utc.with_ymd_and_hms(2026, 9, 23, 12, 0, 0).unwrap();
        let acc_plus = make_test_account_with_plan("acc_plus", "PlusAcc", "plus", None);
        let acc_prolite = make_test_account_with_plan("acc_prolite", "ProLiteAcc", "prolite", None);

        let usage_plus = UsageInfo {
            account_id: "acc_plus".into(),
            plan_type: Some("plus".into()),
            primary_used_percent: Some(0.0),
            primary_window_minutes: Some(300),
            primary_resets_at: Some(now.timestamp() + 5 * 3600),
            secondary_used_percent: Some(0.0),
            secondary_window_minutes: Some(10080),
            secondary_resets_at: Some(now.timestamp() + 120 * 3600),
            has_credits: None,
            unlimited_credits: None,
            credits_balance: None,
            error: None,
        };

        let usage_prolite = UsageInfo {
            account_id: "acc_prolite".into(),
            plan_type: Some("prolite".into()),
            primary_used_percent: None,
            primary_window_minutes: None,
            primary_resets_at: None,
            secondary_used_percent: Some(20.0),
            secondary_window_minutes: Some(10080),
            secondary_resets_at: Some(now.timestamp() + 120 * 3600),
            has_credits: None,
            unlimited_credits: None,
            credits_balance: None,
            error: None,
        };

        let resets_prolite = AccountResetCredits {
            available_count: 1,
            next_expires_at: Some((now + ChronoDuration::days(10)).to_rfc3339()),
            credits: vec![crate::commands::account_stats::AccountResetCredit {
                id: "rc_prolite".into(),
                reset_type: "standard".into(),
                status: "available".into(),
                granted_at: None,
                expires_at: Some((now + ChronoDuration::days(10)).to_rfc3339()),
                redeem_started_at: None,
                redeemed_at: None,
                title: None,
                description: None,
            }],
        };

        let score_plus = calculate_account_score(&acc_plus, AutoSwitchStrategy::SmartBalanced, Some(&usage_plus), None, 3, now);
        let score_prolite = calculate_account_score(&acc_prolite, AutoSwitchStrategy::SmartBalanced, Some(&usage_prolite), Some(&resets_prolite), 3, now);

        assert!(
            score_prolite > score_plus,
            "Pro Lite with banked reset ({score_prolite}) must beat healthy Plus ({score_plus}) so banked reset is utilized"
        );
    }

    #[test]
    fn test_tier_priority_prolite_with_expiring_weekly_burns_before_plus() {
        let now = Utc.with_ymd_and_hms(2026, 9, 23, 12, 0, 0).unwrap();
        let acc_plus = make_test_account_with_plan("acc_plus", "PlusAcc", "plus", None);
        let acc_prolite = make_test_account_with_plan("acc_prolite", "ProLiteAcc", "prolite", None);

        // Plus reset in 5 days (distant)
        let usage_plus = UsageInfo {
            account_id: "acc_plus".into(),
            plan_type: Some("plus".into()),
            primary_used_percent: Some(0.0),
            primary_window_minutes: Some(300),
            primary_resets_at: Some(now.timestamp() + 5 * 3600),
            secondary_used_percent: Some(0.0),
            secondary_window_minutes: Some(10080),
            secondary_resets_at: Some(now.timestamp() + 120 * 3600),
            has_credits: None,
            unlimited_credits: None,
            credits_balance: None,
            error: None,
        };

        // Pro Lite weekly reset in 8 hours with 80% quota left -> burn before reset!
        let usage_prolite = UsageInfo {
            account_id: "acc_prolite".into(),
            plan_type: Some("prolite".into()),
            primary_used_percent: None,
            primary_window_minutes: None,
            primary_resets_at: None,
            secondary_used_percent: Some(20.0),
            secondary_window_minutes: Some(10080),
            secondary_resets_at: Some(now.timestamp() + 8 * 3600),
            has_credits: None,
            unlimited_credits: None,
            credits_balance: None,
            error: None,
        };

        let score_plus = calculate_account_score(&acc_plus, AutoSwitchStrategy::SmartBalanced, Some(&usage_plus), None, 3, now);
        let score_prolite = calculate_account_score(&acc_prolite, AutoSwitchStrategy::SmartBalanced, Some(&usage_prolite), None, 3, now);

        assert!(
            score_prolite > score_plus,
            "Pro Lite with expiring weekly reset ({score_prolite}) must beat healthy Plus ({score_plus}) to burn quota before week rollover"
        );
    }

    #[test]
    fn test_weekly_only_prolite_account_quota_calculation() {
        let now = Utc.with_ymd_and_hms(2026, 9, 23, 12, 0, 0).unwrap();
        let acc_prolite = make_test_account_with_plan("acc_prolite", "ProLiteAcc", "prolite", None);

        // Pro Lite screenshot case: primary is None, secondary is 20% used (80% left)
        let usage_prolite = UsageInfo {
            account_id: "acc_prolite".into(),
            plan_type: Some("prolite".into()),
            primary_used_percent: None,
            primary_window_minutes: None,
            primary_resets_at: None,
            secondary_used_percent: Some(20.0),
            secondary_window_minutes: Some(10080),
            secondary_resets_at: Some(now.timestamp() + 158 * 3600),
            has_credits: None,
            unlimited_credits: None,
            credits_balance: None,
            error: None,
        };

        let score = calculate_account_score(&acc_prolite, AutoSwitchStrategy::SmartBalanced, Some(&usage_prolite), None, 3, now);

        // Immediate left is 80, tier penalty is -3000 -> score is 80 - 3000 = -2920
        assert_eq!(score, 80.0 - 3000.0);
    }

    #[test]
    fn test_goal_resume_phrase_detection_in_db() {
        let temp_dir = std::env::temp_dir().join(format!("test_goal_db_{}", uuid::Uuid::new_v4()));
        let _ = fs::create_dir_all(&temp_dir);
        let db_path = temp_dir.join("goals_1.sqlite");

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "CREATE TABLE thread_goals (
                thread_id TEXT PRIMARY KEY NOT NULL,
                goal_id TEXT NOT NULL,
                objective TEXT NOT NULL,
                status TEXT NOT NULL
            );",
            [],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO thread_goals (thread_id, goal_id, objective, status) VALUES
                ('thread_active', 'g1', 'obj1', 'active'),
                ('thread_paused', 'g2', 'obj2', 'paused'),
                ('thread_usage_limited', 'g3', 'obj3', 'usage_limited'),
                ('thread_blocked', 'g4', 'obj4', 'blocked'),
                ('thread_complete', 'g5', 'obj5', 'complete');",
            [],
        )
        .unwrap();
        drop(conn);

        // Active/paused/usage_limited/blocked goals must evaluate to true
        assert!(is_session_goal_active_in_db(&db_path, "thread_active"));
        assert!(is_session_goal_active_in_db(&db_path, "thread_paused"));
        assert!(is_session_goal_active_in_db(&db_path, "thread_usage_limited"));
        assert!(is_session_goal_active_in_db(&db_path, "thread_blocked"));

        // Completed goal or non-existent thread must evaluate to false
        assert!(!is_session_goal_active_in_db(&db_path, "thread_complete"));
        assert!(!is_session_goal_active_in_db(&db_path, "thread_non_existent"));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_resolve_session_resume_phrase_fallback() {
        // Without an active goal, returns custom phrase or default "continue"
        assert_eq!(
            resolve_session_resume_phrase("non_existent_thread_xyz", "продолжи"),
            "продолжи"
        );
        assert_eq!(
            resolve_session_resume_phrase("non_existent_thread_xyz", "   "),
            "continue"
        );
        assert_eq!(
            resolve_session_resume_phrase("", "продолжи"),
            "продолжи"
        );
    }

    #[test]
    fn test_activate_session_goal_in_db() {
        let temp_dir = std::env::temp_dir().join(format!("test_goal_act_{}", uuid::Uuid::new_v4()));
        let _ = fs::create_dir_all(&temp_dir);
        let db_path = temp_dir.join("goals_1.sqlite");

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "CREATE TABLE thread_goals (
                thread_id TEXT PRIMARY KEY NOT NULL,
                goal_id TEXT NOT NULL,
                objective TEXT NOT NULL,
                status TEXT NOT NULL,
                updated_at_ms INTEGER NOT NULL
            );",
            [],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO thread_goals (thread_id, goal_id, objective, status, updated_at_ms) VALUES
                ('thread_paused', 'g2', 'obj2', 'paused', 100),
                ('thread_usage_limited', 'g3', 'obj3', 'usage_limited', 200),
                ('thread_complete', 'g5', 'obj5', 'complete', 300);",
            [],
        )
        .unwrap();
        drop(conn);

        assert!(activate_session_goal_in_db(&db_path, "thread_usage_limited"));
        assert!(activate_session_goal_in_db(&db_path, "thread_paused"));
        assert!(!activate_session_goal_in_db(&db_path, "thread_complete"));
        assert!(!activate_session_goal_in_db(&db_path, "non_existent"));

        let conn = rusqlite::Connection::open(&db_path).unwrap();
        let status1: String = conn.query_row("SELECT status FROM thread_goals WHERE thread_id = 'thread_usage_limited'", [], |r| r.get(0)).unwrap();
        let status2: String = conn.query_row("SELECT status FROM thread_goals WHERE thread_id = 'thread_paused'", [], |r| r.get(0)).unwrap();
        let status3: String = conn.query_row("SELECT status FROM thread_goals WHERE thread_id = 'thread_complete'", [], |r| r.get(0)).unwrap();
        assert_eq!(status1, "active");
        assert_eq!(status2, "active");
        assert_eq!(status3, "complete");

        let _ = fs::remove_dir_all(&temp_dir);
    }

}
