//! Codex Switcher - Multi-account manager for Codex CLI

pub mod api;
pub mod auth;
pub mod commands;
pub mod types;
pub mod web;

use commands::{
    add_account_from_file, bind_instance_account, cancel_login, check_codex_processes,
    clear_codex_auth, complete_login, create_empty_instance, create_instance, delete_account,
    export_accounts_full_encrypted_file, export_accounts_slim_text, get_active_account_info,
    get_active_instance, get_instance_launch_command, get_masked_account_ids, get_usage,
    import_accounts_full_encrypted_file, import_accounts_slim_text, is_auto_usage_poll_active,
    launch_instance_codex, list_accounts, list_instances, refresh_account_metadata,
    refresh_all_accounts_usage, remove_instance, rename_account, set_active_instance,
    set_masked_account_ids, start_auto_usage_poll, start_login, stop_auto_usage_poll,
    switch_account, warmup_account, warmup_all_accounts,
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

            if let Err(err) = auth::instance_manager::restore_active_instance_env() {
                println!("[InstanceManager] Failed to restore active instance: {err}");
            }

            // Start background token keeper (refreshes ChatGPT tokens proactively)
            auth::token_keeper::start(app.handle().clone());

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // Account management
            list_accounts,
            get_active_account_info,
            add_account_from_file,
            switch_account,
            delete_account,
            clear_codex_auth,
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
            refresh_account_metadata,
            refresh_all_accounts_usage,
            warmup_account,
            warmup_all_accounts,
            // Auto-poll
            start_auto_usage_poll,
            stop_auto_usage_poll,
            is_auto_usage_poll_active,
            // Instances
            list_instances,
            create_instance,
            create_empty_instance,
            set_active_instance,
            get_active_instance,
            remove_instance,
            bind_instance_account,
            get_instance_launch_command,
            launch_instance_codex,
            // Process detection
            check_codex_processes,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
