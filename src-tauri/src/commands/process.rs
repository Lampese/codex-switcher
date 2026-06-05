//! Process detection commands

use std::process::Command;

#[cfg(windows)]
use anyhow::Context;

#[cfg(unix)]
use std::collections::{HashMap, HashSet};

#[cfg(windows)]
use std::collections::HashSet;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[cfg(windows)]
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
struct WindowsCodexProcess {
    name: String,
    process_id: u32,
    parent_process_id: u32,
    #[serde(default)]
    command_line: String,
    #[serde(default)]
    main_window_title: String,
}

/// Information about running Codex processes
#[derive(Debug, Clone, serde::Serialize)]
pub struct CodexProcessInfo {
    /// Number of active Codex app instances
    pub count: usize,
    /// Number of ignored background/stale Codex-related processes
    pub background_count: usize,
    /// Whether switching is allowed (no active Codex app instances)
    pub can_switch: bool,
    /// Process IDs of active Codex app instances
    pub pids: Vec<u32>,
}

/// Summary of a force-close operation for active Codex processes.
#[derive(Debug, Clone, serde::Serialize)]
pub struct KillCodexProcessesResult {
    /// Number of active Codex sessions targeted before expanding child processes.
    pub targeted_count: usize,
    /// Process IDs that were successfully signalled for termination.
    pub killed_pids: Vec<u32>,
    /// Process IDs that could not be terminated.
    pub failed_pids: Vec<u32>,
}

#[cfg(unix)]
struct UnixProcessSnapshot {
    children_by_parent: HashMap<u32, Vec<u32>>,
    uid_by_pid: HashMap<u32, u32>,
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

pub(crate) fn ensure_codex_not_running() -> Result<(), String> {
    let (pids, _) = find_codex_processes().map_err(|e| e.to_string())?;

    if pids.is_empty() {
        return Ok(());
    }

    Err(format!(
        "Cannot switch accounts while {} Codex process{} running",
        pids.len(),
        if pids.len() == 1 { " is" } else { "es are" }
    ))
}

/// Force-close active Codex processes that currently block account switching.
#[tauri::command]
pub async fn kill_codex_processes() -> Result<KillCodexProcessesResult, String> {
    tokio::task::spawn_blocking(kill_codex_processes_blocking)
        .await
        .map_err(|e| e.to_string())?
}

fn kill_codex_processes_blocking() -> Result<KillCodexProcessesResult, String> {
    let (pids, _) = find_codex_processes().map_err(|e| e.to_string())?;
    let targeted_count = pids.len();
    let mut killed_pids = Vec::new();
    let mut failed_pids = Vec::new();

    #[cfg(target_os = "macos")]
    let mut admin_targets: Vec<u32> = Vec::new();

    #[cfg(unix)]
    let snapshot = read_unix_process_snapshot();

    #[cfg(unix)]
    let targets = expand_process_targets(&pids, snapshot.as_ref());

    #[cfg(windows)]
    let targets = expand_process_targets(&pids);

    #[cfg(target_os = "macos")]
    let current_uid = current_unix_uid();

    for pid in targets {
        #[cfg(target_os = "macos")]
        if snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.uid_by_pid.get(&pid).copied())
            .zip(current_uid)
            .is_some_and(|(owner_uid, current_uid)| owner_uid != current_uid)
        {
            admin_targets.push(pid);
            continue;
        }

        if force_kill_process(pid) {
            killed_pids.push(pid);
        } else {
            failed_pids.push(pid);
        }
    }

    #[cfg(target_os = "macos")]
    {
        admin_targets.extend(failed_pids.iter().copied());
        admin_targets.sort_unstable();
        admin_targets.dedup();

        let mut still_failed = Vec::new();
        if force_kill_processes_with_admin_privileges(&admin_targets) {
            for pid in admin_targets.iter().copied() {
                if process_exists(pid) {
                    still_failed.push(pid);
                } else if !killed_pids.contains(&pid) {
                    killed_pids.push(pid);
                }
            }
        } else {
            still_failed.extend(
                admin_targets
                    .iter()
                    .copied()
                    .filter(|pid| process_exists(*pid)),
            );
        }
        failed_pids = still_failed;
    }

