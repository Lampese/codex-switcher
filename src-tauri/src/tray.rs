use std::time::{Duration, SystemTime};

use tauri::{
    menu::{CheckMenuItemBuilder, Menu, MenuItemBuilder, PredefinedMenuItem},
    tray::TrayIconBuilder,
    AppHandle, Emitter, Manager, Runtime,
};

use crate::{
    auth::{get_accounts_file, load_accounts},
    commands::{is_codex_running_switch_block, switch_account_by_id},
    types::AccountsStore,
};

const TRAY_ID: &str = "codex-switcher-tray";
const ACCOUNT_ITEM_PREFIX: &str = "account:";
const OPEN_ITEM_ID: &str = "open";
const QUIT_ITEM_ID: &str = "quit";
const ACCOUNTS_CHANGED_EVENT: &str = "accounts-changed";
const SWITCH_ACCOUNT_BLOCKED_EVENT: &str = "switch-account-blocked";

pub fn setup(app: &AppHandle) -> tauri::Result<()> {
    let menu = build_menu(app, &load_accounts().unwrap_or_default())?;
    let icon = app
        .default_window_icon()
        .cloned()
        .expect("application icon should be configured");

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .tooltip("Codex Switcher")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(handle_menu_event)
        .build(app)?;

    watch_accounts_file(app.clone());
    Ok(())
}

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
            let item = CheckMenuItemBuilder::with_id(
                account_menu_id(&account.id),
                menu_label(&account.name),
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
                    let _ = app.emit(SWITCH_ACCOUNT_BLOCKED_EVENT, error);
                }
                return;
            }

            refresh_menu(app);
            let _ = app.emit(ACCOUNTS_CHANGED_EVENT, ());
        }
    }
}

fn show_main_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
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
                refresh_menu(&app);
            }
        }
    });
}

fn modified_at(path: &std::path::Path) -> Option<SystemTime> {
    path.metadata()
        .and_then(|metadata| metadata.modified())
        .ok()
}

fn account_menu_id(account_id: &str) -> String {
    format!("{ACCOUNT_ITEM_PREFIX}{account_id}")
}

fn menu_label(label: &str) -> String {
    label.replace('&', "&&")
}

#[cfg(test)]
mod tests {
    use super::*;
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
