use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use tauri::{
    menu::{CheckMenuItemBuilder, Menu, MenuItemBuilder, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, PhysicalPosition, Runtime, WebviewUrl, WebviewWindowBuilder,
    WindowEvent,
};

use crate::{
    auth::{get_accounts_file, load_accounts},
    commands::{is_codex_running_switch_block, switch_account_by_id, window::TRAY_WINDOW},
    types::{AccountsStore, UsageInfo},
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
    let builder = TrayIconBuilder::with_id(TRAY_ID)
        .icon(TRAY_ICON)
        .icon_as_template(true)
        .tooltip("Codex Switcher")
        .menu(&menu)
        .on_menu_event(handle_menu_event);

    #[cfg(not(target_os = "linux"))]
    let builder = builder
        .on_tray_icon_event(handle_tray_icon_event)
        .show_menu_on_left_click(false);

    builder.build(app)?;

    watch_accounts_file(app.clone());
    Ok(())
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
        for account in &store.accounts {
            let label = format!("{}{}", account.name, usage_suffix(&account.id));
            let item = CheckMenuItemBuilder::with_id(
                account_menu_id(&account.id),
                menu_label(&label),
            )
            .checked(store.active_account_id.as_deref() == Some(&account.id))
            .build(app)?;
            menu.append(&item)?;
        }
    }

    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItemBuilder::with_id(OPEN_ITEM_ID, "Open Codex Switcher").build(app)?)?;
    menu.append(&MenuItemBuilder::with_id(QUIT_ITEM_ID, "Quit").build(app)?)?;
    Ok(menu)
}

fn handle_menu_event(app: &AppHandle, event: tauri::menu::MenuEvent) {
    let item_id = event.id().as_ref();

    match item_id {
        OPEN_ITEM_ID => show_main_window(app),
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
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };

    match load_accounts()
        .map_err(|error| error.to_string())
        .and_then(|store| build_menu(app, &store).map_err(|error| error.to_string()))
    {
        Ok(menu) => {
            if let Err(error) = tray.set_menu(Some(menu)) {
                eprintln!("Failed to refresh tray menu: {error}");
            }
        }
        Err(error) => eprintln!("Failed to build tray menu: {error}"),
    }
}

fn show_main_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(tray) = app.get_webview_window(TRAY_WINDOW) {
        let _ = tray.hide();
    }
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
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
    if let Some(used) = usage.primary_used_percent {
        parts.push(format!("S:{:.0}%", (100.0 - used).max(0.0)));
    }
    if let Some(used) = usage.secondary_used_percent {
        parts.push(format!("W:{:.0}%", (100.0 - used).max(0.0)));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_tray_icon_is_not_an_opaque_block() {
        let alphas: Vec<_> = TRAY_ICON.rgba().iter().skip(3).step_by(4).copied().collect();
        let width = TRAY_ICON.width() as usize;

        assert_eq!(
            [alphas[0], alphas[width - 1], alphas[alphas.len() - width], alphas[alphas.len() - 1]],
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
}