    Ok(KillCodexProcessesResult {
        targeted_count,
        killed_pids,
        failed_pids,
    })
}

#[cfg(unix)]
fn expand_process_targets(root_pids: &[u32], snapshot: Option<&UnixProcessSnapshot>) -> Vec<u32> {
    let mut targets = Vec::new();
    let mut visited = HashSet::new();

    if let Some(snapshot) = snapshot {
        for root_pid in root_pids {
            let mut stack = snapshot
                .children_by_parent
                .get(root_pid)
                .cloned()
                .unwrap_or_default();
            while let Some(pid) = stack.pop() {
                if !visited.insert(pid) {
                    continue;
                }
                targets.push(pid);

                if let Some(children) = snapshot.children_by_parent.get(&pid) {
                    stack.extend(children.iter().copied());
                }
            }
        }
    }

    for root_pid in root_pids {
        if visited.insert(*root_pid) {
            targets.push(*root_pid);
        }
    }

    targets
}

#[cfg(windows)]
fn expand_process_targets(root_pids: &[u32]) -> Vec<u32> {
    root_pids.to_vec()
}

#[cfg(unix)]
fn read_unix_process_snapshot() -> Option<UnixProcessSnapshot> {
    let output = Command::new("ps")
        .args(["-axo", "pid=,ppid=,uid="])
        .output()
        .ok()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut children_by_parent = HashMap::new();
    let mut uid_by_pid = HashMap::new();

    for line in stdout.lines() {
        let mut parts = line.split_whitespace();
        let Some(pid_str) = parts.next() else {
            continue;
        };
        let Some(ppid_str) = parts.next() else {
            continue;
        };
        let Some(uid_str) = parts.next() else {
            continue;
        };
        let (Ok(pid), Ok(ppid), Ok(uid)) = (
            pid_str.parse::<u32>(),
            ppid_str.parse::<u32>(),
            uid_str.parse::<u32>(),
        ) else {
            continue;
        };

        children_by_parent
            .entry(ppid)
            .or_insert_with(Vec::new)
            .push(pid);
        uid_by_pid.insert(pid, uid);
    }

    Some(UnixProcessSnapshot {
        children_by_parent,
        uid_by_pid,
    })
}

fn force_kill_process(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let killed = Command::new("/bin/kill")
            .arg("-9")
            .arg(pid.to_string())
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        return killed || !process_exists(pid);
    }

    #[cfg(windows)]
    {
        let killed = Command::new("taskkill")
            .creation_flags(CREATE_NO_WINDOW)
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        return killed || !process_exists(pid);
    }

    #[allow(unreachable_code)]
    false
}

#[cfg(target_os = "macos")]
fn force_kill_processes_with_admin_privileges(pids: &[u32]) -> bool {
    if pids.is_empty() {
        return true;
    }

    let pid_args = pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(" ");
    let script = format!(
        r#"do shell script "for pid in {pid_args}; do /bin/kill -9 \"$pid\" 2>/dev/null || true; done" with administrator privileges with prompt "Codex Switcher needs permission to force close sudo/root Codex processes.""#
    );

    Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(script)
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn current_unix_uid() -> Option<u32> {
    Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                String::from_utf8_lossy(&output.stdout)
                    .trim()
                    .parse::<u32>()
                    .ok()
            } else {
                None
            }
        })
}

fn process_exists(pid: u32) -> bool {
    #[cfg(unix)]
    {
        return Command::new("ps")
            .arg("-p")
            .arg(pid.to_string())
            .args(["-o", "pid="])
            .output()
            .map(|output| {
                output.status.success()
                    && String::from_utf8_lossy(&output.stdout)
                        .split_whitespace()
                        .any(|value| value == pid.to_string())
            })
            .unwrap_or(false);
    }

    #[cfg(windows)]
    {
        return Command::new("tasklist")
            .creation_flags(CREATE_NO_WINDOW)
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .output()
            .map(|output| String::from_utf8_lossy(&output.stdout).contains(&pid.to_string()))
            .unwrap_or(false);
    }

    #[allow(unreachable_code)]
    false
}

