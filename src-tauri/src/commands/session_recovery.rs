//! Snapshot and continue Codex sessions stopped by a usage limit.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

const ACTIVE_WINDOW_SECS: u64 = 24 * 60 * 60;
const MAX_SESSIONS: usize = 32;
const RECOVERY_BUDGET: std::time::Duration = std::time::Duration::from_secs(120);
const CONTINUE_PROMPT: &str = "Continue the previous task.";

#[derive(Debug, Clone)]
struct StoppedSession {
    id: String,
    path: PathBuf,
    running: Option<super::running_recovery::RunningTurn>,
}

#[derive(Debug, Serialize, Deserialize)]
struct RecoveryReceipt {
    offset: u64,
    prompt: String,
    queued: bool,
    #[serde(default)]
    queue_error: Option<String>,
}

static PENDING_SESSIONS: Mutex<Vec<StoppedSession>> = Mutex::new(Vec::new());
static RECOVERY_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Debug, Clone, Serialize)]
pub struct SessionRecoveryReport {
    pub resumed: Vec<String>,
    pub skipped: Vec<String>,
    pub failed: Vec<RecoveryFailure>,
    pub timed_out: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecoveryFailure {
    pub session_id: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecoveryProgress {
    pub completed: usize,
    pub total: usize,
    pub current_session_id: Option<String>,
}

#[tauri::command]
pub async fn prepare_codex_session_recovery() -> Result<usize, String> {
    let open = super::running_recovery::open_rollouts().await?;
    prepare_sessions(&open)
}

fn prepare_sessions(open: &std::collections::HashSet<PathBuf>) -> Result<usize, String> {
    let sessions = find_recoverable_sessions(&open);
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
pub async fn resume_codex_session_recovery(
    on_progress: tauri::ipc::Channel<RecoveryProgress>,
) -> Result<SessionRecoveryReport, String> {
    resume_pending_sessions(|progress| {
        let _ = on_progress.send(progress);
    })
    .await
}

async fn resume_pending_sessions(
    on_progress: impl Fn(RecoveryProgress),
) -> Result<SessionRecoveryReport, String> {
    let _guard = RECOVERY_LOCK.lock().await;
    let sessions =
        std::mem::take(&mut *PENDING_SESSIONS.lock().map_err(|error| error.to_string())?);
    let home = crate::auth::get_codex_home().map_err(|error| error.to_string())?;
    Ok(recover_sessions(
        sessions,
        &home,
        &codex_binary(),
        open_desktop_thread,
        on_progress,
        RECOVERY_BUDGET,
    )
    .await)
}

async fn recover_sessions<F, Fut>(
    sessions: Vec<StoppedSession>,
    home: &Path,
    binary: &Path,
    open_thread: F,
    on_progress: impl Fn(RecoveryProgress),
    budget: std::time::Duration,
) -> SessionRecoveryReport
where
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    let mut report = SessionRecoveryReport {
        resumed: Vec::new(),
        skipped: Vec::new(),
        failed: Vec::new(),
        timed_out: false,
    };
    let deadline = tokio::time::Instant::now() + budget;
    let total = sessions.len();
    for (index, session) in sessions.into_iter().enumerate() {
        if tokio::time::Instant::now() >= deadline {
            report.timed_out = true;
            report.failed.push(RecoveryFailure {
                session_id: session.id,
                reason: "Not attempted: the recovery time limit was reached. Open this conversation to inspect its state and any pending queue.".into(),
            });
            continue;
        }
        on_progress(RecoveryProgress {
            completed: index,
            total,
            current_session_id: Some(session.id.clone()),
        });
        // The user or another client may have continued it during the switch.
        if !has_usage_limit_failure(&session.path)
            && !session.running.as_ref().is_some_and(|snapshot| {
                super::running_recovery::still_interrupted(&session.path, snapshot)
            })
        {
            report.skipped.push(session.id);
        } else {
            // Dropping an overdue attempt keeps its receipt and kills any CLI child.
            let result = tokio::time::timeout_at(
                deadline,
                resume_session(&session, home, binary, &open_thread, 60),
            )
            .await;
            match result {
                Ok(Ok(())) => report.resumed.push(session.id),
                Ok(Err(reason)) => report.failed.push(RecoveryFailure {
                    session_id: session.id,
                    reason,
                }),
                Err(_) => {
                    report.timed_out = true;
                    report.failed.push(RecoveryFailure {
                        session_id: session.id,
                        reason: "The recovery time limit was reached before this turn was confirmed. A continuation may still be queued; open the conversation to inspect it. Its receipt is kept to prevent duplicate messages.".into(),
                    });
                }
            }
        }
        on_progress(RecoveryProgress {
            completed: index + 1,
            total,
            current_session_id: None,
        });
    }
    on_progress(RecoveryProgress {
        completed: total,
        total,
        current_session_id: None,
    });
    report
}

#[tauri::command]
pub async fn open_codex_recovery_conversation(session_id: String) -> Result<(), String> {
    let id =
        uuid::Uuid::parse_str(&session_id).map_err(|_| "Invalid conversation ID".to_string())?;
    open_desktop_thread(id.to_string()).await
}

fn codex_binary() -> PathBuf {
    if let Some(binary) = std::env::var_os("CODEX_BIN") {
        return binary.into();
    }
    // Finder-launched apps do not inherit the interactive shell's PATH.
    #[cfg(target_os = "macos")]
    for bundle in ["/Applications/ChatGPT.app", "/Applications/Codex.app"] {
        for relative in [
            "Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex",
            "Contents/Resources/codex",
        ] {
            let binary = Path::new(bundle).join(relative);
            if binary.is_file() {
                return binary;
            }
        }
    }
    "codex".into()
}

async fn open_desktop_thread(id: String) -> Result<(), String> {
    let url = format!("codex://threads/{id}");
    #[cfg(target_os = "macos")]
    let mut command = tokio::process::Command::new("open");
    #[cfg(windows)]
    let mut command = tokio::process::Command::new("explorer.exe");
    #[cfg(not(any(target_os = "macos", windows)))]
    let mut command = tokio::process::Command::new("xdg-open");
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        command.arg(url).kill_on_drop(true).output(),
    )
    .await
    .map_err(|_| "Opening the conversation timed out".to_string())?
    .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err("Could not open the conversation in Codex desktop".into());
    }
    Ok(())
}

