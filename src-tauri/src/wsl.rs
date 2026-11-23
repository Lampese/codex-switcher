//! WSL Codex support for the Windows build (#55).
//!
//! When the user enables "Use WSL Codex", `get_codex_home()` resolves to the
//! `\\wsl.localhost` UNC path of the distro's `~/.codex` directory and process
//! checks run inside WSL instead of against Windows `tasklist`.
//! All functionality is Windows-only; elsewhere every helper is a no-op.

#[cfg(not(test))]
use std::path::PathBuf;

#[cfg(windows)]
use std::process::Command;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

fn wsl_enabled() -> bool {
    crate::auth::load_app_settings()
        .map(|settings| settings.use_wsl_codex)
        .unwrap_or(false)
}

fn configured_distro() -> Option<String> {
    crate::auth::load_app_settings()
        .ok()
        .and_then(|settings| settings.wsl_distro)
        .filter(|distro| !distro.trim().is_empty())
}

/// Codex home inside WSL as a `\\wsl.localhost` UNC path, when the toggle is
/// on and the distro's home can be resolved. `None` on non-Windows builds or
/// when the feature is off — callers keep their regular `~/.codex`.
#[cfg(not(test))]
pub fn codex_home_override() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        if !wsl_enabled() {
            return None;
        }
        let distro = configured_distro()
            .or_else(|| list_distros().into_iter().next())?;
        let home = wsl_home(Some(&distro))?;
        let unc = home.trim().replace('/', "\\");
        if unc.is_empty() {
            return None;
        }
        return Some(PathBuf::from(format!(
            "\\\\wsl.localhost\\{distro}\\{unc}\\.codex"
        )));
    }
    #[cfg(not(windows))]
    None
}

/// List installed WSL distro names for the settings dropdown.
pub fn list_distros() -> Vec<String> {
    #[cfg(windows)]
    {
        let output = Command::new("wsl")
            .args(["-l", "-q"])
            .creation_flags(CREATE_NO_WINDOW)
            .output();
        let Ok(output) = output else {
            return Vec::new();
        };
        // `wsl -l` prints UTF-16LE (possibly with a BOM) on Windows.
        let bytes = &output.stdout[..];
        let utf16: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        let text = String::from_utf16_lossy(&utf16);
        return text
            .lines()
            .map(|line| line.trim().trim_matches('\u{feff}').to_string())
            .filter(|line| !line.is_empty())
            .collect();
    }
    #[cfg(not(windows))]
    Vec::new()
}

/// `$HOME` inside the given (or default) distro.
#[cfg(windows)]
fn wsl_home(distro: Option<&str>) -> Option<String> {
    let mut cmd = Command::new("wsl");
    if let Some(distro) = distro {
        cmd.args(["-d", distro]);
    }
    let output = cmd
        .args(["-e", "sh", "-c", "echo $HOME"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Codex process pids inside WSL (`pgrep -f codex`), for
/// `find_codex_processes` on Windows when the toggle is on. Returns
/// `None` when WSL is off/unavailable so the Windows path is used.
#[cfg(windows)]
pub fn codex_processes() -> Option<Vec<u32>> {
    if !wsl_enabled() {
        return None;
    }
    let mut cmd = Command::new("wsl");
    if let Some(distro) = configured_distro() {
        cmd.args(["-d", &distro]);
    }
    let output = cmd
        .args(["-e", "pgrep", "-f", "codex"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    if !output.status.success() {
        // pgrep exits 1 when nothing matches — that is a real result.
        return Some(Vec::new());
    }
    let pids = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().parse::<u32>().ok())
        .collect();
    Some(pids)
}

#[tauri::command]
pub async fn get_wsl_config() -> Result<serde_json::Value, String> {
    let settings = crate::auth::load_app_settings().map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "supported": cfg!(windows),
        "enabled": settings.use_wsl_codex,
        "distro": settings.wsl_distro,
        "distros": list_distros(),
    }))
}

#[tauri::command]
pub async fn set_wsl_config(enabled: bool, distro: Option<String>) -> Result<(), String> {
    if enabled && !cfg!(windows) {
        return Err("WSL Codex is only supported on Windows.".into());
    }
    let mut settings = crate::auth::load_app_settings().map_err(|e| e.to_string())?;
    settings.use_wsl_codex = enabled;
    settings.wsl_distro = distro.filter(|d| !d.trim().is_empty());
    crate::auth::save_app_settings(&settings).map_err(|e| e.to_string())
}
