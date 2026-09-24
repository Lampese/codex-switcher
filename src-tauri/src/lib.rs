//! Codex Switcher - Multi-account manager for Codex CLI

pub mod api;
#[cfg(desktop)]
pub mod app_menu;
pub mod auth;
pub mod commands;
#[cfg(desktop)]
pub mod tray;
pub mod types;
pub mod web;

use commands::{
    ack_close_behavior_prompt, add_account_from_file, cancel_login, check_codex_processes,
    complete_close_behavior, complete_login, delete_account, export_accounts_full_encrypted_file,
    export_accounts_slim_text, get_account_usage_stats, get_active_account_info,
    get_dock_display_mode, get_masked_account_ids, get_usage, hide_tray_window,
    import_accounts_full_encrypted_file, import_accounts_slim_text, kill_codex_processes,
    list_accounts, open_main_window, quit_app, refresh_account_metadata,
    refresh_all_accounts_usage, rename_account, report_usage, set_dock_display_mode,
    set_masked_account_ids, start_login, switch_account, warmup_account, warmup_all_accounts,
};
use tauri::Emitter;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            // On macOS, a second process can be started by a login item or
            // launchd. That must not interrupt the foreground app. An explicit
            // Dock/Finder reopen is handled by RunEvent::Reopen below.
            #[cfg(not(target_os = "macos"))]
            commands::restore_main_window(app);
            #[cfg(target_os = "macos")]
            let _ = app;
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            #[cfg(desktop)]
            {
                app.handle()
                    .plugin(tauri_plugin_updater::Builder::new().build())?;
                app_menu::setup(app.handle())?;
                tray::setup(app.handle())?;
            }

            // Spawn background auto-recovery loop
            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    match commands::auto_recovery::check_and_recover_sessions().await {
                        Ok(Some(event)) => {
                            let _ = app_handle.emit("session-recovery-event", event);
                        }
                        Err(error) => eprintln!("[AutoRecovery] {error:#}"),
                        Ok(None) => {}
                    }
                }
            });

            Ok(())
        })
        .on_window_event(|window, event| {
            #[cfg(desktop)]
            if window.label() == "main" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    #[cfg(target_os = "macos")]
                    if commands::should_prompt_for_close_behavior() {
                        let payload = commands::window::next_close_behavior_prompt_payload();
                        let app_handle = tauri::Manager::app_handle(window);
                        commands::window::schedule_close_behavior_prompt_fallback(
                            app_handle.clone(),
                            payload.request_id,
                        );
                        let _ =
                            window.emit(commands::window::CLOSE_BEHAVIOR_REQUESTED_EVENT, payload);
                        return;
                    }
                    commands::hide_main_window(&tauri::Manager::app_handle(window));
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_display_settings,
            commands::set_tray_display_mode,
            commands::open_codex_app,
            commands::get_codex_reopen_info,
            commands::reopen_closed_codex_desktop,
            // Auto Recovery & Session Automation
            commands::get_app_settings,
            commands::get_auto_recovery_status,
            commands::trigger_auto_recovery_check,
            commands::launch_codex_session,
            commands::save_auto_recovery_settings,
            // Account management
            list_accounts,
            get_active_account_info,
            add_account_from_file,
            switch_account,
            delete_account,
            rename_account,
            export_accounts_slim_text,
            import_accounts_slim_text,
            export_accounts_full_encrypted_file,
            import_accounts_full_encrypted_file,
            // Masked accounts
            get_masked_account_ids,
            set_masked_account_ids,
            // OAuth
            start_login,
            complete_login,
            cancel_login,
            // Usage
            get_usage,
            get_account_usage_stats,
            commands::redeem_account_reset_credit,
            refresh_account_metadata,
            refresh_all_accounts_usage,
            warmup_account,
            warmup_all_accounts,
            // Process detection
            check_codex_processes,
            kill_codex_processes,
            // Tray window
            hide_tray_window,
            open_main_window,
            quit_app,
            report_usage,
            get_dock_display_mode,
            set_dock_display_mode,
            complete_close_behavior,
            ack_close_behavior_prompt,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app, _event| {
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = _event {
                commands::restore_main_window(_app);
            }
        });
}
