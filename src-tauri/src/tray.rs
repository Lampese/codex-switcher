use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use tauri::{
    menu::{CheckMenuItemBuilder, Menu, MenuItemBuilder, PredefinedMenuItem, Submenu},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, PhysicalPosition, Runtime, WebviewUrl, WebviewWindowBuilder,
    WindowEvent,
};

use crate::{
    api::usage::get_account_usage,
    auth::{get_account, get_accounts_file, load_accounts, load_app_settings},
    commands::{
        is_codex_running_switch_block, restore_main_window, switch_account_by_id,
        window::TRAY_WINDOW,
    },
    types::{AccountsStore, TrayDisplayMode, UsageInfo},
};

static TRAY_USAGE: LazyLock<Mutex<HashMap<String, UsageInfo>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

const TRAY_ID: &str = "codex-switcher-tray";
const TRAY_ICON: tauri::image::Image<'static> = tauri::include_image!("./icons/tray.png");
const TRAY_REFRESH_EVENT: &str = "tray-refresh";
const ACCOUNTS_CHANGED_EVENT: &str = "accounts-changed";
const SWITCH_ACCOUNT_BLOCKED_EVENT: &str = "switch-account-blocked";
const ACCOUNT_ITEM_PREFIX: &str = "account:";
const OPEN_ITEM_ID: &str = "open";
const OPEN_CODEX_ITEM_ID: &str = "open-codex";
const QUIT_ITEM_ID: &str = "quit";
const TRAY_WIDTH: f64 = 300.0;
const TRAY_HEIGHT: f64 = 420.0;

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SwitchAccountBlockedPayload {
    account_id: String,
    error: String,
}

pub fn setup(app: &AppHandle) -> tauri::Result<()> {
    #[cfg(not(target_os = "linux"))]
    create_tray_window(app)?;

    let menu = build_menu(app, &load_accounts().unwrap_or_default())?;

    #[cfg(target_os = "linux")]
    let icon = app
        .default_window_icon()
        .cloned()
        .expect("application icon should be configured");

    #[cfg(not(target_os = "linux"))]
    let icon = TRAY_ICON;

    let builder = TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .tooltip("Codex Switcher")
        .menu(&menu)
        .on_menu_event(handle_menu_event);

    #[cfg(target_os = "macos")]
    let builder = builder.icon_as_template(true);

    #[cfg(not(target_os = "linux"))]
    let builder = builder
        .on_tray_icon_event(handle_tray_icon_event)
        .show_menu_on_left_click(false);

    builder.build(app)?;
    refresh_menu(app);

    watch_accounts_file(app.clone());
    poll_active_account_usage(app.clone());
    Ok(())
}

pub fn refresh<R: Runtime>(app: &AppHandle<R>) {
    refresh_menu(app);
}

/// Store usage reported by the main app and refresh the native menu labels.
pub fn ingest_usage<R: Runtime>(app: &AppHandle<R>, usages: Vec<UsageInfo>) {
    if let Ok(mut cache) = TRAY_USAGE.lock() {
        for usage in usages {
            cache.insert(usage.account_id.clone(), usage);
        }
    }
    refresh_menu(app);
}

// ============================================================================
// React popup window (used on macOS/Windows via tray click events)
// ============================================================================

#[cfg_attr(target_os = "linux", allow(dead_code))]
fn create_tray_window<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    if app.get_webview_window(TRAY_WINDOW).is_some() {
        return Ok(());
    }

    let window = WebviewWindowBuilder::new(app, TRAY_WINDOW, WebviewUrl::App("tray.html".into()))
        .title("Codex Switcher")
        .inner_size(TRAY_WIDTH, TRAY_HEIGHT)
        .resizable(false)
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false)
        .build()?;

    // Hide the popup as soon as it loses focus so it behaves like a native menu.
    let app_handle = app.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::Focused(false) = event {
            if let Some(window) = app_handle.get_webview_window(TRAY_WINDOW) {
                let _ = window.hide();
            }
        }
    });

    Ok(())
}

