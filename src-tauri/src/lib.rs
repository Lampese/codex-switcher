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

use crate::auth::account_repository::AccountRepository;
use crate::auth::paths::AppPaths;
use commands::{
    ack_close_behavior_prompt, cancel_login, check_codex_processes, complete_close_behavior,
    export_accounts_full_encrypted_file, export_accounts_slim_text, get_account_usage_stats,
    get_dock_display_mode, get_usage, hide_tray_window, import_accounts_full_encrypted_file,
    import_accounts_slim_text, kill_codex_processes, open_main_window, quit_app,
    refresh_account_metadata, refresh_all_accounts_usage, report_usage, set_dock_display_mode,
    start_login, switch_account, warmup_account, warmup_all_accounts,
};
use tauri::{Emitter, Manager};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            let paths = AppPaths::production()
                .map_err(|_| std::io::Error::other("Failed to resolve account storage paths"))?;
            let repository = AccountRepository::from_paths(paths);
            tauri::async_runtime::block_on(repository.validate_startup_state())
                .map_err(|_| std::io::Error::other("Account storage validation failed"))?;
            app.manage(repository);

            #[cfg(desktop)]
            {
                app.handle()
                    .plugin(tauri_plugin_updater::Builder::new().build())?;
                app_menu::setup(app.handle())?;
                tray::setup(app.handle())?;
            }
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
            commands::open_codex_app,
            // Account management
            commands::account::read_only_tauri_commands::list_accounts,
            commands::account::read_only_tauri_commands::get_active_account_info,
            commands::account::secure_mutation_tauri_commands::add_account_from_file,
            switch_account,
            commands::account::secure_mutation_tauri_commands::delete_account,
            commands::account::secure_mutation_tauri_commands::rename_account,
            export_accounts_slim_text,
            import_accounts_slim_text,
            export_accounts_full_encrypted_file,
            import_accounts_full_encrypted_file,
            // Masked accounts
            commands::account::read_only_tauri_commands::get_masked_account_ids,
            commands::account::secure_mutation_tauri_commands::set_masked_account_ids,
            // OAuth
            start_login,
            commands::oauth::secure_oauth_tauri_commands::complete_login,
            cancel_login,
            // Usage
            get_usage,
            get_account_usage_stats,
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
