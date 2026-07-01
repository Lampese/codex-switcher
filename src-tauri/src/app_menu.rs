//! Native application menu management.

use tauri::{
    menu::{AboutMetadata, CheckMenuItem, Menu, PredefinedMenuItem, Submenu},
    AppHandle, Runtime,
};

#[cfg(target_os = "macos")]
pub(crate) use crate::types::DockDisplayMode;
use crate::{
    auth::{load_app_settings, save_app_settings},
    types::{AppSettings, TrayDisplayMode},
};

const TRAY_ICON_AND_SESSION_ID: &str = "tray-display-icon-and-session";
const TRAY_ACTIVE_USAGE_TEXT_ID: &str = "tray-display-active-usage-text";
const TRAY_HIDDEN_ID: &str = "tray-display-hidden";
#[cfg(target_os = "macos")]
pub(crate) const DOCK_SHOW_IN_DOCK_ID: &str = "dock-display-show-in-dock";
#[cfg(target_os = "macos")]
pub(crate) const DOCK_MENU_BAR_ONLY_ID: &str = "dock-display-menu-bar-only";

pub fn setup(app: &AppHandle) -> tauri::Result<()> {
    #[cfg(target_os = "macos")]
    apply_saved_dock_display_mode(app);
    refresh(app)?;
    app.on_menu_event(handle_menu_event);
    Ok(())
}

pub fn refresh<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let settings = load_app_settings().unwrap_or_default();
    let menu = build_menu(app, &settings)?;
    app.set_menu(menu)?;
    Ok(())
}

fn handle_menu_event(app: &AppHandle, event: tauri::menu::MenuEvent) {
    let item_id = event.id();

    if let Some(mode) = tray_display_mode_for_item(item_id.as_ref()) {
        update_tray_display_mode(app, mode);
        return;
    }

    #[cfg(target_os = "macos")]
    if let Some(mode) = dock_display_mode_for_item(item_id.as_ref()) {
        update_dock_display_mode(app, mode);
    }
}

fn tray_display_mode_for_item(item_id: &str) -> Option<TrayDisplayMode> {
    Some(match item_id {
        TRAY_ICON_AND_SESSION_ID => TrayDisplayMode::IconAndSession,
        TRAY_ACTIVE_USAGE_TEXT_ID => TrayDisplayMode::ActiveUsageText,
        TRAY_HIDDEN_ID => TrayDisplayMode::Hidden,
        _ => return None,
    })
}

pub(crate) fn update_tray_display_mode(app: &AppHandle, mode: TrayDisplayMode) {
    let mut settings = load_app_settings().unwrap_or_default();
    if settings.tray_display_mode == mode {
        return;
    }

    settings.tray_display_mode = mode;
    #[cfg(target_os = "macos")]
    let dock_mode_changed = ensure_dock_entry_for_tray_mode(&mut settings);
    if let Err(error) = save_app_settings(&settings) {
        eprintln!("Failed to save app settings: {error}");
        return;
    }

    #[cfg(target_os = "macos")]
    if dock_mode_changed {
        apply_dock_display_mode(app, settings.dock_display_mode);
    }
    if let Err(error) = refresh(app) {
        eprintln!("Failed to refresh app menu: {error}");
    }
    crate::tray::refresh(app);
}

#[cfg(target_os = "macos")]
pub(crate) fn dock_display_mode_for_item(item_id: &str) -> Option<DockDisplayMode> {
    Some(match item_id {
        DOCK_SHOW_IN_DOCK_ID => DockDisplayMode::ShowInDock,
        DOCK_MENU_BAR_ONLY_ID => DockDisplayMode::MenuBarOnly,
        _ => return None,
    })
}