#[cfg_attr(target_os = "linux", allow(dead_code))]
fn handle_tray_icon_event<R: Runtime>(tray: &tauri::tray::TrayIcon<R>, event: TrayIconEvent) {
    if let TrayIconEvent::Click {
        button: MouseButton::Left,
        button_state: MouseButtonState::Up,
        position,
        ..
    } = event
    {
        toggle_tray_window(tray.app_handle(), position);
    }
}

#[cfg_attr(target_os = "linux", allow(dead_code))]
fn toggle_tray_window<R: Runtime>(app: &AppHandle<R>, cursor: PhysicalPosition<f64>) {
    let Some(window) = app.get_webview_window(TRAY_WINDOW) else {
        return;
    };

    if window.is_visible().unwrap_or(false) {
        let _ = window.hide();
        return;
    }

    position_near_cursor(&window, cursor);
    let _ = window.show();
    let _ = window.set_focus();
    let _ = app.emit_to(TRAY_WINDOW, TRAY_REFRESH_EVENT, ());
}

#[cfg_attr(target_os = "linux", allow(dead_code))]
fn position_near_cursor<R: Runtime>(
    window: &tauri::WebviewWindow<R>,
    cursor: PhysicalPosition<f64>,
) {
    let size = window.outer_size().ok();
    let width = size.map(|s| s.width as f64).unwrap_or(TRAY_WIDTH);
    let height = size.map(|s| s.height as f64).unwrap_or(TRAY_HEIGHT);

    let x = (cursor.x - width / 2.0).max(0.0);
    // macOS menu bar sits at the top, so drop the popup below the icon.
    // Other platforms keep the tray at the bottom, so float it above the cursor.
    let y = if cfg!(target_os = "macos") {
        cursor.y + 4.0
    } else {
        (cursor.y - height - 4.0).max(0.0)
    };

    let _ = window.set_position(PhysicalPosition::new(x, y));
}

// ============================================================================
// Native menu (the only tray interaction on Linux; right-click on macOS/Windows)
// ============================================================================

fn build_menu<R: Runtime>(app: &AppHandle<R>, store: &AccountsStore) -> tauri::Result<Menu<R>> {
    let menu = Menu::new(app)?;

    if store.accounts.is_empty() {
        menu.append(
            &MenuItemBuilder::with_id("empty", "No accounts configured")
                .enabled(false)
                .build(app)?,
        )?;
    } else {
        // Pin the active account at the top, then the rest in storage order.
        let mut sorted = store.accounts.iter().collect::<Vec<_>>();
        sorted.sort_by_key(|a| {
            if store.active_account_id.as_deref() == Some(&a.id) {
                0
            } else {
                1
            }
        });

        for account in sorted {
            let label = format!("{}{}", account.name, usage_suffix(&account.id));
            let item =
                CheckMenuItemBuilder::with_id(account_menu_id(&account.id), menu_label(&label))
                    .checked(store.active_account_id.as_deref() == Some(&account.id))
                    .build(app)?;
            menu.append(&item)?;
        }
    }

    menu.append(&PredefinedMenuItem::separator(app)?)?;
    #[cfg(target_os = "macos")]
    append_dock_settings_menu(app, &menu)?;
    #[cfg(target_os = "macos")]
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItemBuilder::with_id(OPEN_ITEM_ID, "Open Codex Switcher").build(app)?)?;
    menu.append(&MenuItemBuilder::with_id(OPEN_CODEX_ITEM_ID, "Open Codex").build(app)?)?;
    menu.append(&MenuItemBuilder::with_id(QUIT_ITEM_ID, "Quit").build(app)?)?;
    Ok(menu)
}

#[cfg(target_os = "macos")]
fn append_dock_settings_menu<R: Runtime>(app: &AppHandle<R>, menu: &Menu<R>) -> tauri::Result<()> {
    let settings = load_app_settings().unwrap_or_default();
    let dock_settings = Submenu::with_items(
        app,
        "Dock Icon",
        true,
        &[
            &CheckMenuItemBuilder::with_id(crate::app_menu::DOCK_SHOW_IN_DOCK_ID, "Show in Dock")
                .checked(settings.dock_display_mode == crate::app_menu::DockDisplayMode::ShowInDock)
                .build(app)?,
            &CheckMenuItemBuilder::with_id(crate::app_menu::DOCK_MENU_BAR_ONLY_ID, "Menu Bar Only")
                .checked(
                    settings.dock_display_mode == crate::app_menu::DockDisplayMode::MenuBarOnly,
                )
                .build(app)?,
        ],
    )?;
    menu.append(&dock_settings)?;
    Ok(())
}

