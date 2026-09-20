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

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::auth::{
    get_accounts_file, load_accounts, load_app_settings, save_app_settings,
    switch_to_account,
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
    /// Last account switch timestamp and target account ID (to prevent cascading switches across multiple active sessions)
    last_account_switch: Option<(Instant, String)>,
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

/// Find active Codex CLI sessions by inspecting thread-writer-locks and running processes
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
        let pid = find_pid_for_session(&session_id, &path).unwrap_or(0);
        let rollout_path = locate_rollout_file(&home, &session_id);
        let cwd = if pid > 0 {
            get_process_cwd(pid)
        } else {
            None
        }.or_else(|| rollout_path.as_deref().and_then(extract_cwd_from_rollout));

        let is_managed = pid > 0 && managed_pids.contains(&pid);

        let mut session = ActiveCodexSession {
            session_id: session_id.clone(),
            pid,
            cwd,
            rollout_path: rollout_path.as_ref().map(|p| p.to_string_lossy().to_string()),
            last_error: None,
            is_managed,
        };

        if let Some(ref r_path) = rollout_path {
            session.last_error = check_rollout_for_errors(r_path, &session_id);
        }

        sessions.push(session);
    }

    Ok(sessions)
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

/// Extract working directory from rollout jsonl file if process is no longer inspectable
fn extract_cwd_from_rollout(rollout_path: &Path) -> Option<String> {
    use std::io::{BufRead, BufReader};
    let file = fs::File::open(rollout_path).ok()?;
    let reader = BufReader::new(file);

    for line in reader.lines().take(50).filter_map(Result::ok) {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&line) {
            if let Some(cwd) = val
                .get("payload")
                .and_then(|p| p.get("thread_settings"))
                .and_then(|ts| ts.get("cwd"))
                .and_then(|c| c.as_str())
            {
                return Some(cwd.to_string());
            }
            if let Some(cwd) = val
                .get("payload")
                .and_then(|p| p.get("cwd"))
                .and_then(|c| c.as_str())
            {
                return Some(cwd.to_string());
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
                        if cmdline.contains(session_id) && !cmdline.contains("codex-switcher") {
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
                        return Some(pid);
                    }
                }
            }
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

        if !line.contains("\"task_started\"") && !line.contains("\"task_complete\"") {
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
        // It is NOT in an error state.
        if event_type == Some("task_started") {
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

    let used_percent = usage
        .and_then(|u| u.primary_used_percent)
        .unwrap_or(0.0);

    // If completely full (>= 100%), penalize heavily unless no other choice exists
    if used_percent >= 100.0 {
        score -= 50000.0;
    } else {
        // Base remaining quota score (0 to 100 points)
        score += (100.0 - used_percent).max(0.0);
    }

    // Evaluate available resets
    let mut has_urgent_reset = false;
    let mut closest_reset_days: Option<f64> = None;
    if let Some(credits) = resets {
        for credit in &credits.credits {
            if credit.status.to_lowercase() != "available" {
                continue;
            }
            if let Some(exp_str) = credit.expires_at.as_deref() {
                if let Ok(exp) = DateTime::parse_from_rfc3339(exp_str) {
                    let exp_utc = exp.with_timezone(&Utc);
                    let diff_secs = (exp_utc - now).num_seconds();
                    if diff_secs > 0 {
                        let diff_days = diff_secs as f64 / 86400.0;
                        if diff_days <= warning_days as f64 {
                            has_urgent_reset = true;
                        }
                        closest_reset_days = Some(
                            closest_reset_days
                                .map_or(diff_days, |current| current.min(diff_days)),
                        );
                    }
                }
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

    match strategy {
        AutoSwitchStrategy::SmartBalanced => {
            // 1. Prioritize accounts with expiring reset credits so they don't go to waste
            if has_urgent_reset {
                if let Some(days) = closest_reset_days {
                    score += 20000.0 + (warning_days as f64 - days).max(0.0) * 500.0;
                } else {
                    score += 20000.0;
                }
            }

            // 2. Prioritize subscriptions expiring soon or in grace period
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
            if let Some(days) = closest_reset_days {
                score += 30000.0 - (days * 100.0);
            } else if has_urgent_reset {
                score += 25000.0;
            }
        }
        AutoSwitchStrategy::ExpiringSubscriptionFirst => {
            if let Some(days) = sub_diff_days {
                if days <= 0.0 {
                    score += 25000.0;
                } else {
                    score += 20000.0 - (days * 50.0);
                }
            }
        }
        AutoSwitchStrategy::MostRemainingQuota => {
            // Primarily dictated by (100.0 - used_percent)
            score *= 10.0;
        }
        AutoSwitchStrategy::RoundRobin => {
            // Neutral scoring, order handled by caller
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
        // Pick first candidate with available limit
        for candidate in &candidates {
            let used = usage_map
                .get(&candidate.id)
                .and_then(|u| u.primary_used_percent)
                .unwrap_or(0.0);
            if used < 95.0 {
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

/// Send resume message via codex queue command
pub async fn send_codex_queue_resume(session_id: &str, phrase: &str) -> Result<()> {
    let codex_bin = find_codex_binary();
    let mut command = Command::new(codex_bin);
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
        let _ = Command::new("notify-send")
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
        if let Ok(output) = Command::new("wmctrl").args(["-l", "-p"]).output() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 3 && parts[2] == pid.to_string() {
                    let win_id = parts[0];
                    let _ = Command::new("wmctrl").args(["-i", "-a", win_id]).status();
                    return;
                }
            }
        }

        // 2. Fallback: try xdotool by PID
        if let Ok(output) = Command::new("xdotool").args(["search", "--pid", &pid.to_string()]).output() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if let Some(win_id) = stdout.lines().last() {
                if !win_id.trim().is_empty() {
                    let _ = Command::new("xdotool").args(["windowactivate", win_id.trim()]).status();
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
    let restart_file_str = restart_file.to_string_lossy().to_string();

    let default_cwd = cwd
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."));

    // Check preferred or detected terminal
    #[cfg(target_os = "linux")]
    {
        let terminal = preferred_terminal
            .and_then(which_cmd)
            .or_else(|| which_cmd("ghostty"))
            .or_else(|| which_cmd("alacritty"))
            .or_else(|| which_cmd("kitty"))
            .or_else(|| which_cmd("gnome-terminal"))
            .or_else(|| which_cmd("x-terminal-emulator"));

        let runner_script = r#"
CODEX_BIN="$1"
SESSION_ID="$2"
CURRENT_PHRASE="$3"
RESTART_FILE="$4"

while true; do
    "$CODEX_BIN" resume "$SESSION_ID" "$CURRENT_PHRASE"
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

        if let Some(term_path) = terminal {
            let term_name = term_path.file_name().unwrap_or_default().to_string_lossy();
            let mut cmd = Command::new(&term_path);

            let args_bundle = [
                "sh",
                "-c",
                runner_script,
                "sh",
                &codex_bin.to_string_lossy(),
                session_id,
                phrase,
                &restart_file_str,
            ];

            if term_name.contains("ghostty") {
                cmd.arg(format!("--working-directory={}", default_cwd.display()))
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

            let child = cmd.spawn().context("Failed to spawn terminal")?;
            let pid = child.id();
            focus_terminal_window(pid);
            if let Ok(mut state) = TRACKER.lock() {
                state.managed_pids.push(pid);
            }
            return Ok(pid);
        }
    }

    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "tell application \"Terminal\" to do script \"cd {} && while true; do {} resume {} {}; if [ -f {} ]; then rm -f {}; sleep 1; continue; fi; break; done; exit\"",
            default_cwd.display(),
            codex_bin.to_string_lossy(),
            session_id,
            phrase,
            restart_file_str,
            restart_file_str
        );
        let mut cmd = Command::new("osascript");
        cmd.arg("-e").arg(script);
        let child = cmd.spawn().context("Failed to spawn macOS Terminal")?;
        return Ok(child.id());
    }

    #[cfg(windows)]
    {
        let codex_cmd = format!(
            "{} resume {} {}",
            escape_shell_arg(&codex_bin.to_string_lossy()),
            escape_shell_arg(session_id),
            escape_shell_arg(phrase)
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
        // If the session is healthy (actively executing or completed normally),
        // clear its capacity retry counter so future errors start fresh.
        if session.last_error.is_none() {
            if let Ok(mut tracker) = TRACKER.lock() {
                tracker.capacity_retries.remove(&session.session_id);
            }
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
                let mut attempt_number = 1;

                {
                    let mut tracker = TRACKER
                        .lock()
                        .map_err(|_| anyhow::anyhow!("Tracker poisoned"))?;
                    
                    // entry: (last_retried_turn_id, consecutive_attempts, last_attempt_time)
                    let entry = tracker
                        .capacity_retries
                        .entry(session.session_id.clone())
                        .or_insert((None, 0, Instant::now() - Duration::from_secs(60)));

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
                        } else {
                            // Consecutive retry limit reached
                            continue;
                        }
                    }
                }

                if should_retry {
                    let phrase = if settings.continue_phrase.trim().is_empty() {
                        "continue"
                    } else {
                        &settings.continue_phrase
                    };

                    send_codex_queue_resume(&session.session_id, phrase).await?;

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

    if let Some((switch_time, _)) = recent_switch {
        if switch_time.elapsed() < Duration::from_secs(20) {
            let phrase = if settings.continue_phrase.trim().is_empty() {
                "continue"
            } else {
                &settings.continue_phrase
            };

            let restart_file = std::env::temp_dir()
                .join(format!("codex-switcher-restart-{}", session.session_id));
            let _ = fs::write(&restart_file, phrase);

            // Terminate stale process that still holds old credentials in memory
            if session.pid > 0 {
                terminate_process(session.pid);
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
                    phrase,
                    settings.preferred_terminal.as_deref(),
                );
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

    // Fetch usage and stats cache
    let mut usage_map = HashMap::new();
    let mut resets_map = HashMap::new();

    for acc in &store.accounts {
        if let Ok(u) = crate::commands::usage::fetch_usage(&acc.id).await {
            usage_map.insert(acc.id.clone(), u);
        }
        if let Ok(stats) = crate::commands::account_stats::get_account_usage_stats(acc.id.clone()).await {
            if let Some(resets) = stats.reset_credits {
                resets_map.insert(acc.id.clone(), resets);
            }
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

    // Switch account credentials in auth.json
    switch_to_account(&target)?;
    let mut updated_store = store;
    updated_store.active_account_id = Some(target.id.clone());
    let accounts_path = get_accounts_file()?;
    if let Ok(content) = serde_json::to_string_pretty(&updated_store) {
        let _ = fs::write(&accounts_path, content);
    }

    let phrase = if settings.continue_phrase.trim().is_empty() {
        "continue"
    } else {
        &settings.continue_phrase
    };

    let restart_file = std::env::temp_dir()
        .join(format!("codex-switcher-restart-{}", session.session_id));
    let _ = fs::write(&restart_file, phrase);

    // Codex CLI caches JWT tokens in process memory (CachedAuth) for the entire lifetime
    // of the process. In-place queue messages on an existing process will reuse the stale token
    // and fail again. We must terminate the old process and relaunch with `codex resume`
    // so the new process initializes fresh AuthManager from the newly written auth.json.
    if session.pid > 0 {
        terminate_process(session.pid);
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
            phrase,
            settings.preferred_terminal.as_deref(),
        );
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
        tracker.last_account_switch = Some((Instant::now(), target.id.clone()));
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
    let phrase = prompt.unwrap_or_else(|| settings.continue_phrase);
    let s_id = session_id.unwrap_or_else(|| "".to_string());

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

/// Save auto-recovery configuration
#[tauri::command]
pub async fn save_auto_recovery_settings(
    auto_retry_capacity_enabled: bool,
    auto_retry_capacity_max_attempts: u32,
    auto_retry_capacity_initial_delay_sec: u32,
    auto_retry_capacity_escalate_to_switch: bool,
    auto_switch_limit_enabled: bool,
    auto_switch_strategy: AutoSwitchStrategy,
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

    fn make_test_account(id: &str, name: &str, expires_at: Option<DateTime<Utc>>) -> StoredAccount {
        StoredAccount::new_chatgpt(
            name.into(),
            Some(format!("{name}@example.com")),
            Some("plus".into()),
            expires_at,
            "header.payload.sig".into(),
            "access".into(),
            "refresh".into(),
            Some(id.into()),
        )
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

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
