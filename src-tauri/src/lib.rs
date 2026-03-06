//! Codex Switcher - Multi-account manager for Codex CLI

pub mod api;
pub mod auth;
pub mod commands;
pub mod scheduler;
pub mod types;

use commands::{
    add_account_from_file, cancel_login, check_codex_processes, complete_login, delete_account,
    dismiss_missed_scheduled_warmup, export_accounts_full_encrypted_file,
    export_accounts_slim_text, get_active_account_info, get_app_settings,
    get_scheduled_warmup_status, get_usage, import_accounts_full_encrypted_file,
    import_accounts_slim_text, list_accounts, refresh_all_accounts_usage, rename_account,
    run_scheduled_warmup_now, save_export_security_mode, save_scheduled_warmup_settings,
    start_login, switch_account, warmup_account, warmup_all_accounts,
};
use scheduler::{spawn_scheduler, ScheduledWarmupRuntimeState};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(ScheduledWarmupRuntimeState::new())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            spawn_scheduler(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
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
            get_app_settings,
            save_export_security_mode,
            save_scheduled_warmup_settings,
            get_scheduled_warmup_status,
            dismiss_missed_scheduled_warmup,
            run_scheduled_warmup_now,
            // OAuth
            start_login,
            complete_login,
            cancel_login,
            // Usage
            get_usage,
            refresh_all_accounts_usage,
            warmup_account,
            warmup_all_accounts,
            // Process detection
            check_codex_processes,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