/// Find all running codex processes. Returns (active_pids, background_count)
fn find_codex_processes() -> anyhow::Result<(Vec<u32>, usize)> {
    #[cfg(unix)]
    {
        let mut pids = Vec::new();
        let mut bg_count = 0;
        let process_names = read_unix_process_names();

        // Include TTY so we can distinguish interactive CLI sessions from
        // background helper processes such as lingering app-server instances.
        let output = Command::new("ps")
            .args(["-axo", "pid=,tty=,command="])
            .output();

        if let Ok(output) = output {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }

                let mut parts = line.split_whitespace();
                let Some(pid_str) = parts.next() else {
                    continue;
                };
                let Some(tty) = parts.next() else {
                    continue;
                };
                let command = parts.collect::<Vec<_>>().join(" ");
                if command.is_empty() {
                    continue;
                }

                let Ok(pid) = pid_str.parse::<u32>() else {
                    continue;
                };

                let lowercase_command = command.to_ascii_lowercase();
                let is_switcher = lowercase_command.contains("codex-switcher");

                if is_switcher {
                    continue;
                }

                // macOS app bundle paths can contain spaces (`Codex Helper.app`), so
                // splitting on whitespace can turn helper processes into false
                // positives for the main `Codex` app. Detect by full command shape
                // instead of relying on the first token.
                let first_token = command.split_whitespace().next().unwrap_or("");
                let is_codex_cli = first_token == "codex" || first_token.ends_with("/codex");
                let is_codex_desktop = is_macos_codex_desktop_process(
                    &command,
                    process_names.get(&pid).map(String::as_str),
                );

                if !is_codex_cli && !is_codex_desktop {
                    continue;
                }

                if pid == std::process::id() || pids.contains(&pid) {
                    continue;
                }

                let is_ide_plugin = is_ide_plugin_process(&lowercase_command);
                let is_app_server = lowercase_command.contains("codex app-server");
                let has_tty = tty != "??" && tty != "?";

                if is_ide_plugin || is_app_server {
                    bg_count += 1;
                    continue;
                }

                if is_codex_desktop || has_tty {
                    pids.push(pid);
                } else {
                    // Headless or orphaned codex processes should not block switching.
                    bg_count += 1;
                }
            }
        }

        pids.sort_unstable();
        pids.dedup();

        return Ok((pids, bg_count));
    }

    #[cfg(windows)]
    {
        return find_windows_codex_processes();
    }

    #[allow(unreachable_code)]
    Ok((Vec::new(), 0))
}

#[cfg(unix)]
fn read_unix_process_names() -> HashMap<u32, String> {
    let Ok(output) = Command::new("ps").args(["-axo", "pid=,ucomm="]).output() else {
        return HashMap::new();
    };

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut parts = line.trim().splitn(2, char::is_whitespace);
            let pid = parts.next()?.parse::<u32>().ok()?;
            let name = parts.next()?.trim();
            (!name.is_empty()).then(|| (pid, name.to_string()))
        })
        .collect()
}

#[cfg(unix)]
fn is_macos_codex_desktop_process(command: &str, process_name: Option<&str>) -> bool {
    const EXECUTABLE_SUFFIX: &str = "/Codex.app/Contents/MacOS/Codex";

    process_name == Some("Codex")
        && command.find(EXECUTABLE_SUFFIX).is_some_and(|index| {
            command[index + EXECUTABLE_SUFFIX.len()..]
                .chars()
                .next()
                .is_none_or(char::is_whitespace)
        })
}

