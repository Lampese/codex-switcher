//! Process detection commands

use std::process::Command;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

/// Information about running Codex processes
#[derive(Debug, Clone, serde::Serialize)]
pub struct CodexProcessInfo {
    /// Number of running codex processes
    pub count: usize,
    /// Number of background IDE/extension codex processes (like Antigravity)
    pub background_count: usize,
    /// Whether switching is allowed (no processes running)
    pub can_switch: bool,
    /// Process IDs of running codex processes
    pub pids: Vec<u32>,
}

/// Check for running Codex processes
#[tauri::command]
pub async fn check_codex_processes() -> Result<CodexProcessInfo, String> {
    let (pids, bg_count) = find_codex_processes().map_err(|e| e.to_string())?;
    let count = pids.len();

    Ok(CodexProcessInfo {
        count,
        background_count: bg_count,
        can_switch: count == 0,
        pids,
    })
}

/// Find all running codex processes. Returns (active_pids, background_count)
fn find_codex_processes() -> anyhow::Result<(Vec<u32>, usize)> {
    let mut pids = Vec::new();
    let mut bg_count = 0;

    #[cfg(unix)]
    {
        // Use ps with custom format to get the pid and full command line
        let output = Command::new("ps").args(["-eo", "pid,command"]).output();

        if let Ok(output) = output {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines().skip(1) {
                // Skip header
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                
                // The first part is PID, the rest is the command string
                if let Some((pid_str, command)) = line.split_once(' ') {
                    let command = command.trim();
                    
                    // Get the executable path/name (first word of the command string before args)
                    let executable = command.split_whitespace().next().unwrap_or("");
                    
                    // Check if the executable is exactly "codex" or ends with "/codex"
                    let is_codex = executable == "codex" || executable.ends_with("/codex");
                    
                    // Exclude if it's running from an extension or IDE integration (like Antigravity)
                    // These are expected background processes we shouldn't block on
                    let is_ide_plugin = command.contains(".antigravity") || command.contains("openai.chatgpt") || command.contains(".vscode");
                    
                    // Skip our own app
                    let is_switcher =
                        command.contains("codex-switcher") || command.contains("Codex Switcher");

                    if is_codex && !is_switcher {
                        if let Ok(pid) = pid_str.trim().parse::<u32>() {
                            if pid != std::process::id() && !pids.contains(&pid) {
                                if is_ide_plugin {
                                    bg_count += 1;
                                } else {
                                    pids.push(pid);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[cfg(windows)]
    {
        // Use tasklist on Windows - match exact "codex.exe"
        let output = Command::new("tasklist")
            // Prevent a console window from flashing when this command is invoked from the GUI app.
            .creation_flags(CREATE_NO_WINDOW)
            .args(["/FI", "IMAGENAME eq codex.exe", "/FO", "CSV", "/NH"])
            .output();

        if let Ok(output) = output {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                // CSV format: "name","pid",...
                let parts: Vec<&str> = line.split(',').collect();
                if parts.len() > 1 {
                    let name = parts[0].trim_matches('"').to_lowercase();
                    // Only match exact "codex.exe", not "codex-switcher.exe"
                    if name == "codex.exe" {
                        let pid_str = parts[1].trim_matches('"');
                        if let Ok(pid) = pid_str.parse::<u32>() {
                            if pid != std::process::id() {
                                // For Windows, we don't have an easy way to check if it's an IDE plugin
                                // just from the tasklist output, so assume they're regular for now
                                pids.push(pid);
                            }
                        }
                    }
                }
            }
        }
    }

    Ok((pids, bg_count))
}
