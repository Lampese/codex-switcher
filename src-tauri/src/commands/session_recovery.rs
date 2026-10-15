//! Snapshot and continue Codex sessions interrupted by account switching.

use serde::Serialize;
use serde_json::Value;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::sync::Mutex;
use std::time::SystemTime;

const ACTIVE_WINDOW_SECS: u64 = 24 * 60 * 60;
const MAX_SESSIONS: usize = 32;
const CONTINUE_PROMPT: &str = "Continue the previous task.";

static PENDING_SESSIONS: Mutex<Vec<String>> = Mutex::new(Vec::new());

#[derive(Debug, Clone, Serialize)]
pub struct SessionRecoveryReport {
    pub queued: Vec<String>,
    pub failed: Vec<String>,
}

#[tauri::command]
pub fn prepare_codex_session_recovery() -> Result<usize, String> {
    let sessions = find_incomplete_sessions();
    let count = sessions.len();
    *PENDING_SESSIONS.lock().map_err(|error| error.to_string())? = sessions;
    Ok(count)
}

#[tauri::command]
pub fn cancel_codex_session_recovery() -> Result<(), String> {
    PENDING_SESSIONS
        .lock()
        .map_err(|error| error.to_string())?
        .clear();
    Ok(())
}

#[tauri::command]
pub async fn resume_codex_session_recovery() -> Result<SessionRecoveryReport, String> {
    let sessions =
        std::mem::take(&mut *PENDING_SESSIONS.lock().map_err(|error| error.to_string())?);
    let mut report = SessionRecoveryReport {
        queued: Vec::new(),
        failed: Vec::new(),
    };
    let binary = std::env::var_os("CODEX_BIN").unwrap_or_else(|| "codex".into());

    for session_id in sessions {
        let result = tokio::time::timeout(
            tokio::time::Duration::from_secs(10),
            tokio::process::Command::new(&binary)
                .args([
                    "queue",
                    "--thread",
                    session_id.as_str(),
                    "--message",
                    CONTINUE_PROMPT,
                ])
                .output(),
        )
        .await;

        match result {
            Ok(Ok(output)) if output.status.success() => report.queued.push(session_id),
            Ok(Ok(output)) => {
                let detail = String::from_utf8_lossy(&output.stderr)
                    .lines()
                    .next()
                    .unwrap_or("codex queue failed")
                    .to_string();
                report.failed.push(format!("{session_id}: {detail}"));
            }
            Ok(Err(error)) => report.failed.push(format!("{session_id}: {error}")),
            Err(_) => report
                .failed
                .push(format!("{session_id}: codex queue timed out")),
        }
    }
    Ok(report)
}

fn find_incomplete_sessions() -> Vec<String> {
    let Some(root) = dirs::home_dir().map(|home| home.join(".codex").join("sessions")) else {
        return Vec::new();
    };
    let now = SystemTime::now();
    let mut sessions = Vec::new();
    collect_jsonl(&root, &now, &mut sessions);
    sessions.sort_by(|(left, _), (right, _)| right.cmp(left));
    sessions
        .into_iter()
        .take(MAX_SESSIONS)
        .map(|(_, id)| id)
        .collect()
}

fn collect_jsonl(root: &Path, now: &SystemTime, sessions: &mut Vec<(SystemTime, String)>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl(&path, now, sessions);
        } else if path.extension().and_then(|value| value.to_str()) == Some("jsonl")
            && is_recent(&path, now)
            && has_unfinished_turn(&path)
        {
            if let (Some(modified), Some(id)) = (
                fs::metadata(&path)
                    .ok()
                    .and_then(|metadata| metadata.modified().ok()),
                session_id(&path),
            ) {
                sessions.push((modified, id));
            }
        }
    }
}

fn is_recent(path: &Path, now: &SystemTime) -> bool {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| now.duration_since(modified).ok())
        .is_some_and(|age| age.as_secs() <= ACTIVE_WINDOW_SECS)
}

fn session_id(path: &Path) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    let line = BufReader::new(file).lines().next()?.ok()?;
    let value: Value = serde_json::from_str(&line).ok()?;
    if value.get("type")?.as_str()? != "session_meta" {
        return None;
    }
    value.get("payload")?.get("id")?.as_str().map(str::to_owned)
}

fn has_unfinished_turn(path: &Path) -> bool {
    let Ok(file) = fs::File::open(path) else {
        return false;
    };
    has_unfinished_turn_lines(BufReader::new(file).lines().filter_map(Result::ok))
}

fn has_unfinished_turn_lines<I>(lines: I) -> bool
where
    I: IntoIterator<Item = String>,
{
    let mut last_started = None;
    let mut last_terminal = None;
    for (index, line) in lines.into_iter().enumerate() {
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let event_type = value
            .get("payload")
            .and_then(|payload| payload.get("type"))
            .and_then(Value::as_str);
        match event_type {
            Some("task_started") => last_started = Some(index),
            Some("task_complete") | Some("turn_aborted") | Some("turn_failed") => {
                last_terminal = Some(index)
            }
            _ => {}
        }
    }
    last_started.is_some() && last_started > last_terminal
}

#[cfg(test)]
mod tests {
    use super::has_unfinished_turn_lines;

    #[test]
    fn detects_only_unfinished_turns() {
        let started = r#"{"type":"event_msg","payload":{"type":"task_started"}}"#;
        let completed = r#"{"type":"event_msg","payload":{"type":"task_complete"}}"#;
        assert!(has_unfinished_turn_lines([started.to_string()]));
        assert!(!has_unfinished_turn_lines([
            started.to_string(),
            completed.to_string()
        ]));
    }
}
