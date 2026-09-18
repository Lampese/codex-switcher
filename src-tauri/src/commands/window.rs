//! Window and tray popup management commands.

use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use tauri::{AppHandle, Manager, Runtime};

use crate::{
    auth::{load_app_settings, mutate_app_settings},
    types::{DockDisplayMode, TrayDisplayMode, UsageInfo, WarmupPolicy, WarmupState},
};

/// Label of the borderless tray popup window.
pub const TRAY_WINDOW: &str = "tray";
pub const CLOSE_BEHAVIOR_REQUESTED_EVENT: &str = "close-behavior-requested";

static CLOSE_BEHAVIOR_PROMPT_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static CLOSE_BEHAVIOR_PROMPT_ACKED: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloseBehaviorRequestedPayload {
    pub request_id: u64,
}

/// Receive the main app's polled usage so the tray menu can show remaining quota
/// without doing its own fetching. The main window is the single usage poller.
#[tauri::command]
pub fn report_usage(app: AppHandle, usages: Vec<UsageInfo>) {
    #[cfg(desktop)]
    crate::tray::ingest_usage(&app, usages);
    #[cfg(not(desktop))]
    let _ = (app, usages);
}

/// Hide the tray popup window (called by the tray UI after an action).
#[tauri::command]
pub fn hide_tray_window(app: AppHandle) {
    if let Some(window) = app.get_webview_window(TRAY_WINDOW) {
        let _ = window.hide();
    }
}

/// Bring the main window to the foreground and hide the tray popup.
#[tauri::command]
pub fn open_main_window(app: AppHandle) {
    restore_main_window(&app);
}

pub fn hide_main_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
    #[cfg(target_os = "macos")]
    let _ = app.hide();
}

#[cfg(target_os = "macos")]
pub fn next_close_behavior_prompt_payload() -> CloseBehaviorRequestedPayload {
    CloseBehaviorRequestedPayload {
        request_id: CLOSE_BEHAVIOR_PROMPT_SEQUENCE.fetch_add(1, Ordering::Relaxed) + 1,
    }
}

#[cfg(target_os = "macos")]
pub fn schedule_close_behavior_prompt_fallback<R: Runtime>(app: AppHandle<R>, request_id: u64) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(750));
        if CLOSE_BEHAVIOR_PROMPT_ACKED.load(Ordering::SeqCst) >= request_id {
            return;
        }

        let app_handle = app.clone();
        if let Err(error) = app.run_on_main_thread(move || {
            hide_main_window(&app_handle);
        }) {
            eprintln!("Failed to schedule close prompt fallback: {error}");
        }
    });
}

/// Bring the main window to the foreground and hide the tray popup.
pub fn restore_main_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(tray) = app.get_webview_window(TRAY_WINDOW) {
        let _ = tray.hide();
    }
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Quit the whole application from the tray.
#[tauri::command]
pub fn quit_app(app: AppHandle) {
    app.exit(0);
}

#[derive(serde::Serialize)]
pub struct DisplaySettings {
    tray_display_mode: TrayDisplayMode,
    dock_display_mode: Option<DockDisplayMode>,
    language: String,
}

#[tauri::command]
pub fn get_display_settings() -> Result<DisplaySettings, String> {
    let settings = load_app_settings().map_err(|error| error.to_string())?;
    Ok(DisplaySettings {
        tray_display_mode: settings.tray_display_mode,
        dock_display_mode: if cfg!(target_os = "macos") {
            Some(settings.dock_display_mode)
        } else {
            None
        },
        language: settings.language,
    })
}

#[tauri::command]
pub fn get_warmup_policy() -> Result<WarmupPolicy, String> {
    Ok(load_app_settings()
        .map_err(|error| error.to_string())?
        .warmup_policy)
}

#[tauri::command]
pub fn get_warmup_state() -> Result<WarmupState, String> {
    crate::warmup_scheduler::get_state().map_err(|error| error.to_string())
}

#[tauri::command]
pub fn set_warmup_policy(policy: WarmupPolicy) -> Result<(), String> {
    crate::warmup_scheduler::set_policy(policy).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn get_language() -> String {
    crate::auth::load_app_settings()
        .map(|settings| settings.language)
        .unwrap_or_else(|_| "en-US".to_string())
}

#[tauri::command]
pub fn set_language(app: AppHandle, language: String) -> Result<(), String> {
    let normalized = if language.eq_ignore_ascii_case("zh-cn") {
        "zh-CN".to_string()
    } else if language.eq_ignore_ascii_case("en-us") {
        "en-US".to_string()
    } else {
        return Err(format!("Unsupported language: {language}"));
    };

    mutate_app_settings(|settings| {
        settings.language = normalized;
        Ok(())
    })
    .map_err(|error| error.to_string())?;
    #[cfg(desktop)]
    {
        crate::app_menu::refresh(&app).map_err(|error| error.to_string())?;
        crate::tray::refresh(&app);
    }
    #[cfg(not(desktop))]
    let _ = &app;
    Ok(())
}

#[tauri::command]
pub fn set_tray_display_mode(app: AppHandle, mode: TrayDisplayMode) -> Result<(), String> {
    #[cfg(desktop)]
    {
        crate::app_menu::set_tray_display_mode(&app, mode).map_err(|error| error.to_string())
    }
    #[cfg(not(desktop))]
    {
        let _ = (app, mode);
        Err("Tray settings are only available on desktop".into())
    }
}

#[tauri::command]
pub fn get_dock_display_mode() -> Option<DockDisplayMode> {
    #[cfg(target_os = "macos")]
    {
        Some(
            crate::auth::load_app_settings()
                .unwrap_or_default()
                .dock_display_mode,
        )
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

#[tauri::command]
pub fn set_dock_display_mode(
    app: AppHandle,
    mode: DockDisplayMode,
) -> Result<Option<DockDisplayMode>, String> {
    #[cfg(target_os = "macos")]
    {
        crate::app_menu::set_dock_display_mode(&app, mode)
            .map(|settings| Some(settings.dock_display_mode))
            .map_err(|error| error.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, mode);
        Ok(None)
    }
}

#[tauri::command]
pub fn complete_close_behavior(
    app: AppHandle,
    mode: DockDisplayMode,
    dont_ask_again: bool,
) -> Result<Option<DockDisplayMode>, String> {
    #[cfg(target_os = "macos")]
    {
        let settings = crate::app_menu::set_dock_display_mode(&app, mode)
            .map_err(|error| error.to_string())?;
        if dont_ask_again {
            mutate_app_settings(|settings| {
                settings.close_behavior_prompt_enabled = false;
                Ok(())
            })
            .map_err(|error| error.to_string())?;
        }
        hide_main_window(&app);
        Ok(Some(settings.dock_display_mode))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (mode, dont_ask_again);
        hide_main_window(&app);
        Ok(None)
    }
}

#[tauri::command]
pub fn ack_close_behavior_prompt(request_id: u64) {
    CLOSE_BEHAVIOR_PROMPT_ACKED.fetch_max(request_id, Ordering::SeqCst);
}

pub fn should_prompt_for_close_behavior() -> bool {
    #[cfg(target_os = "macos")]
    {
        load_app_settings()
            .unwrap_or_default()
            .close_behavior_prompt_enabled
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}