async fn resume_session<F, Fut>(
    session: &StoppedSession,
    home: &Path,
    binary: &Path,
    open_thread: F,
    wait_seconds: u64,
) -> Result<(), String>
where
    F: FnOnce(String) -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    // A timed-out delivery may still be queued. Keep a receipt across Switcher
    // restarts so retrying opens that chat instead of submitting a duplicate.
    let receipts = home.join("switcher-recovery");
    fs::create_dir_all(&receipts).map_err(|error| error.to_string())?;
    let receipt = receipts.join(format!("{}.offset", session.id));
    let previous = match fs::read_to_string(&receipt) {
        Ok(value) => Some(serde_json::from_str::<RecoveryReceipt>(&value)
            .map_err(|_| format!("Unreadable recovery receipt at {}. Inspect the chat's queue before removing this receipt to retry", receipt.display()))?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.to_string()),
    };
    let length = fs::metadata(&session.path)
        .map_err(|error| error.to_string())?
        .len();
    let pending = match previous {
        Some(previous) => {
            if previous.offset > length {
                return Err(
                    "Session log changed; inspect the conversation before retrying recovery".into(),
                );
            }
            if read_resume_result(&session.path, &previous)?.is_none() {
                Some(previous)
            } else {
                None
            }
        }
        None => None,
    };
    let needs_queue = pending.is_none();
    let mut delivery = pending.unwrap_or_else(|| RecoveryReceipt {
        offset: length,
        prompt: format!(
            "{CONTINUE_PROMPT}\n\n[Codex Switcher recovery: {}]",
            uuid::Uuid::new_v4()
        ),
        queued: false,
        queue_error: None,
    });
    if needs_queue {
        save_receipt(&receipt, &delivery)?;
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            tokio::process::Command::new(binary)
                .env("CODEX_HOME", home)
                .kill_on_drop(true)
                .args([
                    "queue",
                    "--thread",
                    &session.id,
                    "--message",
                    &delivery.prompt,
                ])
                .output(),
        )
        .await;
        match result {
            Ok(Ok(output)) if output.status.success() => {
                delivery.queued = true;
                save_receipt(&receipt, &delivery)?;
            }
            Ok(Err(error)) => {
                let _ = fs::remove_file(&receipt);
                return Err(format!("Could not run Codex CLI: {error}"));
            }
            // Keep only safe process diagnostics, not potentially sensitive CLI output.
            Ok(Ok(output)) => {
                delivery.queue_error = Some(format!("Codex CLI exited with {}", output.status));
                save_receipt(&receipt, &delivery)?;
            }
            Err(_) => {
                delivery.queue_error = Some("Codex CLI timed out after 10 seconds".into());
                save_receipt(&receipt, &delivery)?;
            }
        }
    }

    // Queue only persists a message. Loading the desktop chat starts its queue.
    open_thread(session.id.clone()).await?;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(wait_seconds);
    loop {
        if let Some(result) = read_resume_result(&session.path, &delivery)? {
            let _ = fs::remove_file(&receipt);
            return result;
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(if delivery.queued {
                "Continuation is queued, but its turn was not confirmed. Open this chat in Codex to inspect its queue; retrying will not submit another message".into()
            } else {
                format!("Queue delivery is unknown. {}. Open this chat and inspect its queue. If the recovery message is absent, remove {} before retrying; automatic resubmission is suppressed to avoid duplicates", delivery.queue_error.as_deref().unwrap_or("The previous attempt did not retain its CLI failure details"), receipt.display())
            });
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

fn save_receipt(path: &Path, receipt: &RecoveryReceipt) -> Result<(), String> {
    fs::write(
        path,
        serde_json::to_vec(receipt).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

fn read_resume_result(
    path: &Path,
    receipt: &RecoveryReceipt,
) -> Result<Option<Result<(), String>>, String> {
    let mut file = fs::File::open(path).map_err(|error| error.to_string())?;
    file.seek(SeekFrom::Start(receipt.offset))
        .map_err(|error| error.to_string())?;
    let mut started = false;
    let mut matched = false;
    let mut turn_id: Option<String> = None;
    for line in BufReader::new(file).lines() {
        let line = line.map_err(|error| error.to_string())?;
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let payload = &value["payload"];
        if started && value["type"] == "response_item" && payload["role"] == "user" {
            let message_turn =
                payload["internal_chat_message_metadata_passthrough"]["turn_id"].as_str();
            if message_turn.is_none() || message_turn == turn_id.as_deref() {
                matched |= payload["content"].as_array().is_some_and(|items| {
                    items
                        .iter()
                        .any(|item| item["text"].as_str() == Some(&receipt.prompt))
                });
            }
        }
        if value["type"] != "event_msg" {
            continue;
        }
        match payload["type"].as_str() {
            Some("task_started" | "turn_started") => {
                if matched {
                    return Ok(Some(Ok(())));
                }
                started = true;
                turn_id = payload["turn_id"].as_str().map(str::to_owned);
            }
            Some("user_message") if started => {
                matched |= payload["message"].as_str() == Some(&receipt.prompt)
            }
            Some("task_complete" | "turn_failed" | "turn_aborted") => {
                if matched {
                    return Ok(Some(
                        if payload["error"].is_null() && payload["type"] == "task_complete" {
                            Ok(())
                        } else {
                            Err("The recovery turn started but failed or was stopped; check the conversation in Codex".into())
                        },
                    ));
                }
                started = false;
            }
            Some("error") if matched => {
                return Ok(Some(Err(
                    "The recovery turn reported an error; check the conversation in Codex".into(),
                )))
            }
            _ => {}
        }
    }
    Ok(matched.then_some(Ok(())))
}

#[cfg(test)]
fn find_quota_stopped_sessions() -> Vec<StoppedSession> {
    find_recoverable_sessions(&std::collections::HashSet::new())
}

fn find_recoverable_sessions(open: &std::collections::HashSet<PathBuf>) -> Vec<StoppedSession> {
    let Ok(home) = crate::auth::get_codex_home() else {
        return Vec::new();
    };
    let root = home.join("sessions");
    let now = SystemTime::now();
    let mut sessions = Vec::new();
    collect_jsonl(&root, &now, &mut sessions, open);
    sessions.sort_by(|(left, _), (right, _)| right.cmp(left));
    sessions
        .into_iter()
        .take(MAX_SESSIONS)
        .map(|(_, id)| id)
        .collect()
}

fn collect_jsonl(
    root: &Path,
    now: &SystemTime,
    sessions: &mut Vec<(SystemTime, StoppedSession)>,
    open: &std::collections::HashSet<PathBuf>,
) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl(&path, now, sessions, open);
        } else if path.extension().and_then(|value| value.to_str()) == Some("jsonl")
            && is_recent(&path, now)
        {
            let running = open
                .contains(&path)
                .then(|| super::running_recovery::capture(&path))
                .flatten();
            if running.is_none() && !has_usage_limit_failure(&path) {
                continue;
            }
            if let (Some(modified), Some(id)) = (
                fs::metadata(&path)
                    .ok()
                    .and_then(|metadata| metadata.modified().ok()),
                session_id(&path),
            ) {
                sessions.push((modified, StoppedSession { id, path, running }));
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
    let payload = value.get("payload")?;
    // Internal agents have their own rollouts but are controlled by their parent chat.
    if payload.get("source").is_some_and(|source| {
        source.get("subagent").is_some() || source.as_str() == Some("subagent")
    }) || payload
        .get("agent_path")
        .and_then(Value::as_str)
        .is_some_and(|path| path != "/root" && !path.is_empty())
    {
        return None;
    }
    let id = payload.get("id")?.as_str()?;
    uuid::Uuid::parse_str(id).ok()?;
    Some(id.to_owned())
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
    fn recovery_excludes_internal_agents_but_keeps_user_forks() {
        let _home = crate::auth::test_support::TestHome::new();
        let root = crate::auth::get_codex_home().unwrap().join("sessions");
        std::fs::create_dir_all(&root).unwrap();
        for (n, extra, expected) in [
            (
                0,
                json!({"source":{"subagent":{"thread_spawn":{"parent_thread_id":"parent"}}}}),
                false,
            ),
            (1, json!({"source":"subagent"}), false),
            (
                2,
                json!({"source":"cli", "agent_path":"/root/verify_auto_switch"}),
                false,
            ),
            (3, json!({"source":"cli", "agent_path":"/root"}), true),
            (
                4,
                json!({"source":"cli", "forked_from_id":"parent", "parent_thread_id":"parent"}),
                true,
            ),
            (5, json!({}), true),
        ] {
            let id = uuid::Uuid::new_v4().to_string();
            let mut payload = extra;
            payload["id"] = json!(id);
            let path = root.join(format!("{n}.jsonl"));
            std::fs::write(
                &path,
                format!(
                    "{}\n{}\n",
                    json!({"type":"session_meta","payload":payload}),
                    quota_failure()
                ),
            )
            .unwrap();
            assert_eq!(super::session_id(&path).is_some(), expected);
        }
        assert_eq!(super::find_quota_stopped_sessions().len(), 3);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn failed_queue_preserves_exit_status_without_cli_output_on_retry() {
        let (_fixture, session, home, binary) = delivery_fixture();
        std::fs::write(&binary, "#!/bin/sh\necho private-token >&2\nexit 7\n").unwrap();
        for _ in 0..2 {
            let error = super::resume_session(&session, &home, &binary, |_| async { Ok(()) }, 0)
                .await
                .unwrap_err();
            assert!(error.contains("exit status: 7"));
            assert!(!error.contains("private-token"));
        }
        let legacy: super::RecoveryReceipt =
            serde_json::from_value(json!({"offset":0,"prompt":"Continue", "queued":false}))
                .unwrap();
        assert!(legacy.queue_error.is_none());
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

    #[cfg(unix)]
    fn delivery_fixture() -> (
        crate::auth::test_support::TestHome,
        super::StoppedSession,
        std::path::PathBuf,
        std::path::PathBuf,
    ) {
        use std::os::unix::fs::PermissionsExt;
        let fixture = crate::auth::test_support::TestHome::new();
        let home = crate::auth::get_codex_home().unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let path = home.join("rollout.jsonl");
        std::fs::write(
            &path,
            format!(
                "{}\n{}\n",
                json!({"type":"session_meta","payload":{"id":id}}),
                quota_failure()
            ),
        )
        .unwrap();
        let binary = home.join("fake-codex");
        std::fs::write(&binary, "#!/bin/sh\n[ \"$1\" = queue ] || exit 1\nprintf 'queued\\n' >> \"$CODEX_HOME/queue-calls\"\nprintf '%s' \"$5\" > \"$CODEX_HOME/queued-prompt\"\n").unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        (
            fixture,
            super::StoppedSession {
                id,
                path,
                running: None,
            },
            home,
            binary,
        )
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn queue_acceptance_is_not_success_and_retry_does_not_duplicate() {
        let (_fixture, session, home, binary) = delivery_fixture();
        let result = super::resume_session(&session, &home, &binary, |_| async { Ok(()) }, 0).await;
        assert!(result.unwrap_err().contains("turn was not confirmed"));
        let path = session.path.clone();
        let prompt_path = home.join("queued-prompt");
        super::resume_session(
            &session,
            &home,
            &binary,
            move |_| async move {
                use std::io::Write;
                writeln!(
                    std::fs::OpenOptions::new().append(true).open(path).unwrap(),
                    "{}\n{}",
                    event(json!({"type":"task_started"})),
                    event(json!({"type":"user_message", "message":std::fs::read_to_string(prompt_path).unwrap()}))
                )
                .unwrap();
                Ok(())
            },
            0,
        )
        .await
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(home.join("queue-calls")).unwrap(),
            "queued\n"
        );
        assert!(!home
            .join("switcher-recovery")
            .join(format!("{}.offset", session.id))
            .exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn waits_for_desktop_start_and_surfaces_immediate_failure() {
        let (_fixture, session, home, binary) = delivery_fixture();
        let path = session.path.clone();
        let prompt_path = home.join("queued-prompt");
        let result = super::resume_session(
            &session,
            &home,
            &binary,
            move |_| async move {
                tokio::spawn(async move {
                    use std::io::Write;
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
                    writeln!(
                        file,
                        "{}\n{}\n{}",
                        event(json!({"type":"task_started"})),
                        event(json!({"type":"user_message", "message":std::fs::read_to_string(prompt_path).unwrap()})),
                        quota_failure()
                    )
                    .unwrap();
                });
                Ok(())
            },
            2,
        )
        .await;
        assert!(result.unwrap_err().contains("failed or was stopped"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn failed_opener_retains_receipt_and_old_started_event_is_ignored() {
        let (_fixture, session, home, binary) = delivery_fixture();
        std::fs::write(
            &session.path,
            format!(
                "{}\n{}\n",
                event(json!({"type":"task_started"})),
                quota_failure()
            ),
        )
        .unwrap();
        let result = super::resume_session(
            &session,
            &home,
            &binary,
            |_| async { Err("No URL handler".into()) },
            0,
        )
        .await;
        assert_eq!(result.unwrap_err(), "No URL handler");
        let result = super::resume_session(&session, &home, &binary, |_| async { Ok(()) }, 0).await;
        assert!(result.unwrap_err().contains("turn was not confirmed"));
        assert_eq!(
            std::fs::read_to_string(home.join("queue-calls")).unwrap(),
            "queued\n"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cli_failure_does_not_open_chat_or_leave_receipt() {
        let (_fixture, session, home, binary) = delivery_fixture();
        std::fs::remove_file(&binary).unwrap();
        let result = super::resume_session(
            &session,
            &home,
            &binary,
            |_| async { panic!("must not open") },
            0,
        )
        .await;
        assert!(result.unwrap_err().contains("Could not run Codex CLI"));
        assert!(!home
            .join("switcher-recovery")
            .join(format!("{}.offset", session.id))
            .exists());
    }

    #[cfg(unix)]
    #[test]
    fn unrelated_turns_and_quoted_markers_do_not_confirm_recovery() {
        let (_fixture, session, _home, _binary) = delivery_fixture();
        let receipt = super::RecoveryReceipt {
            offset: 0,
            prompt: "unique recovery message".into(),
            queued: true,
            queue_error: None,
        };
        for records in [
            vec![event(json!({"type":"task_started","turn_id":"other"})), event(json!({"type":"user_message","message":"Please list the workspace files instead."}))],
            vec![event(json!({"type":"task_started","turn_id":"other"})), json!({"type":"response_item","payload":{"role":"assistant","content":[{"text":receipt.prompt}]}}).to_string()],
            vec![event(json!({"type":"task_started","turn_id":"other"})), json!({"type":"response_item","payload":{"role":"user","content":[{"text":receipt.prompt}],"internal_chat_message_metadata_passthrough":{"turn_id":"old"}}}).to_string()],
        ] {
            std::fs::write(&session.path, records.join("\n")).unwrap();
            assert!(super::read_resume_result(&session.path, &receipt).unwrap().is_none());
        }
        std::fs::write(&session.path, format!("{}\n{}", event(json!({"type":"task_started","turn_id":"ours"})), json!({"type":"response_item","payload":{"role":"user","content":[{"type":"input_text","text":receipt.prompt}],"internal_chat_message_metadata_passthrough":{"turn_id":"ours"}}}))).unwrap();
        assert!(super::read_resume_result(&session.path, &receipt)
            .unwrap()
            .unwrap()
            .is_ok());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_before_insertion_is_unknown_and_retry_does_not_duplicate() {
        let (_fixture, session, home, binary) = delivery_fixture();
        std::fs::write(
            &binary,
            "#!/bin/sh\nprintf 'attempt\\n' >> \"$CODEX_HOME/attempts\"\nexec sleep 30\n",
        )
        .unwrap();
        let error = super::resume_session(&session, &home, &binary, |_| async { Ok(()) }, 0)
            .await
            .unwrap_err();
        assert!(error.contains("delivery is unknown"));
        assert!(!error.contains("is queued"));
        assert!(error.contains("inspect its queue"));
        assert!(error.contains("timed out after 10 seconds"));
        let error = super::resume_session(&session, &home, &binary, |_| async { Ok(()) }, 0)
            .await
            .unwrap_err();
        assert!(error.contains("delivery is unknown"));
        assert!(error.contains("timed out after 10 seconds"));
        assert_eq!(
            std::fs::read_to_string(home.join("attempts")).unwrap(),
            "attempt\n"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn nonzero_exit_can_still_have_queued_and_consumed_the_message() {
        let (_fixture, session, home, binary) = delivery_fixture();
        let script = std::fs::read_to_string(&binary).unwrap();
        std::fs::write(&binary, format!("{script}exit 1\n")).unwrap();
        let path = session.path.clone();
        let prompt_path = home.join("queued-prompt");
        super::resume_session(&session, &home, &binary, move |_| async move {
            use std::io::Write;
            let mut file=std::fs::OpenOptions::new().append(true).open(path).unwrap();
            writeln!(file, "{}\n{}", event(json!({"type":"task_started"})), event(json!({"type":"user_message","message":std::fs::read_to_string(prompt_path).unwrap()}))).unwrap();
            Ok(())
        }, 0).await.unwrap();
    }

    #[tokio::test]
    async fn snapshot_filters_old_invalid_and_completed_logs_and_cancel_is_one_shot() {
        use std::io::Write;
        let _fixture = crate::auth::test_support::TestHome::new();
        let root = crate::auth::get_codex_home()
            .unwrap()
            .join("sessions/2026/10/18");
        std::fs::create_dir_all(&root).unwrap();
        let now = std::time::SystemTime::now();
        for n in 0..35 {
            let path = root.join(format!("rollout-{n}.jsonl"));
            let id = uuid::Uuid::new_v4().to_string();
            std::fs::write(
                &path,
                format!(
                    "{}\n{}\n",
                    json!({"type":"session_meta","payload":{"id":id}}),
                    quota_failure()
                ),
            )
            .unwrap();
            std::fs::File::open(path)
                .unwrap()
                .set_times(
                    std::fs::FileTimes::new().set_modified(now - std::time::Duration::from_secs(n)),
                )
                .unwrap();
        }
        for (name, id, tail, age) in [
            (
                "old",
                uuid::Uuid::new_v4().to_string(),
                quota_failure(),
                25 * 3600,
            ),
            ("invalid", "../not-a-session".into(), quota_failure(), 0),
            (
                "completed",
                uuid::Uuid::new_v4().to_string(),
                event(json!({"type":"task_complete"})),
                0,
            ),
        ] {
            let path = root.join(format!("{name}.jsonl"));
            std::fs::write(
                &path,
                format!(
                    "{}\n{}\n",
                    json!({"type":"session_meta","payload":{"id":id}}),
                    tail
                ),
            )
            .unwrap();
            std::fs::File::open(path)
                .unwrap()
                .set_times(
                    std::fs::FileTimes::new()
                        .set_modified(now - std::time::Duration::from_secs(age)),
                )
                .unwrap();
        }
        let found = super::find_quota_stopped_sessions();
        assert_eq!(found.len(), 32);
        assert_eq!(found[0].path.file_name().unwrap(), "rollout-0.jsonl");
        assert_eq!(found[31].path.file_name().unwrap(), "rollout-31.jsonl");
        assert_eq!(super::prepare_sessions(&Default::default()).unwrap(), 32);
        super::cancel_codex_session_recovery().unwrap();
        let report = super::resume_pending_sessions(|_| {}).await.unwrap();
        assert!(report.resumed.is_empty() && report.failed.is_empty() && report.skipped.is_empty());
        assert_eq!(super::prepare_sessions(&Default::default()).unwrap(), 32);
        for session in found {
            writeln!(
                std::fs::OpenOptions::new()
                    .append(true)
                    .open(session.path)
                    .unwrap(),
                "{}",
                event(json!({"type":"task_started"}))
            )
            .unwrap();
        }
        let report = super::resume_pending_sessions(|_| {}).await.unwrap();
        assert_eq!(report.skipped.len(), 32);
        assert!(report.resumed.is_empty() && report.failed.is_empty());
        assert!(super::resume_pending_sessions(|_| {})
            .await
            .unwrap()
            .skipped
            .is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn recovers_quota_and_owned_running_chats_together() {
        use std::io::Write;
        let (_fixture, _, home, binary) = delivery_fixture();
        let root = home.join("sessions");
        std::fs::create_dir_all(&root).unwrap();
        let mut open = std::collections::HashSet::new();
        let mut expected = Vec::new();
        for n in 0..8 {
            let id = uuid::Uuid::new_v4().to_string();
            let path = root.join(format!("{id}.jsonl"));
            let tail = match n {
                0 => quota_failure(),
                4 => event(json!({"type":"task_complete"})),
                5 => format!(
                    "{}\n{}",
                    event(json!({"type":"task_started"})),
                    json!({"type":"response_item","payload":{"type":"function_call","name":"request_user_input","call_id":"question"}})
                ),
                _ => event(json!({"type":"task_started"})),
            };
            std::fs::write(&path, format!("{}\n{tail}\n", json!({"type":"session_meta","payload":{"id":id,"agent_path":if n == 7 {"/root/child"} else {"/root"}}}))).unwrap();
            if n != 6 {
                open.insert(path);
            }
            if n < 4 {
                expected.push(id);
            }
        }
        let sessions = super::find_recoverable_sessions(&open);
        assert_eq!(
            sessions.len(),
            4,
            "quota plus three running chats, excluding idle, waiting, stale and child chats"
        );
        let report = super::recover_sessions(
            sessions,
            &home,
            &binary,
            |id| {
                let path = root.join(format!("{id}.jsonl"));
                let prompt = std::fs::read_to_string(home.join("queued-prompt")).unwrap();
                async move {
                    let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
                    writeln!(
                        file,
                        "{}\n{}",
                        event(json!({"type":"task_started"})),
                        event(json!({"type":"user_message","message":prompt}))
                    )
                    .unwrap();
                    Ok(())
                }
            },
            |_| {},
            std::time::Duration::from_secs(5),
        )
        .await;
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        assert!(report.skipped.is_empty());
        expected.sort();
        let mut resumed = report.resumed;
        resumed.sort();
        assert_eq!(resumed, expected);
        assert_eq!(
            std::fs::read_to_string(home.join("queue-calls"))
                .unwrap()
                .lines()
                .count(),
            4
        );
        // A second snapshot cannot enqueue continuations for old unfinished logs.
        assert!(super::find_recoverable_sessions(&Default::default()).is_empty());
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    #[ignore = "Read-only inspection of the running desktop; no queue or restart"]
    async fn live_running_snapshot() {
        let open = super::super::running_recovery::open_rollouts()
            .await
            .unwrap();
        let root = dirs::home_dir().unwrap().join(".codex/sessions");
        let mut sessions = Vec::new();
        super::collect_jsonl(&root, &std::time::SystemTime::now(), &mut sessions, &open);
        let running = sessions
            .iter()
            .filter(|(_, session)| session.running.is_some())
            .count();
        println!(
            "Open rollout files: {}; recoverable running chats: {running}",
            open.len()
        );
        assert!(running > 0, "Run while a desktop chat is working");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn batch_deadline_reports_every_session_and_retains_pending_receipt() {
        use std::cell::RefCell;
        use std::io::Write;
        let (_fixture, first, home, binary) = delivery_fixture();
        let second = super::StoppedSession {
            id: uuid::Uuid::new_v4().to_string(),
            path: home.join("second.jsonl"),
            running: None,
        };
        let third = super::StoppedSession {
            id: uuid::Uuid::new_v4().to_string(),
            path: home.join("third.jsonl"),
            running: None,
        };
        std::fs::copy(&first.path, &second.path).unwrap();
        std::fs::copy(&first.path, &third.path).unwrap();
        let progress = RefCell::new(Vec::new());
        let report = super::recover_sessions(
            vec![first.clone(), second.clone(), third.clone()],
            &home,
            &binary,
            |id| {
                if id == first.id {
                    let prompt = std::fs::read_to_string(home.join("queued-prompt")).unwrap();
                    let mut file = std::fs::OpenOptions::new()
                        .append(true)
                        .open(&first.path)
                        .unwrap();
                    writeln!(
                        file,
                        "{}\n{}",
                        event(json!({"type":"task_started"})),
                        event(json!({"type":"user_message","message":prompt}))
                    )
                    .unwrap();
                }
                async { Ok(()) }
            },
            |update| progress.borrow_mut().push(update),
            std::time::Duration::from_secs(3),
        )
        .await;
        assert!(report.timed_out);
        assert_eq!(report.resumed, vec![first.id]);
        assert_eq!(report.failed.len(), 2);
        assert_eq!(report.failed[0].session_id, second.id);
        assert!(report.failed[0].reason.contains("may still be queued"));
        assert_eq!(report.failed[1].session_id, third.id);
        assert!(report.failed[1].reason.starts_with("Not attempted"));
        assert_eq!(progress.borrow().last().unwrap().completed, 3);
        assert!(progress.borrow().iter().any(|p| p.completed == 1
            && p.total == 3
            && p.current_session_id == Some(second.id.clone())));
        assert!(home
            .join("switcher-recovery")
            .join(format!("{}.offset", second.id))
            .exists());
        assert!(!home
            .join("switcher-recovery")
            .join(format!("{}.offset", third.id))
            .exists());
        assert_eq!(
            std::fs::read_to_string(home.join("queue-calls")).unwrap(),
            "queued\nqueued\n"
        );
        // The interrupted attempt can finish on retry without queueing again.
        super::resume_session(
            &second,
            &home,
            &binary,
            |_| async {
                let prompt = std::fs::read_to_string(home.join("queued-prompt")).unwrap();
                let mut file = std::fs::OpenOptions::new()
                    .append(true)
                    .open(&second.path)
                    .unwrap();
                writeln!(
                    file,
                    "{}\n{}",
                    event(json!({"type":"task_started"})),
                    event(json!({"type":"user_message","message":prompt}))
                )
                .unwrap();
                Ok(())
            },
            0,
        )
        .await
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(home.join("queue-calls")).unwrap(),
            "queued\nqueued\n"
        );
    }

    #[tokio::test]
    async fn open_conversation_rejects_invalid_ids() {
        assert_eq!(
            super::open_codex_recovery_conversation("../not-a-chat?command=run".into())
                .await
                .unwrap_err(),
            "Invalid conversation ID"
        );
    }

    // Explicit opt-in: sends one continuation to the named local test session
    // using the real CLI and OS URL handler. Never run against an arbitrary chat.
    #[tokio::test]
    #[ignore = "requires a running Codex desktop and an explicitly selected test session"]
    async fn live_desktop_recovery() {
        let path = std::path::PathBuf::from(
            std::env::var("CODEX_RECOVERY_E2E_ROLLOUT").expect("test rollout path"),
        );
        let home =
            std::path::PathBuf::from(std::env::var("CODEX_RECOVERY_E2E_HOME").expect("Codex home"));
        let id = super::session_id(&path).expect("valid session UUID");
        assert!(
            super::has_usage_limit_failure(&path),
            "test session must be quota stopped"
        );
        super::resume_session(
            &super::StoppedSession {
                id,
                path,
                running: None,
            },
            &home,
            &super::codex_binary(),
            super::open_desktop_thread,
            60,
        )
        .await
        .unwrap();
    }
}
