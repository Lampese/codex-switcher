//! Reopen only desktop apps observed immediately before a force close.
//! Launch targets stay in the backend; the frontend receives a one-use token.

use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
enum DesktopTarget {
    #[cfg(any(target_os = "macos", test))]
    MacBundle(String),
    #[cfg(any(windows, test))]
    WindowsExecutable(String),
    #[cfg(any(windows, test))]
    WindowsAppId(String),
}

pub(super) struct CapturedDesktop {
    pid: u32,
    target: DesktopTarget,
}

pub(super) fn desktop_pids(desktops: &[CapturedDesktop]) -> Vec<u32> {
    desktops.iter().map(|desktop| desktop.pid).collect()
}

struct ReopenTicket {
    token: String,
    created_at: Instant,
    targets: Vec<DesktopTarget>,
}

static PENDING_REOPEN: Mutex<Option<ReopenTicket>> = Mutex::new(None);

#[derive(serde::Serialize)]
pub struct CodexReopenInfo {
    supported: bool,
    desktop_count: usize,
}

#[tauri::command]
pub async fn get_codex_reopen_info() -> Result<CodexReopenInfo, String> {
    tokio::task::spawn_blocking(|| {
        let (pids, _) = super::find_codex_processes().map_err(|e| e.to_string())?;
        let desktops = capture_desktops(&pids)?;
        Ok(CodexReopenInfo {
            supported: cfg!(any(target_os = "macos", windows)),
            desktop_count: desktops.len(),
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

pub(super) fn capture_desktops(pids: &[u32]) -> Result<Vec<CapturedDesktop>, String> {
    let mut desktops = Vec::new();

    #[cfg(target_os = "macos")]
    {
        let names = super::read_unix_process_names();
        for &pid in pids {
            let output = super::Command::new("ps")
                .args(["-p", &pid.to_string(), "-o", "command="])
                .output()
                .map_err(|e| e.to_string())?;
            if !output.status.success() {
                continue;
            }
            let command = String::from_utf8_lossy(&output.stdout);
            let name = names.get(&pid).map(String::as_str);
            if let Some(bundle) = mac_bundle_path(command.trim(), name) {
                if is_codex_bundle(&bundle) {
                    desktops.push(CapturedDesktop {
                        pid,
                        target: DesktopTarget::MacBundle(bundle),
                    });
                }
            }
        }
    }

    #[cfg(windows)]
    {
        for process in super::read_windows_codex_processes().map_err(|e| e.to_string())? {
            if !pids.contains(&process.process_id) {
                continue;
            }
            if let Some(path) = windows_desktop_executable(&process) {
                let target = if path.to_ascii_lowercase().contains("\\windowsapps\\") {
                    // Packaged apps require their exact registered identity, not
                    // whichever Codex happens to be first in the Start menu.
                    windows_app_id(&path).map(DesktopTarget::WindowsAppId)
                } else {
                    let executable = std::path::Path::new(&path);
                    let has_desktop_resources = executable.parent().is_some_and(|parent| {
                        parent.join("resources/app.asar").is_file()
                            || parent.join("resources/app").is_dir()
                    });
                    (executable.is_file() && has_desktop_resources)
                        .then_some(DesktopTarget::WindowsExecutable(path))
                };
                if let Some(target) = target {
                    desktops.push(CapturedDesktop {
                        pid: process.process_id,
                        target,
                    });
                }
            }
        }
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    let _ = (pids, &mut desktops);
    Ok(desktops)
}

#[cfg(any(target_os = "macos", test))]
fn mac_bundle_path(command: &str, name: Option<&str>) -> Option<String> {
    let suffix = match name? {
        "ChatGPT" => "/ChatGPT.app/Contents/MacOS/ChatGPT",
        "Codex" => "/Codex.app/Contents/MacOS/Codex",
        _ => return None,
    };
    if !command.starts_with('/') {
        return None;
    }
    let index = command.find(suffix)?;
    if command[index + suffix.len()..]
        .chars()
        .next()
        .is_some_and(|c| !c.is_whitespace())
    {
        return None;
    }
    Some(command[..index + suffix.find("/Contents/")?].to_string())
}

#[cfg(target_os = "macos")]
fn is_codex_bundle(bundle: &str) -> bool {
    plist::Value::from_file(std::path::Path::new(bundle).join("Contents/Info.plist"))
        .ok()
        .and_then(|value| {
            value
                .as_dictionary()?
                .get("CFBundleIdentifier")?
                .as_string()
                .map(str::to_owned)
        })
        .is_some_and(|id| id == "com.openai.codex")
}

#[cfg(any(windows, test))]
fn windows_desktop_executable(process: &super::WindowsCodexProcess) -> Option<String> {
    if !super::is_windows_codex_root_process(process)
        || super::is_ide_plugin_process(&process.command_line.to_ascii_lowercase())
    {
        return None;
    }
    let path = if process.executable_path.trim().is_empty() {
        super::windows_command_executable_path(&process.command_line)?
    } else {
        process.executable_path.trim()
    };
    let path = path.replace('/', "\\");
    let bytes = path.as_bytes();
    // Only an absolute local executable path; never a CLI name or arguments.
    if bytes.len() < 3 || !bytes[0].is_ascii_alphabetic() || &bytes[1..3] != b":\\" {
        return None;
    }
    Some(path)
}

#[cfg(windows)]
fn windows_app_id(executable: &str) -> Option<String> {
    use std::os::windows::process::CommandExt;
    // Resolve the manifest application whose executable matches this process.
    // Passing the path via the environment avoids interpreting it as PowerShell.
    let script = r#"
$ErrorActionPreference = 'Stop'
$exe = $env:CODEX_SWITCHER_REOPEN_EXE
foreach ($pkg in (Get-AppxPackage -Name 'OpenAI.Codex*')) {
  if (-not $pkg.InstallLocation) { continue }
  foreach ($app in (Get-AppxPackageManifest -Package $pkg.PackageFullName).Package.Applications.Application) {
    if (-not $app.Executable -or -not $app.Id) { continue }
    $candidate = Join-Path $pkg.InstallLocation $app.Executable
    if ([string]::Equals($candidate, $exe, [StringComparison]::OrdinalIgnoreCase)) {
      Write-Output ($pkg.PackageFamilyName + '!' + $app.Id)
      exit 0
    }
  }
}
exit 1
"#;
    let output = super::Command::new("powershell.exe")
        .creation_flags(super::CREATE_NO_WINDOW)
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env("CODEX_SWITCHER_REOPEN_EXE", executable)
        .output()
        .ok()?;
    let id = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (output.status.success() && valid_windows_app_id(&id)).then_some(id)
}

#[cfg(any(windows, test))]
fn valid_windows_app_id(id: &str) -> bool {
    id.to_ascii_lowercase().starts_with("openai.codex")
        && id.contains("_2p2nqsd0c76g0!")
        && id.split('!').count() == 2
        && !id.ends_with('!')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-!".contains(c))
}

fn closed_targets(desktops: Vec<CapturedDesktop>, killed_pids: &[u32]) -> Vec<DesktopTarget> {
    let mut targets = Vec::new();
    for desktop in desktops {
        if killed_pids.contains(&desktop.pid) && !targets.contains(&desktop.target) {
            targets.push(desktop.target);
        }
    }
    targets
}

pub(super) fn remember_closed_desktops(
    desktops: Vec<CapturedDesktop>,
    killed_pids: &[u32],
) -> Option<String> {
    let targets = closed_targets(desktops, killed_pids);
    let mut pending = PENDING_REOPEN.lock().ok()?;
    *pending = None;
    if targets.is_empty() {
        return None;
    }
    let token = uuid::Uuid::new_v4().to_string();
    *pending = Some(ReopenTicket {
        token: token.clone(),
        created_at: Instant::now(),
        targets,
    });
    Some(token)
}

fn take_targets(
    pending: &mut Option<ReopenTicket>,
    token: &str,
) -> Result<Vec<DesktopTarget>, String> {
    let valid = pending.as_ref().is_some_and(|ticket| {
        ticket.token == token && ticket.created_at.elapsed() < Duration::from_secs(120)
    });
    if !valid {
        return Err("The desktop reopen request expired or is no longer available".into());
    }
    Ok(pending.take().expect("validated ticket").targets)
}

#[tauri::command]
pub async fn reopen_closed_codex_desktop(token: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        let targets = take_targets(
            &mut *PENDING_REOPEN.lock().map_err(|e| e.to_string())?,
            &token,
        )?;
        super::ensure_codex_not_running()?;
        let expected = targets.clone();
        for target in &targets {
            if !launch_desktop(target) {
                return Err("Could not launch Codex desktop. The account change was not undone. Open Codex manually.".into());
            }
        }
        let confirmed = wait_for_desktops(
            &expected,
            Duration::from_secs(20),
            || {
                let (pids, _) = super::find_codex_processes().map_err(|e| e.to_string())?;
                Ok(capture_desktops(&pids)?.into_iter().map(|desktop| desktop.target).collect())
            },
        );
        if !confirmed {
            return Err("Codex desktop did not appear within 20 seconds. The account change was not undone. Check the app or open it manually.".into());
        }
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

fn wait_for_desktops(
    expected: &[DesktopTarget],
    timeout: Duration,
    mut inspect: impl FnMut() -> Result<Vec<DesktopTarget>, String>,
) -> bool {
    let started = Instant::now();
    loop {
        if let Ok(running) = inspect() {
            if expected.iter().all(|target| running.contains(target)) {
                return true;
            }
        }
        if started.elapsed() >= timeout {
            return false;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn launch_desktop(target: &DesktopTarget) -> bool {
    match target {
        #[cfg(target_os = "macos")]
        DesktopTarget::MacBundle(bundle) => {
            is_codex_bundle(bundle)
                && super::command_succeeds(super::Command::new("open").arg("-a").arg(bundle))
        }
        #[cfg(windows)]
        DesktopTarget::WindowsExecutable(path) => {
            super::spawn_windows_codex_exe(std::path::Path::new(path))
        }
        #[cfg(windows)]
        DesktopTarget::WindowsAppId(id) => {
            use std::os::windows::process::CommandExt;
            valid_windows_app_id(id) && super::command_succeeds(
                super::Command::new("powershell.exe")
                    .creation_flags(super::CREATE_NO_WINDOW)
                    .args(["-NoProfile", "-NonInteractive", "-Command",
                        "$ErrorActionPreference = 'Stop'; Start-Process ('shell:AppsFolder\\' + $env:CODEX_SWITCHER_REOPEN_APP_ID)"])
                    .env("CODEX_SWITCHER_REOPEN_APP_ID", id),
            )
        }
        #[allow(unreachable_patterns)]
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_confirmation_requires_the_exact_desktop_and_handles_detection_failure() {
        let expected = vec![DesktopTarget::MacBundle("/Applications/ChatGPT.app".into())];
        assert!(wait_for_desktops(&expected, Duration::ZERO, || Ok(
            expected.clone()
        )));
        assert!(!wait_for_desktops(&expected, Duration::ZERO, || Ok(vec![])));
        assert!(!wait_for_desktops(&expected, Duration::ZERO, || Err(
            "query failed".into()
        )));
        assert!(!wait_for_desktops(&expected, Duration::ZERO, || Ok(vec![
            DesktopTarget::MacBundle("/Other/ChatGPT.app".into())
        ])));
    }

    #[test]
    fn extracts_exact_macos_bundle_with_spaces_and_rejects_helpers() {
        assert_eq!(
            mac_bundle_path(
                "/Users/test/My Apps/ChatGPT.app/Contents/MacOS/ChatGPT --flag",
                Some("ChatGPT")
            ),
            Some("/Users/test/My Apps/ChatGPT.app".into()),
        );
        assert_eq!(
            mac_bundle_path(
                "/Applications/Codex.app/Contents/MacOS/Codex",
                Some("Codex")
            ),
            Some("/Applications/Codex.app".into())
        );
        assert_eq!(
            mac_bundle_path(
                "/Applications/ChatGPT.app/Contents/MacOS/ChatGPTHelper",
                Some("ChatGPT")
            ),
            None
        );
        assert_eq!(
            mac_bundle_path(
                "/Applications/ChatGPT.app/Contents/Resources/codex app-server",
                Some("codex")
            ),
            None
        );
        assert_eq!(mac_bundle_path("codex", Some("codex")), None);
    }

    fn windows_process(path: &str, command: &str) -> super::super::WindowsCodexProcess {
        super::super::WindowsCodexProcess {
            name: "ChatGPT.exe".into(),
            process_id: 100,
            parent_process_id: 1,
            executable_path: path.into(),
            command_line: command.into(),
            main_window_title: "Codex".into(),
        }
    }

    #[test]
    fn windows_package_requires_codex_identity_and_preserves_exact_path() {
        let path =
            r"C:\Program Files\WindowsApps\OpenAI.Codex_1.2.3.0_x64__2p2nqsd0c76g0\app\ChatGPT.exe";
        assert_eq!(
            windows_desktop_executable(&windows_process(path, "")),
            Some(path.into())
        );
        assert_eq!(
            windows_desktop_executable(&windows_process("", &format!("\"{path}\" --flag"))),
            Some(path.into())
        );
        assert_eq!(
            windows_desktop_executable(&windows_process(path, "--type=renderer")),
            None
        );
        assert_eq!(
            windows_desktop_executable(&windows_process(r"C:\Other\ChatGPT.exe", "")),
            None
        );
        assert_eq!(
            windows_desktop_executable(&windows_process(
                &path.replace("2p2nqsd0c76g0", "other"),
                ""
            )),
            None
        );
        let mut legacy = windows_process(r"C:\Users\test\Apps\Codex\Codex.exe", "");
        legacy.name = "Codex.exe".into();
        assert_eq!(
            windows_desktop_executable(&legacy),
            Some(legacy.executable_path.clone())
        );
        legacy.executable_path = r"C:\Apps\Codex\resources\codex.exe".into();
        assert_eq!(windows_desktop_executable(&legacy), None);
        legacy.executable_path = "codex.exe".into();
        assert_eq!(windows_desktop_executable(&legacy), None);
    }

    #[test]
    fn windows_app_id_rejects_other_apps_and_malformed_identifiers() {
        assert!(valid_windows_app_id("OpenAI.Codex_2p2nqsd0c76g0!App"));
        for id in [
            "OpenAI.ChatGPT_2p2nqsd0c76g0!App",
            "OpenAI.Codex_other!App",
            "OpenAI.Codex_2p2nqsd0c76g0!",
            "OpenAI.Codex_2p2nqsd0c76g0!App\n",
            "OpenAI.Codex_2p2nqsd0c76g0!App!Other",
        ] {
            assert!(!valid_windows_app_id(id));
        }
    }

    #[test]
    fn reopens_only_closed_roots_and_deduplicates_same_installation() {
        let target = DesktopTarget::MacBundle("/Applications/ChatGPT.app".into());
        let desktops = vec![
            CapturedDesktop {
                pid: 1,
                target: target.clone(),
            },
            CapturedDesktop {
                pid: 2,
                target: target.clone(),
            },
            CapturedDesktop {
                pid: 3,
                target: DesktopTarget::MacBundle("/Other/Codex.app".into()),
            },
        ];
        assert_eq!(closed_targets(desktops, &[1, 2]), vec![target]);
        assert!(closed_targets(vec![], &[1]).is_empty());
    }

    #[test]
    fn reopen_tickets_are_one_use_and_expire() {
        let mut ticket = Some(ReopenTicket {
            token: "token".into(),
            created_at: Instant::now(),
            targets: vec![DesktopTarget::WindowsAppId(
                "OpenAI.Codex_2p2nqsd0c76g0!App".into(),
            )],
        });
        assert!(take_targets(&mut ticket, "wrong").is_err());
        assert_eq!(take_targets(&mut ticket, "token").unwrap().len(), 1);
        assert!(take_targets(&mut ticket, "token").is_err());
        let mut expired = Some(ReopenTicket {
            token: "expired".into(),
            created_at: Instant::now() - Duration::from_secs(121),
            targets: vec![DesktopTarget::WindowsExecutable("C:\\Codex.exe".into())],
        });
        assert!(take_targets(&mut expired, "expired").is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "Read-only inspection of live desktop processes; run explicitly"]
    fn inspect_live_desktop_targets() {
        let (pids, _) = super::super::find_codex_processes().unwrap();
        let desktops = capture_desktops(&pids).unwrap();
        for desktop in &desktops {
            println!("Desktop PID {}: {:?}", desktop.pid, desktop.target);
        }
        assert!(!desktops.is_empty(), "No running Codex desktop found");
    }
}
