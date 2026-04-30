//! Codex Switcher - Multi-account manager for Codex CLI

pub mod api;
pub mod auth;
pub mod commands;
pub mod types;
pub mod web;

use std::fs;

use anyhow::{Context, Result};
use commands::{
    add_account_from_file, cancel_login, check_codex_processes, complete_login, delete_account,
    export_accounts_full_encrypted_file, export_accounts_slim_text, get_active_account_info,
    get_masked_account_ids, get_usage, import_accounts_full_encrypted_file,
    import_accounts_slim_text, list_accounts, refresh_account_metadata, refresh_all_accounts_usage,
    rename_account, set_masked_account_ids, start_login, switch_account, warmup_account,
    warmup_all_accounts,
};

/// Load optional environment variables from ~/.codex-switcher/.env.
///
/// GUI launches on macOS do not inherit shell startup files, so proxy variables
/// placed in this file must be loaded before any reqwest clients are created.
pub fn load_config_env() -> Result<()> {
    let env_path = auth::storage::get_config_dir()?.join(".env");
    if !env_path.exists() {
        return Ok(());
    }

    let content = fs::read_to_string(&env_path)
        .with_context(|| format!("Failed to read env file: {}", env_path.display()))?;

    for (line_number, line) in content.lines().enumerate() {
        if let Some((key, value)) = parse_env_line(line)
            .with_context(|| format!("Invalid env file line {}", line_number + 1))?
        {
            if std::env::var_os(&key).is_none() {
                std::env::set_var(key, value);
            }
        }
    }

    Ok(())
}

fn parse_env_line(line: &str) -> Result<Option<(String, String)>> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return Ok(None);
    }

    let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
    let Some((key, value)) = line.split_once('=') else {
        anyhow::bail!("expected KEY=VALUE");
    };

    let key = key.trim();
    if key.is_empty() || !key.chars().all(is_env_key_char) {
        anyhow::bail!("invalid variable name");
    }

    Ok(Some((key.to_string(), parse_env_value(value.trim())?)))
}

fn is_env_key_char(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

fn parse_env_value(value: &str) -> Result<String> {
    if let Some(rest) = value.strip_prefix('"') {
        let Some(end) = rest.find('"') else {
            anyhow::bail!("unterminated double-quoted value");
        };
        return Ok(rest[..end].to_string());
    }

    if let Some(rest) = value.strip_prefix('\'') {
        let Some(end) = rest.find('\'') else {
            anyhow::bail!("unterminated single-quoted value");
        };
        return Ok(rest[..end].to_string());
    }

    let value_without_comment = value
        .split_once(" #")
        .map(|(before_comment, _)| before_comment)
        .unwrap_or(value);

    Ok(value_without_comment.trim_end().to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    if let Err(error) = load_config_env() {
        eprintln!("{error:#}");
    }

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
            refresh_account_metadata,
            refresh_all_accounts_usage,
            warmup_account,
            warmup_all_accounts,
            // Process detection
            check_codex_processes,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
