//! Capture only unfinished turns owned by processes the switch will close.

use serde_json::Value;
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub(super) struct RunningTurn {
    offset: u64,
}

pub(super) async fn open_rollouts() -> Result<HashSet<PathBuf>, String> {
    #[cfg(target_os = "macos")]
    {
        let pids = tauri::async_runtime::spawn_blocking(super::process::recovery_process_targets)
            .await
            .map_err(|error| error.to_string())??;
        if pids.is_empty() {
            return Ok(HashSet::new());
        }
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            tokio::process::Command::new("/usr/sbin/lsof")
                .args([
                    "-n",
                    "-P",
                    "-a",
                    "-p",
                    &pids
                        .iter()
                        .map(u32::to_string)
                        .collect::<Vec<_>>()
                        .join(","),
                    "-Fn",
                ])
                .kill_on_drop(true)
                .output(),
        )
        .await
        .map_err(|_| "Inspecting running Codex chats timed out")?
        .map_err(|error| error.to_string())?;
        let inventory = String::from_utf8_lossy(&output.stdout);
        // lsof returns 1 when a short-lived child exits while the inventory is read.
        if !output.status.success() && !inventory.lines().any(|line| line.starts_with('p')) {
            return Err("Could not inspect running Codex chats before switching".into());
        }
        Ok(parse_open_rollouts(&inventory))
    }
    // Other platforms retain quota-error recovery until live ownership is available.
    #[cfg(not(target_os = "macos"))]
    Ok(HashSet::new())
}

#[cfg(any(target_os = "macos", test))]
fn parse_open_rollouts(output: &str) -> HashSet<PathBuf> {
    output
        .lines()
        .filter_map(|line| line.strip_prefix('n'))
        .map(PathBuf::from)
        .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
        .collect()
}

pub(super) fn capture(path: &Path) -> Option<RunningTurn> {
    let file = std::fs::File::open(path).ok()?;
    let offset = file.metadata().ok()?.len();
    let lines = BufReader::new(file.take(offset)).lines();
    let mut active = false;
    let mut waiting = HashSet::new();
    for line in lines {
        let value: Value = serde_json::from_str(&line.ok()?).ok()?;
        let payload = &value["payload"];
        if value["type"] == "event_msg" {
            match payload["type"].as_str() {
                Some("task_started" | "turn_started") => {
                    active = true;
                    waiting.clear();
                }
                Some("task_complete" | "turn_failed" | "turn_aborted" | "error") => active = false,
                _ => {}
            }
        }
        if value["type"] == "response_item" && active {
            match payload["type"].as_str() {
                Some("function_call" | "custom_tool_call") => {
                    let name = payload["name"].as_str().unwrap_or("");
                    let args = payload["arguments"]
                        .as_str()
                        .or_else(|| payload["input"].as_str())
                        .unwrap_or("");
                    // User-input tools are direct calls, not source text quoted inside exec.
                    let question = name.contains("request_user_input");
                    let approval = args.contains("require_escalated")
                        || args.contains("with_escalated_permissions");
                    if question || approval {
                        waiting.insert((
                            payload["call_id"].as_str()?.to_owned(),
                            question && name.contains("_async"),
                        ));
                    }
                }
                Some("function_call_output" | "custom_tool_call_output") => {
                    if let Some(id) = payload["call_id"].as_str() {
                        waiting.retain(|(call, asynchronous)| call != id || *asynchronous);
                    }
                }
                Some("message")
                    if payload["role"] == "assistant"
                        && matches!(payload["phase"].as_str(), Some("final" | "final_answer")) =>
                {
                    active = false
                }
                _ => {}
            }
        }
    }
    (active && waiting.is_empty()).then_some(RunningTurn { offset })
}

pub(super) fn still_interrupted(path: &Path, snapshot: &RunningTurn) -> bool {
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    if file
        .metadata()
        .map_or(true, |meta| meta.len() < snapshot.offset)
        || file.seek(SeekFrom::Start(snapshot.offset)).is_err()
    {
        return false;
    }
    for line in BufReader::new(file).lines() {
        let Ok(line) = line else {
            return false;
        };
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            return false;
        };
        let p = &value["payload"];
        if value["type"] == "event_msg"
            && matches!(
                p["type"].as_str(),
                Some(
                    "task_started"
                        | "turn_started"
                        | "user_message"
                        | "task_complete"
                        | "turn_failed"
                        | "turn_aborted"
                        | "error"
                )
            )
        {
            return false;
        }
        if value["type"] == "response_item"
            && p["type"] == "message"
            && (p["role"] == "user"
                || (p["role"] == "assistant"
                    && matches!(p["phase"].as_str(), Some("final" | "final_answer"))))
        {
            return false;
        }
    }
    capture(path).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn event(kind: &str) -> Value {
        json!({"type":"event_msg","payload":{"type":kind}})
    }
    fn write(path: &Path, values: &[Value]) {
        std::fs::write(
            path,
            values
                .iter()
                .map(|value| format!("{value}\n"))
                .collect::<String>(),
        )
        .unwrap();
    }
    #[test]
    fn captures_running_but_not_terminal_or_waiting_turns() {
        let _home = crate::auth::test_support::TestHome::new();
        let path = crate::auth::get_codex_home().unwrap().join("running.jsonl");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        write(&path, &[event("task_started")]);
        assert!(capture(&path).is_some());
        for end in ["task_complete", "turn_aborted", "turn_failed", "error"] {
            write(&path, &[event("task_started"), event(end)]);
            assert!(capture(&path).is_none(), "{end}");
        }
        for name in ["request_user_input", "functions.request_user_input_async"] {
            write(
                &path,
                &[
                    event("task_started"),
                    json!({"type":"response_item","payload":{"type":"function_call","name":name,"call_id":"q"}}),
                ],
            );
            assert!(capture(&path).is_none());
        }
        write(
            &path,
            &[
                event("task_started"),
                json!({"type":"response_item","payload":{"type":"message","role":"assistant","phase":"final_answer"}}),
            ],
        );
        assert!(capture(&path).is_none());
        // Mentioning the API in ordinary tool arguments is not a pending question.
        write(
            &path,
            &[
                event("task_started"),
                json!({"type":"response_item","payload":{"type":"function_call","name":"functions.exec","arguments":"rg request_user_input_async README.md","call_id":"read"}}),
            ],
        );
        assert!(capture(&path).is_some());
    }
    #[test]
    fn does_not_resume_a_new_completed_or_manually_stopped_turn() {
        let _home = crate::auth::test_support::TestHome::new();
        let path = crate::auth::get_codex_home().unwrap().join("running.jsonl");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        write(&path, &[event("task_started")]);
        let snapshot = capture(&path).unwrap();
        assert!(still_interrupted(&path, &snapshot));
        for end in [
            "task_complete",
            "turn_aborted",
            "task_started",
            "user_message",
        ] {
            write(&path, &[event("task_started"), event(end)]);
            assert!(!still_interrupted(&path, &snapshot), "{end}");
        }
    }
    #[test]
    fn open_file_inventory_only_accepts_jsonl_names() {
        assert_eq!(
            parse_open_rollouts("p123\nf15\nn/tmp/a.jsonl\nn/tmp/auth.json\nn/tmp/a.jsonl\n"),
            HashSet::from([PathBuf::from("/tmp/a.jsonl")])
        );
    }
}