#[cfg(target_os = "macos")]
pub(crate) fn update_dock_display_mode(app: &AppHandle, mode: DockDisplayMode) {
    if let Err(error) = set_dock_display_mode(app, mode) {
        eprintln!("Failed to update Dock display mode: {error}");
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn set_dock_display_mode<R: Runtime>(
    app: &AppHandle<R>,
    mode: DockDisplayMode,
) -> anyhow::Result<AppSettings> {
    let mut settings = load_app_settings().unwrap_or_default();
    if settings.dock_display_mode == mode {
        let changed = ensure_menu_bar_entry_for_dock_mode(&mut settings);
        if changed {
            save_app_settings(&settings)?;
        }
        apply_dock_display_mode(app, mode);
        if changed {
            if let Err(error) = refresh(app) {
                eprintln!("Failed to refresh app menu: {error}");
            }
            crate::tray::refresh(app);
        }
        return Ok(settings);
    }

    settings.dock_display_mode = mode;
    ensure_menu_bar_entry_for_dock_mode(&mut settings);
    save_app_settings(&settings)?;
    apply_dock_display_mode(app, mode);

    if let Err(error) = refresh(app) {
        eprintln!("Failed to refresh app menu: {error}");
    }
    crate::tray::refresh(app);
    Ok(settings)
}

#[cfg(target_os = "macos")]
fn apply_saved_dock_display_mode<R: Runtime>(app: &AppHandle<R>) {
    let mut settings = load_app_settings().unwrap_or_default();
    let changed = ensure_menu_bar_entry_for_dock_mode(&mut settings);
    if changed {
        if let Err(error) = save_app_settings(&settings) {
            eprintln!("Failed to save app settings: {error}");
        }
    }
    apply_dock_display_mode(app, settings.dock_display_mode);
}

#[cfg(target_os = "macos")]
fn ensure_menu_bar_entry_for_dock_mode(settings: &mut AppSettings) -> bool {
    if settings.dock_display_mode == DockDisplayMode::MenuBarOnly
        && settings.tray_display_mode == TrayDisplayMode::Hidden
    {
        settings.tray_display_mode = TrayDisplayMode::ActiveUsageText;
        true
    } else {
        false
    }
}

#[cfg(target_os = "macos")]
fn ensure_dock_entry_for_tray_mode(settings: &mut AppSettings) -> bool {
    if settings.tray_display_mode == TrayDisplayMode::Hidden
        && settings.dock_display_mode == DockDisplayMode::MenuBarOnly
    {
        settings.dock_display_mode = DockDisplayMode::ShowInDock;
        true
    } else {
        false
    }
}

#[cfg(target_os = "macos")]
fn apply_dock_display_mode<R: Runtime>(app: &AppHandle<R>, mode: DockDisplayMode) {
    let visible = mode == DockDisplayMode::ShowInDock;
    if let Err(error) = app.set_dock_visibility(visible) {
        eprintln!("Failed to update Dock visibility: {error}");
    }
}

fn build_menu<R: Runtime>(app: &AppHandle<R>, settings: &AppSettings) -> tauri::Result<Menu<R>> {
    let pkg_info = app.package_info();
    let config = app.config();
    let about_metadata = AboutMetadata {
        name: Some(pkg_info.name.clone()),
        version: Some(pkg_info.version.to_string()),
        copyright: config.bundle.copyright.clone(),
        authors: config
            .bundle
            .publisher
            .clone()
            .map(|publisher| vec![publisher]),
        ..Default::default()
    };

    let tray_settings = Submenu::with_items(
        app,
        "Tray",
        true,
        &[
            &CheckMenuItem::with_id(
                app,
                TRAY_ICON_AND_SESSION_ID,
                "Icon + Session",
                true,
                settings.tray_display_mode == TrayDisplayMode::IconAndSession,
                None::<&str>,
            )?,
            &CheckMenuItem::with_id(
                app,
                TRAY_ACTIVE_USAGE_TEXT_ID,
                "Hourly + Weekly",
                true,
                settings.tray_display_mode == TrayDisplayMode::ActiveUsageText,
                None::<&str>,
            )?,
            &CheckMenuItem::with_id(
                app,
                TRAY_HIDDEN_ID,
                "Hidden",
                true,
                settings.tray_display_mode == TrayDisplayMode::Hidden,
                None::<&str>,
            )?,
        ],
    )?;

    #[cfg(target_os = "macos")]
    let dock_settings = Submenu::with_items(
        app,
        "Dock Icon",
        true,
        &[
            &CheckMenuItem::with_id(
                app,
                DOCK_SHOW_IN_DOCK_ID,
                "Show in Dock",
                true,
                settings.dock_display_mode == DockDisplayMode::ShowInDock,
                None::<&str>,
            )?,
            &CheckMenuItem::with_id(
                app,
                DOCK_MENU_BAR_ONLY_ID,
                "Menu Bar Only",
                true,
                settings.dock_display_mode == DockDisplayMode::MenuBarOnly,
                None::<&str>,
            )?,
        ],
    )?;

    #[cfg(target_os = "macos")]
    let settings_menu =
        Submenu::with_items(app, "Settings", true, &[&tray_settings, &dock_settings])?;

    #[cfg(not(target_os = "macos"))]
    let settings_menu = Submenu::with_items(app, "Settings", true, &[&tray_settings])?;

    let window_menu = Submenu::with_items(
        app,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::maximize(app, None)?,
            #[cfg(target_os = "macos")]
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::close_window(app, None)?,
        ],
    )?;

    let help_menu = Submenu::with_items(app, "Help", true, &[])?;

    Menu::with_items(
        app,
        &[
            #[cfg(target_os = "macos")]
            &Submenu::with_items(
                app,
                pkg_info.name.clone(),
                true,
                &[
                    &PredefinedMenuItem::about(app, None, Some(about_metadata))?,
                    &PredefinedMenuItem::separator(app)?,
                    &settings_menu,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::services(app, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::hide(app, None)?,
                    &PredefinedMenuItem::hide_others(app, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::quit(app, None)?,
                ],
            )?,
            #[cfg(not(any(
                target_os = "linux",
                target_os = "dragonfly",
                target_os = "freebsd",
                target_os = "netbsd",
                target_os = "openbsd"
            )))]
            &Submenu::with_items(
                app,
                "File",
                true,
                &[
                    &PredefinedMenuItem::close_window(app, None)?,
                    #[cfg(not(target_os = "macos"))]
                    &PredefinedMenuItem::quit(app, None)?,
                ],
            )?,
            &Submenu::with_items(
                app,
                "Edit",
                true,
                &[
                    &PredefinedMenuItem::undo(app, None)?,
                    &PredefinedMenuItem::redo(app, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::cut(app, None)?,
                    &PredefinedMenuItem::copy(app, None)?,
                    &PredefinedMenuItem::paste(app, None)?,
                    &PredefinedMenuItem::select_all(app, None)?,
                ],
            )?,
            #[cfg(target_os = "macos")]
            &Submenu::with_items(
                app,
                "View",
                true,
                &[&PredefinedMenuItem::fullscreen(app, None)?],
            )?,
            &window_menu,
            &help_menu,
        ],
    )
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::{ensure_dock_entry_for_tray_mode, ensure_menu_bar_entry_for_dock_mode};
    use crate::types::{AppSettings, DockDisplayMode, TrayDisplayMode};

    #[test]
    fn menu_bar_only_dock_mode_keeps_a_visible_tray_entry() {
        let mut settings = AppSettings {
            tray_display_mode: TrayDisplayMode::Hidden,
            dock_display_mode: DockDisplayMode::MenuBarOnly,
            ..Default::default()
        };

        assert!(ensure_menu_bar_entry_for_dock_mode(&mut settings));
        assert_eq!(settings.tray_display_mode, TrayDisplayMode::ActiveUsageText);
        assert_eq!(settings.dock_display_mode, DockDisplayMode::MenuBarOnly);
    }

    #[test]
    fn hidden_tray_mode_keeps_a_visible_dock_entry() {
        let mut settings = AppSettings {
            tray_display_mode: TrayDisplayMode::Hidden,
            dock_display_mode: DockDisplayMode::MenuBarOnly,
            ..Default::default()
        };

        assert!(ensure_dock_entry_for_tray_mode(&mut settings));
        assert_eq!(settings.tray_display_mode, TrayDisplayMode::Hidden);
        assert_eq!(settings.dock_display_mode, DockDisplayMode::ShowInDock);
    }
}
