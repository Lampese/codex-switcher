//! Process detection commands

use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[derive(Debug, Clone)]
pub struct RunningCodexProcess {
    pub pid: u32,
    pub command: String,
    pub is_background: bool,
}

/// Information about running Codex processes
#[derive(Debug, Clone, serde::Serialize)]
pub struct CodexProcessInfo {
    /// Number of running foreground codex processes
    pub count: usize,
    /// Number of background IDE/extension codex processes (like Antigravity)
    pub background_count: usize,
    /// Whether switching is allowed without a restart prompt
    pub can_switch: bool,
    /// Process IDs of running foreground codex processes
    pub pids: Vec<u32>,
}

/// Check for running Codex processes
#[tauri::command]
pub async fn check_codex_processes() -> Result<CodexProcessInfo, String> {
    let processes = collect_running_codex_processes().map_err(|e| e.to_string())?;
    let pids: Vec<u32> = processes
        .iter()
        .filter(|process| !process.is_background)
        .map(|process| process.pid)
        .collect();
    let background_count = processes
        .iter()
        .filter(|process| process.is_background)
        .count();

    Ok(CodexProcessInfo {
        count: pids.len(),
        background_count,
        can_switch: pids.is_empty() && background_count == 0,
        pids,
    })
}

pub fn collect_running_codex_processes() -> anyhow::Result<Vec<RunningCodexProcess>> {
    #[cfg(unix)]
    {
        collect_running_codex_processes_unix()
    }

    #[cfg(windows)]
    {
        collect_running_codex_processes_windows()
    }
}

pub fn gracefully_stop_codex_processes(processes: &[RunningCodexProcess]) -> anyhow::Result<()> {
    if processes.is_empty() {
        return Ok(());
    }

    #[cfg(unix)]
    {
        for process in processes {
            let _ = Command::new("kill")
                .args(["-TERM", &process.pid.to_string()])
                .output();
        }

        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            let any_running = processes.iter().any(|process| {
                Command::new("kill")
                    .args(["-0", &process.pid.to_string()])
                    .status()
                    .map(|status| status.success())
                    .unwrap_or(false)
            });

            if !any_running {
                return Ok(());
            }

            thread::sleep(Duration::from_millis(100));
        }
    }

    #[cfg(windows)]
    {
        for process in processes {
            let _ = Command::new("taskkill")
                .creation_flags(CREATE_NO_WINDOW)
                .args(["/PID", &process.pid.to_string()])
                .output();
        }
        thread::sleep(Duration::from_secs(2));
        return Ok(());
    }

    anyhow::bail!("Timed out waiting for Codex processes to close gracefully");
}

pub fn restart_codex_processes(processes: &[RunningCodexProcess]) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        for process in processes {
            if process.command.trim().is_empty() {
                continue;
            }

            Command::new("sh")
                .arg("-c")
                .arg("nohup sh -lc \"$1\" >/dev/null 2>&1 &")
                .arg("sh")
                .arg(&process.command)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?;
        }
    }

    #[cfg(windows)]
    {
        for process in processes {
            if process.command.trim().is_empty() {
                continue;
            }

            Command::new("cmd")
                .creation_flags(CREATE_NO_WINDOW)
                .args(["/C", "start", "", "/B", &process.command])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?;
        }
    }

    Ok(())
}

#[cfg(unix)]
fn collect_running_codex_processes_unix() -> anyhow::Result<Vec<RunningCodexProcess>> {
    let mut processes = Vec::new();
    let output = Command::new("ps").args(["-eo", "pid=,command="]).output()?;
    let stdout = String::from_utf8_lossy(&output.stdout);

    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some((pid_str, command)) = line.split_once(' ') {
            let command = command.trim();
            let executable = command.split_whitespace().next().unwrap_or("");
            let is_codex = executable == "codex" || executable.ends_with("/codex");
            let is_background = command.contains(".antigravity")
                || command.contains("openai.chatgpt")
                || command.contains(".vscode");
            let is_switcher =
                command.contains("codex-switcher") || command.contains("Codex Switcher");

            if is_codex && !is_switcher {
                if let Ok(pid) = pid_str.trim().parse::<u32>() {
                    if pid != std::process::id()
                        && !processes.iter().any(|p: &RunningCodexProcess| p.pid == pid)
                    {
                        processes.push(RunningCodexProcess {
                            pid,
                            command: command.to_string(),
                            is_background,
                        });
                    }
                }
            }
        }
    }

    Ok(processes)
}

#[cfg(windows)]
fn collect_running_codex_processes_windows() -> anyhow::Result<Vec<RunningCodexProcess>> {
    let mut processes = Vec::new();
    let output = Command::new("tasklist")
        .creation_flags(CREATE_NO_WINDOW)
        .args(["/FI", "IMAGENAME eq codex.exe", "/FO", "CSV", "/NH"])
        .output()?;
    let stdout = String::from_utf8_lossy(&output.stdout);

    for line in stdout.lines() {
        let parts: Vec<&str> = line.split(',').collect();
        if parts.len() > 1 {
            let name = parts[0].trim_matches('"').to_lowercase();
            if name == "codex.exe" {
                let pid_str = parts[1].trim_matches('"');
                if let Ok(pid) = pid_str.parse::<u32>() {
                    if pid != std::process::id() {
                        processes.push(RunningCodexProcess {
                            pid,
                            command: String::from("codex"),
                            is_background: false,
                        });
                    }
                }
            }
        }
    }

    Ok(processes)
}
