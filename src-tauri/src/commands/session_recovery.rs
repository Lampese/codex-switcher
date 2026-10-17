//! Snapshot and continue Codex sessions stopped by a usage limit.

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
    let sessions = find_quota_stopped_sessions();
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
                .kill_on_drop(true)
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

fn find_quota_stopped_sessions() -> Vec<String> {
    let Ok(home) = crate::auth::get_codex_home() else {
        return Vec::new();
    };
    let root = home.join("sessions");
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
            && has_usage_limit_failure(&path)
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

fn has_usage_limit_failure(path: &Path) -> bool {
    let Ok(file) = fs::File::open(path) else {
        return false;
    };
    let mut read_failed = false;
    let blocked =
        has_usage_limit_failure_lines(BufReader::new(file).lines().map_while(|line| match line {
            Ok(line) => Some(line),
            Err(_) => {
                read_failed = true;
                None
            }
        }));
    blocked && !read_failed
}

fn has_usage_limit_failure_lines<I>(lines: I) -> bool
where
    I: IntoIterator<Item = String>,
{
    let mut quota_stopped = false;
    for line in lines {
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            // A partial record may contain a later resume or completion.
            quota_stopped = false;
            continue;
        };
        let payload = &value["payload"];
        if value["type"] == "response_item"
            && payload["type"] == "message"
            && payload["role"] == "user"
        {
            quota_stopped = false;
        }
        // Never match error text quoted in messages, tool results, or metadata.
        if value.get("type").and_then(Value::as_str) != Some("event_msg") {
            continue;
        }
        match payload["type"].as_str() {
            Some("task_started" | "turn_started" | "user_message" | "turn_aborted") => {
                quota_stopped = false;
            }
            Some("task_complete" | "turn_failed") => {
                quota_stopped = is_usage_limit_error(&payload["error"]);
            }
            Some("error") => {
                quota_stopped = is_usage_limit_error(payload);
            }
            _ => {}
        }
    }
    quota_stopped
}

fn is_usage_limit_error(error: &Value) -> bool {
    // Prefer the structured code: the displayed message varies by plan/workspace.
    matches!(
        error.get("codex_error_info").and_then(Value::as_str),
        Some("usage_limit_exceeded")
    ) || error
        .get("message")
        .and_then(Value::as_str)
        .is_some_and(|message| {
            message.trim().starts_with("You've hit your usage limit.")
                || message.trim().starts_with("You’ve hit your usage limit.")
        })
}

#[cfg(test)]
mod tests {
    use super::has_usage_limit_failure_lines;
    use serde_json::{json, Value};

    fn event(payload: Value) -> String {
        json!({"type": "event_msg", "payload": payload}).to_string()
    }

    fn quota_failure() -> String {
        event(json!({"type": "task_complete", "error": {
            "message": "You've hit your usage limit. Review your workspace's usage settings to continue.",
            "codex_error_info": "usage_limit_exceeded"
        }}))
    }

    #[test]
    fn detects_quota_failure_even_when_recorded_as_task_complete() {
        assert!(has_usage_limit_failure_lines([
            event(json!({"type": "task_started"})),
            quota_failure(),
            event(json!({"type": "token_count"}))
        ]));
    }

    #[test]
    fn recognizes_structured_code_and_message_fallback() {
        for error in [
            json!({"codex_error_info": "usage_limit_exceeded", "message": "Localized message"}),
            json!({"message": "You've hit your usage limit. Review your workspace's usage settings to continue."}),
            json!({"message": "You’ve hit your usage limit. Try again later."}),
        ] {
            for kind in ["task_complete", "turn_failed"] {
                assert!(has_usage_limit_failure_lines([event(
                    json!({"type": kind, "error": error})
                )]));
            }
            let mut payload = error;
            payload["type"] = json!("error");
            assert!(has_usage_limit_failure_lines([event(payload)]));
        }
    }

    #[test]
    fn skips_active_completed_cancelled_and_unrelated_failures() {
        for payload in [
            json!({"type": "task_started"}),
            json!({"type": "turn_started"}),
            json!({"type": "user_message", "message": "Continue"}),
            json!({"type": "task_complete", "error": null}),
            json!({"type": "turn_aborted"}),
            json!({"type": "turn_failed", "error": {"message": "Network error"}}),
            json!({"type": "error", "message": "Rate limit exceeded"}),
        ] {
            assert!(!has_usage_limit_failure_lines([event(payload.clone())]));
            assert!(!has_usage_limit_failure_lines([
                quota_failure(),
                event(payload)
            ]));
        }
    }

    #[test]
    fn ignores_quoted_error_text_and_tool_output() {
        let message =
            "You've hit your usage limit. Review your workspace's usage settings to continue.";
        for record in [
            json!({"type": "response_item", "payload": {"type": "message", "role": "user", "content": message}}),
            json!({"type": "event_msg", "payload": {"type": "agent_message", "message": message}}),
            json!({"type": "event_msg", "payload": {"type": "item_completed", "item": {"aggregated_output": message}}}),
            json!({"type": "response_item", "payload": {"type": "error", "message": message}}),
        ] {
            assert!(!has_usage_limit_failure_lines([record.to_string()]));
        }
    }

    #[test]
    fn latest_failure_wins_and_partial_record_fails_closed() {
        assert!(has_usage_limit_failure_lines([
            quota_failure(),
            event(json!({"type": "task_started"})),
            quota_failure()
        ]));
        assert!(!has_usage_limit_failure_lines([
            quota_failure(),
            "{partial".into()
        ]));
        assert!(!has_usage_limit_failure_lines([
            quota_failure(),
            json!({"type": "response_item", "payload": {"type": "message", "role": "user", "content": "Continue"}}).to_string()
        ]));
    }
}