fn handle_menu_event(app: &AppHandle, event: tauri::menu::MenuEvent) {
    let item_id = event.id().as_ref();

    #[cfg(target_os = "macos")]
    if let Some(mode) = crate::app_menu::dock_display_mode_for_item(item_id) {
        crate::app_menu::update_dock_display_mode(app, mode);
        return;
    }

    match item_id {
        OPEN_ITEM_ID => show_main_window(app),
        OPEN_CODEX_ITEM_ID => {
            tauri::async_runtime::spawn(async move {
                let _ = crate::commands::open_codex_app().await;
            });
        }
        QUIT_ITEM_ID => app.exit(0),
        _ => {
            let Some(account_id) = item_id.strip_prefix(ACCOUNT_ITEM_PREFIX) else {
                return;
            };

            if load_accounts()
                .ok()
                .and_then(|store| store.active_account_id)
                .as_deref()
                == Some(account_id)
            {
                refresh_menu(app);
                return;
            }

            if let Err(error) = switch_account_by_id(account_id) {
                eprintln!("Failed to switch account from tray: {error}");
                refresh_menu(app);
                if is_codex_running_switch_block(&error) {
                    show_main_window(app);
                    let _ = app.emit(
                        SWITCH_ACCOUNT_BLOCKED_EVENT,
                        SwitchAccountBlockedPayload {
                            account_id: account_id.to_string(),
                            error,
                        },
                    );
                }
                return;
            }

            refresh_menu(app);
            let _ = app.emit(ACCOUNTS_CHANGED_EVENT, ());
        }
    }
}

fn refresh_menu<R: Runtime>(app: &AppHandle<R>) {
    let app_handle = app.clone();
    if let Err(error) = app.run_on_main_thread(move || {
        refresh_menu_on_main_thread(&app_handle);
    }) {
        eprintln!("Failed to schedule tray menu refresh: {error}");
    }
}

fn refresh_menu_on_main_thread<R: Runtime>(app: &AppHandle<R>) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };

    match load_accounts()
        .map_err(|error| error.to_string())
        .and_then(|store| {
            let settings = load_app_settings().unwrap_or_default();
            let title = active_tray_title(
                store.active_account_id.as_deref(),
                settings.tray_display_mode,
            );
            let menu = build_menu(app, &store).map_err(|error| error.to_string())?;
            Ok((menu, title, settings.tray_display_mode))
        }) {
        Ok((menu, title, mode)) => {
            if let Err(error) = tray.set_menu(Some(menu)) {
                eprintln!("Failed to refresh tray menu: {error}");
            }
            refresh_tray_display(&tray, mode, title.as_deref());
        }
        Err(error) => eprintln!("Failed to build tray menu: {error}"),
    }
}

fn refresh_tray_display<R: Runtime>(
    tray: &tauri::tray::TrayIcon<R>,
    mode: TrayDisplayMode,
    title: Option<&str>,
) {
    match mode {
        TrayDisplayMode::IconAndSession => {
            if let Err(error) = tray.set_visible(true) {
                eprintln!("Failed to show tray icon: {error}");
            }
            #[cfg(not(target_os = "linux"))]
            {
                if let Err(error) = tray.set_icon(Some(TRAY_ICON)) {
                    eprintln!("Failed to refresh tray icon: {error}");
                }
                #[cfg(target_os = "macos")]
                {
                    if let Err(error) = tray.set_icon_as_template(true) {
                        eprintln!("Failed to refresh tray icon template mode: {error}");
                    }
                }
            }
            if let Err(error) = tray.set_title(title) {
                eprintln!("Failed to refresh tray title: {error}");
            }
        }
        TrayDisplayMode::ActiveUsageText => {
            if let Err(error) = tray.set_visible(true) {
                eprintln!("Failed to show tray icon: {error}");
            }
            #[cfg(target_os = "macos")]
            if let Err(error) = tray.set_icon(None) {
                eprintln!("Failed to hide tray icon: {error}");
            }
            #[cfg(target_os = "windows")]
            if let Err(error) = tray.set_icon(Some(TRAY_ICON)) {
                eprintln!("Failed to refresh tray icon: {error}");
            }
            if let Err(error) = tray.set_title(title) {
                eprintln!("Failed to refresh tray title: {error}");
            }
        }
        TrayDisplayMode::Hidden => {
            if let Err(error) = tray.set_title(None::<&str>) {
                eprintln!("Failed to clear tray title: {error}");
            }
            if let Err(error) = tray.set_visible(false) {
                eprintln!("Failed to hide tray icon: {error}");
            }
        }
    }
}