#[cfg(windows)]
fn find_windows_codex_processes() -> anyhow::Result<(Vec<u32>, usize)> {
    // tasklist counts every Electron helper (`--type=gpu-process`, crashpad, renderer, etc.),
    // which inflates the badge and incorrectly blocks switching. Use PowerShell so we can inspect
    // the command line and only count live top-level app instances.
    const POWERSHELL_SCRIPT: &str = r#"
$windowTitles = @{}
Get-Process -Name Codex -ErrorAction SilentlyContinue | ForEach-Object {
  $windowTitles[[uint32]$_.Id] = $_.MainWindowTitle
}

Get-CimInstance Win32_Process |
  Where-Object { $_.Name -ieq 'Codex.exe' -or $_.Name -ieq 'codex.exe' } |
  ForEach-Object {
    [PSCustomObject]@{
      Name = $_.Name
      ProcessId = [uint32]$_.ProcessId
      ParentProcessId = [uint32]$_.ParentProcessId
      CommandLine = if ($_.CommandLine) { $_.CommandLine } else { '' }
      MainWindowTitle = if ($windowTitles.ContainsKey([uint32]$_.ProcessId)) {
        [string]$windowTitles[[uint32]$_.ProcessId]
      } else {
        ''
      }
    }
  } |
  ConvertTo-Json -Compress
"#;

    let output = Command::new("powershell.exe")
        .creation_flags(CREATE_NO_WINDOW)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            POWERSHELL_SCRIPT,
        ])
        .output()
        .context("failed to query Windows process list")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("PowerShell process query failed: {}", stderr.trim());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let processes = parse_windows_codex_processes(&stdout)?;

    let mut active_pids = Vec::new();
    let mut ignored_count = 0;

    for process in processes
        .iter()
        .filter(|process| is_windows_codex_root_process(process))
    {
        let command = process.command_line.to_ascii_lowercase();
        if is_ide_plugin_process(&command) {
            ignored_count += 1;
            continue;
        }

        let has_window = !process.main_window_title.trim().is_empty();
        let has_renderer =
            windows_has_descendant_matching(process.process_id, &processes, |child| {
                child
                    .command_line
                    .to_ascii_lowercase()
                    .contains("--type=renderer")
            });
        let has_app_server =
            windows_has_descendant_matching(process.process_id, &processes, |child| {
                let command = child.command_line.to_ascii_lowercase();
                command.contains("resources\\codex.exe") && command.contains("app-server")
            });

        if has_window || has_renderer || has_app_server {
            active_pids.push(process.process_id);
        } else {
            // Ignore stale helper trees left behind after the window has already closed.
            ignored_count += 1;
        }
    }

    active_pids.sort_unstable();
    active_pids.dedup();

    Ok((active_pids, ignored_count))
}

#[cfg(windows)]
fn parse_windows_codex_processes(stdout: &str) -> anyhow::Result<Vec<WindowsCodexProcess>> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    let value: serde_json::Value =
        serde_json::from_str(trimmed).context("failed to parse Windows process JSON")?;

    match value {
        serde_json::Value::Array(values) => values
            .into_iter()
            .map(|value| {
                serde_json::from_value(value)
                    .context("failed to deserialize Windows Codex process entry")
            })
            .collect(),
        value => Ok(vec![serde_json::from_value(value)
            .context("failed to deserialize Windows Codex process entry")?]),
    }
}

#[cfg(windows)]
fn is_windows_codex_root_process(process: &WindowsCodexProcess) -> bool {
    let name = process.name.to_ascii_lowercase();
    let command = process.command_line.to_ascii_lowercase();

    name == "codex.exe"
        && !command.contains("codex-switcher")
        && !command.contains("--type=")
        && !command.contains("resources\\codex.exe")
}

#[cfg(any(unix, windows))]
fn is_ide_plugin_process(command: &str) -> bool {
    command.contains(".antigravity")
        || command.contains("openai.chatgpt")
        || command.contains(".vscode")
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::is_macos_codex_desktop_process;

    #[cfg(unix)]
    #[test]
    fn detects_only_the_macos_codex_desktop_root_process() {
        assert!(is_macos_codex_desktop_process(
            "/Applications/Codex.app/Contents/MacOS/Codex",
            Some("Codex")
        ));
        assert!(is_macos_codex_desktop_process(
            "/Users/test/Applications With Spaces/Codex.app/Contents/MacOS/Codex --flag",
            Some("Codex")
        ));
        assert!(!is_macos_codex_desktop_process(
            "/Applications/Codex.app/Contents/Frameworks/Codex Framework.framework/Helpers/Codex (Service).app/Contents/MacOS/Codex (Service) --type=gpu-process",
            Some("Codex (Service)")
        ));
        assert!(!is_macos_codex_desktop_process(
            "/Applications/Codex.app/Contents/Frameworks/Codex Framework.framework/Helpers/Codex (Renderer).app/Contents/MacOS/Codex (Renderer) --type=renderer",
            Some("Codex (Renderer)")
        ));
        assert!(!is_macos_codex_desktop_process(
            "/Applications/Codex.app/Contents/Resources/codex app-server",
            Some("codex")
        ));
        assert!(!is_macos_codex_desktop_process(
            "/Applications/Codex.app/Contents/Frameworks/Codex Framework.framework/Helpers/Codex (Renderer).app/Contents/MacOS/Codex (Renderer) --app-executable /Applications/Codex.app/Contents/MacOS/Codex --type=renderer",
            Some("Codex (Renderer)")
        ));
    }
}

