//! Process detection commands

use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

/// The kind of Codex process detected.
#[derive(Debug, Clone, PartialEq)]
pub enum CodexProcessKind {
    /// The Codex desktop GUI (Electron app at /Applications/Codex.app/Contents/MacOS/Codex)
    DesktopApp,
    /// A standalone `codex` CLI process (not inside an app bundle, not an IDE extension)
    Cli,
    /// A background IDE/extension process (Antigravity, VSCode, ChatGPT extension) — never stopped
    Background,
}

#[derive(Debug, Clone)]
pub struct RunningCodexProcess {
    pub pid: u32,
    pub command: String,
    pub kind: CodexProcessKind,
}

impl RunningCodexProcess {
    pub fn is_background(&self) -> bool {
        self.kind == CodexProcessKind::Background
    }
}

/// Information about running Codex processes
#[derive(Debug, Clone, serde::Serialize)]
pub struct CodexProcessInfo {
    /// Number of running foreground codex processes (desktop app + CLI)
    pub count: usize,
    /// Number of background IDE/extension codex processes (Antigravity, VSCode, etc.)
    pub background_count: usize,
    /// Whether switching is allowed without a restart prompt (no foreground processes)
    pub can_switch: bool,
    /// Process IDs of running foreground codex processes
    pub pids: Vec<u32>,
}