fn show_main_window<R: Runtime>(app: &AppHandle<R>) {
    restore_main_window(app);
}

// The tray title sits after the icon, e.g. "[icon] 66%".
fn active_session_title(active_account_id: Option<&str>) -> Option<String> {
    let active_account_id = active_account_id?;
    let cache = TRAY_USAGE.lock().ok()?;
    let usage = cache.get(active_account_id)?;
    session_remaining_title(
        usage.primary_used_percent.or(usage.secondary_used_percent),
        usage.error.is_some(),
    )
}

fn active_tray_title(active_account_id: Option<&str>, mode: TrayDisplayMode) -> Option<String> {
    match mode {
        TrayDisplayMode::IconAndSession => active_session_title(active_account_id),
        TrayDisplayMode::ActiveUsageText => Some(active_usage_title(active_account_id)),
        TrayDisplayMode::Hidden => None,
    }
}

fn active_usage_title(active_account_id: Option<&str>) -> String {
    let Some(active_account_id) = active_account_id else {
        return "Codex".to_string();
    };

    let usage = TRAY_USAGE
        .lock()
        .ok()
        .and_then(|cache| cache.get(active_account_id).cloned());

    match usage {
        Some(usage) if usage.error.is_none() => {
            usage_title(usage.primary_used_percent, usage.secondary_used_percent)
        }
        _ => "H:-- W:--".to_string(),
    }
}

fn usage_title(primary_used_percent: Option<f64>, secondary_used_percent: Option<f64>) -> String {
    let mut parts = Vec::new();
    if let Some(remaining) = remaining_percent_label(primary_used_percent) {
        parts.push(format!("H:{remaining}"));
    }
    if let Some(remaining) = remaining_percent_label(secondary_used_percent) {
        parts.push(format!("W:{remaining}"));
    }

    if parts.is_empty() {
        "H:-- W:--".to_string()
    } else {
        parts.join(" ")
    }
}

fn session_remaining_title(used_percent: Option<f64>, has_error: bool) -> Option<String> {
    if has_error {
        return None;
    }

    remaining_percent_label(used_percent)
}

fn remaining_percent_label(used_percent: Option<f64>) -> Option<String> {
    let used_percent = used_percent?;
    if !used_percent.is_finite() {
        return None;
    }

    Some(format!("{:.0}%", (100.0 - used_percent).clamp(0.0, 100.0)))
}

// "  —  S:73% W:51%" remaining-quota suffix for a menu label, or "" when unknown.
fn usage_suffix(account_id: &str) -> String {
    let Ok(cache) = TRAY_USAGE.lock() else {
        return String::new();
    };
    let Some(usage) = cache.get(account_id) else {
        return String::new();
    };
    if usage.error.is_some() {
        return String::new();
    }

    let mut parts = Vec::new();
    if let Some(remaining) = session_remaining_title(usage.primary_used_percent, false) {
        parts.push(format!("S:{remaining}"));
    }
    if let Some(used) = usage.secondary_used_percent {
        if used.is_finite() {
            parts.push(format!("W:{:.0}%", (100.0 - used).clamp(0.0, 100.0)));
        }
    }

    if parts.is_empty() {
        String::new()
    } else {
        format!("  —  {}", parts.join(" "))
    }
}

fn account_menu_id(account_id: &str) -> String {
    format!("{ACCOUNT_ITEM_PREFIX}{account_id}")
}

fn menu_label(label: &str) -> String {
    label.replace('&', "&&")
}