#[cfg(windows)]
fn windows_has_descendant_matching<F>(
    root_pid: u32,
    processes: &[WindowsCodexProcess],
    mut predicate: F,
) -> bool
where
    F: FnMut(&WindowsCodexProcess) -> bool,
{
    let mut queue = vec![root_pid];
    let mut visited = HashSet::new();

    while let Some(parent_pid) = queue.pop() {
        for process in processes
            .iter()
            .filter(|process| process.parent_process_id == parent_pid)
        {
            if !visited.insert(process.process_id) {
                continue;
            }

            if predicate(process) {
                return true;
            }

            queue.push(process.process_id);
        }
    }

    false
}

/// Open the Codex desktop app if it is installed.
#[tauri::command]
pub async fn open_codex_app() -> Result<(), String> {
    tokio::task::spawn_blocking(open_codex_app_blocking)
        .await
        .map_err(|e| e.to_string())?
}

fn open_codex_app_blocking() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        if command_succeeds(Command::new("open").args(["-b", "com.openai.codex"])) {
            return Ok(());
        }

        if command_succeeds(Command::new("open").args(["-a", "Codex"])) {
            return Ok(());
        }

        return Err("Codex app is not installed or could not be opened".to_string());
    }

    #[cfg(windows)]
    {
        if open_windows_registered_app() {
            return Ok(());
        }

        if let Some(path) = find_windows_codex_app() {
            if spawn_windows_codex_exe(&path) {
                return Ok(());
            }
        }

        for shortcut in find_windows_codex_shortcuts() {
            if open_windows_shortcut(&shortcut) {
                return Ok(());
            }
        }

        return Err("Codex app is not installed or could not be opened".to_string());
    }

    #[allow(unreachable_code)]
    Err("Opening Codex app is only supported on macOS and Windows".to_string())
}

fn command_succeeds(command: &mut Command) -> bool {
    command
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(windows)]
fn find_windows_codex_app() -> Option<std::path::PathBuf> {
    let mut candidates = Vec::new();

    for key in ["LOCALAPPDATA", "ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(base) = std::env::var_os(key) {
            let base = std::path::PathBuf::from(base);
            candidates.push(base.join("Programs").join("Codex").join("Codex.exe"));
            candidates.push(base.join("Programs").join("codex").join("Codex.exe"));
            candidates.push(base.join("Codex").join("Codex.exe"));
            candidates.push(base.join("OpenAI").join("Codex").join("Codex.exe"));
            candidates.push(
                base.join("OpenAI")
                    .join("Codex")
                    .join("bin")
                    .join("codex.exe"),
            );
            candidates.push(base.join("OpenAI Codex").join("Codex.exe"));
            candidates.push(base.join("Codex Desktop").join("Codex.exe"));
        }
    }

    candidates.extend(find_windows_codex_apps_in_programs());
    candidates.extend(find_windows_codex_apps_in_package_cache());

    candidates
        .into_iter()
        .find(|path| path.is_file() && looks_like_windows_desktop_app(path))
}

#[cfg(windows)]
fn looks_like_windows_desktop_app(path: &std::path::Path) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };

    if is_windows_openai_codex_bin(path) {
        return true;
    }

    parent.join("resources").join("app.asar").is_file()
        || parent.join("resources").join("app").is_dir()
        || parent.join("resources").is_dir()
}