/// Check for running Codex processes
#[tauri::command]
pub async fn check_codex_processes() -> Result<CodexProcessInfo, String> {
    let processes = collect_running_codex_processes().map_err(|e| e.to_string())?;
    let foreground: Vec<_> = processes
        .iter()
        .filter(|p| !p.is_background())
        .collect();
    let pids: Vec<u32> = foreground.iter().map(|p| p.pid).collect();
    let background_count = processes.iter().filter(|p| p.is_background()).count();

    Ok(CodexProcessInfo {
        count: pids.len(),
        background_count,
        can_switch: pids.is_empty(),
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
    let foreground: Vec<_> = processes.iter().filter(|p| !p.is_background()).collect();

    if foreground.is_empty() {
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    {
        let has_desktop = foreground
            .iter()
            .any(|p| p.kind == CodexProcessKind::DesktopApp);

        if has_desktop {
            // The Codex Electron app intercepts AppleScript quit (returns "User canceled"
            // error -128) and ignores SIGTERM, so we must SIGKILL the GUI process directly.
            if let Some(desktop) = foreground.iter().find(|p| p.kind == CodexProcessKind::DesktopApp) {
                let _ = Command::new("kill")
                    .args(["-9", &desktop.pid.to_string()])
                    .output();
            }

            // Wait up to 5 seconds for the Codex GUI process to exit.
            // Match only /MacOS/Codex — not codex app-server orphans.
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(5) {
                let still_running = Command::new("pgrep")
                    .args(["-a", "Codex"])
                    .output()
                    .map(|o| {
                        String::from_utf8_lossy(&o.stdout)
                            .lines()
                            .any(|l| l.contains("MacOS/Codex"))
                    })
                    .unwrap_or(false);

                if !still_running {
                    break;
                }
                thread::sleep(Duration::from_millis(200));
            }
        }

        // SIGTERM any remaining CLI (non-desktop, non-background) processes
        let cli_processes: Vec<_> = foreground
            .iter()
            .filter(|p| p.kind == CodexProcessKind::Cli)
            .collect();

        for process in &cli_processes {
            let _ = Command::new("kill")
                .args(["-TERM", &process.pid.to_string()])
                .output();
        }

        if !cli_processes.is_empty() {
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(3) {
                let any_running = cli_processes.iter().any(|process| {
                    Command::new("kill")
                        .args(["-0", &process.pid.to_string()])
                        .status()
                        .map(|status| status.success())
                        .unwrap_or(false)
                });

                if !any_running {
                    break;
                }
                thread::sleep(Duration::from_millis(100));
            }
        }

        return Ok(());
    }

    #[cfg(target_os = "linux")]
    {
        for process in &foreground {
            let _ = Command::new("kill")
                .args(["-TERM", &process.pid.to_string()])
                .output();
        }

        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            let any_running = foreground.iter().any(|process| {
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
        let has_desktop = foreground
            .iter()
            .any(|p| p.kind == CodexProcessKind::DesktopApp);

        if has_desktop {
            let _ = Command::new("taskkill")
                .creation_flags(CREATE_NO_WINDOW)
                .args(["/IM", "Codex.exe", "/F"])
                .output();

            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(5) {
                let still_running = Command::new("tasklist")
                    .creation_flags(CREATE_NO_WINDOW)
                    .args(["/FI", "IMAGENAME eq Codex.exe", "/NH"])
                    .output()
                    .map(|o| {
                        String::from_utf8_lossy(&o.stdout)
                            .to_lowercase()
                            .contains("codex.exe")
                    })
                    .unwrap_or(false);

                if !still_running {
                    break;
                }
                thread::sleep(Duration::from_millis(200));
            }
        }

        // Kill remaining CLI processes
        for process in foreground.iter().filter(|p| p.kind == CodexProcessKind::Cli) {
            let _ = Command::new("taskkill")
                .creation_flags(CREATE_NO_WINDOW)
                .args(["/PID", &process.pid.to_string()])
                .output();
        }

        thread::sleep(Duration::from_secs(1));
        return Ok(());
    }

    Ok(())
}

pub fn restart_codex_processes(processes: &[RunningCodexProcess]) -> anyhow::Result<()> {
    let has_desktop = processes
        .iter()
        .any(|p| p.kind == CodexProcessKind::DesktopApp);

    #[cfg(target_os = "macos")]
    {
        if has_desktop {
            // Wait until the Codex GUI process is fully gone before reopening,
            // so the new app instance reads the freshly written auth.json.
            // Match only /MacOS/Codex — not codex app-server orphans.
            let deadline = Instant::now() + Duration::from_secs(8);
            loop {
                let still_alive = Command::new("pgrep")
                    .args(["-a", "Codex"])
                    .output()
                    .map(|o| {
                        String::from_utf8_lossy(&o.stdout)
                            .lines()
                            .any(|l| l.contains("MacOS/Codex"))
                    })
                    .unwrap_or(false);

                if !still_alive || Instant::now() >= deadline {
                    break;
                }
                thread::sleep(Duration::from_millis(200));
            }

            Command::new("open")
                .args(["-a", "Codex"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?;
        }

        // Restart any CLI processes
        for process in processes.iter().filter(|p| p.kind == CodexProcessKind::Cli) {
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

    #[cfg(target_os = "linux")]
    {
        for process in processes.iter().filter(|p| !p.is_background()) {
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
        if has_desktop {
            let deadline = Instant::now() + Duration::from_secs(8);
            loop {
                let still_alive = Command::new("tasklist")
                    .creation_flags(CREATE_NO_WINDOW)
                    .args(["/FI", "IMAGENAME eq Codex.exe", "/NH"])
                    .output()
                    .map(|o| {
                        String::from_utf8_lossy(&o.stdout)
                            .to_lowercase()
                            .contains("codex.exe")
                    })
                    .unwrap_or(false);

                if !still_alive || Instant::now() >= deadline {
                    break;
                }
                thread::sleep(Duration::from_millis(200));
            }

            Command::new("cmd")
                .creation_flags(CREATE_NO_WINDOW)
                .args(["/C", "start", "", "Codex"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?;
        }

        // Restart any CLI processes
        for process in processes.iter().filter(|p| p.kind == CodexProcessKind::Cli) {
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
    let mut seen_desktop = false;

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

            // Skip ourselves
            let is_switcher =
                command.contains("codex-switcher") || command.contains("Codex Switcher");
            if is_switcher {
                continue;
            }

            // Detect the Codex desktop app GUI process (Electron, capital C)
            // e.g. /Applications/Codex.app/Contents/MacOS/Codex
            let is_desktop_gui = (executable.ends_with("/Codex") || executable == "Codex")
                && command.contains("Codex.app")
                && !command.contains("Helper")
                && !command.contains("crashpad");

            // Detect background IDE/extension codex processes — never stop these
            let is_background = (executable == "codex" || executable.ends_with("/codex"))
                && (command.contains(".antigravity")
                    || command.contains("openai.chatgpt")
                    || command.contains(".vscode"));

            // Detect standalone CLI codex (not desktop, not background)
            // Explicitly exclude app-server processes (they are managed by the app bundle)
            let is_cli = (executable == "codex" || executable.ends_with("/codex"))
                && !command.contains("Codex.app")
                && !is_background;

            let kind = if is_desktop_gui {
                // Deduplicate: record the desktop app only once via the GUI process.
                // We intentionally ignore codex app-server processes — they are
                // managed by the app bundle and quitting via osascript handles them.
                // Tracking orphaned servers would cause the "wait for exit" loop to
                // stall indefinitely on stale PIDs.
                if seen_desktop {
                    continue;
                }
                seen_desktop = true;
                CodexProcessKind::DesktopApp
            } else if is_background {
                CodexProcessKind::Background
            } else if is_cli {
                CodexProcessKind::Cli
            } else {
                continue;
            };

            if let Ok(pid) = pid_str.trim().parse::<u32>() {
                if pid != std::process::id()
                    && !processes.iter().any(|p: &RunningCodexProcess| p.pid == pid)
                {
                    processes.push(RunningCodexProcess {
                        pid,
                        command: command.to_string(),
                        kind,
                    });
                }
            }
        }
    }

    Ok(processes)
}

#[cfg(windows)]
fn collect_running_codex_processes_windows() -> anyhow::Result<Vec<RunningCodexProcess>> {
    let mut processes = Vec::new();

    // Check for the desktop app (Codex.exe with capital C)
    let output = Command::new("tasklist")
        .creation_flags(CREATE_NO_WINDOW)
        .args(["/FI", "IMAGENAME eq Codex.exe", "/FO", "CSV", "/NH"])
        .output()?;
    let stdout = String::from_utf8_lossy(&output.stdout);

    for line in stdout.lines() {
        let parts: Vec<&str> = line.split(',').collect();
        if parts.len() > 1 {
            let name = parts[0].trim_matches('"');
            if name == "Codex.exe" {
                let pid_str = parts[1].trim_matches('"');
                if let Ok(pid) = pid_str.parse::<u32>() {
                    if pid != std::process::id() {
                        processes.push(RunningCodexProcess {
                            pid,
                            command: String::from("Codex.exe"),
                            kind: CodexProcessKind::DesktopApp,
                        });
                        break; // Only need one desktop app entry
                    }
                }
            }
        }
    }

    // Check for CLI codex processes (lowercase codex.exe)
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
                            kind: CodexProcessKind::Cli,
                        });
                    }
                }
            }
        }
    }

    Ok(processes)
}