// ============================================================================
// Shared: react to external account changes
// ============================================================================

fn watch_accounts_file<R: Runtime>(app: AppHandle<R>) {
    std::thread::spawn(move || {
        let accounts_path = match get_accounts_file() {
            Ok(path) => path,
            Err(error) => {
                eprintln!("Failed to resolve accounts file for tray: {error}");
                return;
            }
        };
        let mut last_modified = modified_at(&accounts_path);

        loop {
            std::thread::sleep(Duration::from_secs(1));
            let modified = modified_at(&accounts_path);
            if modified != last_modified {
                last_modified = modified;
                refresh_menu(&app); // keep the native menu current
                let _ = app.emit(ACCOUNTS_CHANGED_EVENT, ()); // refresh the React UIs
            }
        }
    });
}

fn modified_at(path: &std::path::Path) -> Option<std::time::SystemTime> {
    path.metadata()
        .and_then(|metadata| metadata.modified())
        .ok()
}

/// Poll the active account's usage so the tray title stays fresh even when the
/// main window's webview poller is hidden or suspended by the OS.
fn poll_active_account_usage<R: Runtime>(app: AppHandle<R>) {
    std::thread::spawn(move || loop {
        let account = load_accounts()
            .ok()
            .and_then(|store| store.active_account_id)
            .and_then(|id| get_account(&id).ok().flatten());

        if let Some(account) = account {
            match tauri::async_runtime::block_on(get_account_usage(&account)) {
                // Keep the last known title on transient fetch errors.
                Ok(usage) => ingest_usage(&app, vec![usage]),
                Err(error) => eprintln!("Failed to poll usage for tray title: {error}"),
            }
        }

        std::thread::sleep(Duration::from_secs(60));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_tray_icon_is_not_an_opaque_block() {
        let alphas: Vec<_> = TRAY_ICON
            .rgba()
            .iter()
            .skip(3)
            .step_by(4)
            .copied()
            .collect();
        let width = TRAY_ICON.width() as usize;

        assert_eq!(
            [
                alphas[0],
                alphas[width - 1],
                alphas[alphas.len() - width],
                alphas[alphas.len() - 1]
            ],
            [0, 0, 0, 0]
        );
        assert!(alphas.contains(&0));
        assert!(alphas.contains(&255));
    }

    #[test]
    fn account_ids_are_namespaced_for_tray_events() {
        assert_eq!(account_menu_id("abc-123"), "account:abc-123");
    }

    #[test]
    fn menu_labels_escape_mnemonic_markers() {
        assert_eq!(
            menu_label("Research & Development"),
            "Research && Development"
        );
    }

    #[test]
    fn session_title_shows_remaining_percentage() {
        assert_eq!(
            session_remaining_title(Some(34.0), false),
            Some("66%".to_string())
        );
    }

    #[test]
    fn session_title_hides_unknown_or_invalid_usage() {
        assert_eq!(session_remaining_title(None, false), None);
        assert_eq!(session_remaining_title(Some(f64::NAN), false), None);
        assert_eq!(session_remaining_title(Some(34.0), true), None);
    }

    #[test]
    fn session_title_clamps_remaining_percentage() {
        assert_eq!(
            session_remaining_title(Some(-5.0), false),
            Some("100%".to_string())
        );
        assert_eq!(
            session_remaining_title(Some(105.0), false),
            Some("0%".to_string())
        );
    }

    #[test]
    fn usage_title_omits_missing_windows() {
        assert_eq!(usage_title(Some(27.0), Some(82.0)), "H:73% W:18%");
        assert_eq!(usage_title(None, Some(35.0)), "W:65%");
        assert_eq!(usage_title(Some(27.0), None), "H:73%");
        assert_eq!(usage_title(None, None), "H:-- W:--");
    }

    #[test]
    fn active_usage_title_falls_back_when_usage_is_missing() {
        assert_eq!(active_usage_title(Some("missing")), "H:-- W:--");
        assert_eq!(active_usage_title(None), "Codex");
    }

    #[test]
    fn hidden_tray_mode_has_no_title() {
        assert_eq!(
            active_tray_title(Some("active"), TrayDisplayMode::Hidden),
            None
        );
    }
}