#[cfg(windows)]
fn is_windows_openai_codex_bin(path: &std::path::Path) -> bool {
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };

    if !file_name.eq_ignore_ascii_case("codex.exe") {
        return false;
    }

    let normalized = path
        .to_string_lossy()
        .replace('/', "\\")
        .to_ascii_lowercase();
    normalized.contains("\\openai\\codex\\bin\\codex.exe")
}

#[cfg(windows)]
fn spawn_windows_codex_exe(path: &std::path::Path) -> bool {
    let mut command = Command::new(path);
    command.creation_flags(CREATE_NO_WINDOW);
    if let Some(parent) = path.parent() {
        command.current_dir(parent);
    }
    command.spawn().is_ok()
}

#[cfg(windows)]
fn open_windows_registered_app() -> bool {
    let script = r#"
$app = Get-StartApps |
  Where-Object { $_.Name -like '*Codex*' -or $_.AppID -like '*Codex*' } |
  Select-Object -First 1
if ($null -eq $app) { exit 1 }
Start-Process ("shell:AppsFolder\" + $app.AppID)
"#;

    let mut command = Command::new("powershell.exe");
    command.creation_flags(CREATE_NO_WINDOW);
    command.args(["-NoProfile", "-NonInteractive", "-Command", script]);
    command_succeeds(&mut command)
}

#[cfg(windows)]
fn find_windows_codex_shortcuts() -> Vec<std::path::PathBuf> {
    let mut candidates = Vec::new();

    for key in ["APPDATA", "ProgramData"] {
        if let Some(base) = std::env::var_os(key) {
            let programs = std::path::PathBuf::from(base)
                .join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs");
            candidates.push(programs.join("Codex.lnk"));
            candidates.push(programs.join("OpenAI").join("Codex.lnk"));
            collect_windows_codex_shortcuts(&programs, &mut candidates, 0);
        }
    }

    candidates
        .into_iter()
        .filter(|path| path.is_file())
        .collect()
}

#[cfg(windows)]
fn open_windows_shortcut(path: &std::path::Path) -> bool {
    let mut command = Command::new("cmd.exe");
    command.creation_flags(CREATE_NO_WINDOW);
    command.arg("/C").arg("start").arg("").arg(path);
    command_succeeds(&mut command)
}

#[cfg(windows)]
fn find_windows_codex_apps_in_programs() -> Vec<std::path::PathBuf> {
    let mut candidates = Vec::new();

    let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") else {
        return candidates;
    };

    let programs = std::path::PathBuf::from(local_app_data).join("Programs");
    collect_windows_codex_apps(&programs, &mut candidates, 0);
    candidates
}

#[cfg(windows)]
fn find_windows_codex_apps_in_package_cache() -> Vec<std::path::PathBuf> {
    let mut candidates = Vec::new();

    let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") else {
        return candidates;
    };

    let packages = std::path::PathBuf::from(local_app_data).join("Packages");
    let Ok(entries) = std::fs::read_dir(packages) else {
        return candidates;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Some(dir_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };

        if !dir_name.to_ascii_lowercase().starts_with("openai.codex_") {
            continue;
        }

        candidates.push(
            path.join("LocalCache")
                .join("Local")
                .join("OpenAI")
                .join("Codex")
                .join("bin")
                .join("codex.exe"),
        );
    }

    candidates
}

#[cfg(windows)]
fn collect_windows_codex_apps(
    dir: &std::path::Path,
    candidates: &mut Vec<std::path::PathBuf>,
    depth: usize,
) {
    if depth > 2 {
        return;
    }

    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_windows_codex_apps(&path, candidates, depth + 1);
            continue;
        }

        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };

        if file_name.eq_ignore_ascii_case("Codex.exe") {
            candidates.push(path);
        }
    }
}

#[cfg(windows)]
fn collect_windows_codex_shortcuts(
    dir: &std::path::Path,
    candidates: &mut Vec<std::path::PathBuf>,
    depth: usize,
) {
    if depth > 3 {
        return;
    }

    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_windows_codex_shortcuts(&path, candidates, depth + 1);
            continue;
        }

        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };

        if file_name.to_ascii_lowercase().contains("codex")
            && path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("lnk"))
        {
            candidates.push(path);
        }
    }
}
