//! Codex Switcher - Multi-account manager for Codex CLI

pub mod api;
pub mod auth;
pub mod auto_switch;
pub mod commands;
pub mod types;
pub mod web;

use commands::{
    add_account_from_file, auto_switch_status, cancel_login, check_codex_processes,
    clear_auto_switch_events, complete_login, delete_account, export_accounts_full_encrypted_file,
    export_accounts_slim_text, get_active_account_info, get_auto_switch_config,
    get_auto_switch_events, get_masked_account_ids, get_usage, import_accounts_full_encrypted_file,
    import_accounts_slim_text, list_accounts, refresh_all_accounts_usage, rename_account,
    set_auto_switch_config, set_masked_account_ids, start_auto_switch, start_login,
    stop_auto_switch, switch_account, warmup_account, warmup_all_accounts,
};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            #[cfg(desktop)]
            app.handle()
                .plugin(tauri_plugin_updater::Builder::new().build())?;
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
            // Masked accounts
            get_masked_account_ids,
            set_masked_account_ids,
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
            // Auto-switch
            get_auto_switch_config,
            set_auto_switch_config,
            start_auto_switch,
            stop_auto_switch,
            auto_switch_status,
            get_auto_switch_events,
            clear_auto_switch_events,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
