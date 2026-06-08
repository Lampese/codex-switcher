use clap::Parser;
use console::style;
use dialoguer::{theme::ColorfulTheme, Confirm, Input, Select};
use codex_switcher_lib::commands::{
    cancel_login, complete_login, delete_account, list_accounts,
    refresh_all_accounts_usage, rename_account, start_login, switch_account,
};
use codex_switcher_lib::types::{AccountInfo, AuthMode, UsageInfo};
use codex_switcher_lib::auth::storage::get_config_dir;
use std::time::{Instant, Duration};
use std::io::{self, Write};
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, Clear, ClearType},
    cursor,
};
use tokio::sync::mpsc;
use chrono::Local;

#[derive(Parser)]
#[command(name = "codex-switch")]
#[command(author = "lampese")]
#[command(version = "0.2.2")]
#[command(about = "Codex Account Switcher - Interactive CLI", long_about = None)]
struct Cli {}

#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
struct CliConfig {
    refresh_interval_secs: u64,
}

fn load_cli_config() -> CliConfig {
    let path = get_config_dir().map(|p| p.join("cli-config.json"));
    if let Ok(p) = path {
        if p.exists() {
            if let Ok(content) = std::fs::read_to_string(&p) {
                if let Ok(config) = serde_json::from_str::<CliConfig>(&content) {
                    return config;
                }
            }
        } else {
            // Write default config
            let default_config = CliConfig { refresh_interval_secs: 60 };
            if let Ok(content) = serde_json::to_string_pretty(&default_config) {
                let _ = std::fs::write(&p, content);
            }
        }
    }
    CliConfig { refresh_interval_secs: 60 }
}

#[derive(Clone, PartialEq)]
enum MenuState {
    Main,
    SwitchSelect,
}

enum MainMenuChoice {
    SwitchAccount,
    AddAccount,
    RenameAccount,
    DeleteAccount,
    Exit,
}

enum SubMenuChoice {
    Select(AccountInfo),
    Back,
}

#[tokio::main]
async fn main() {
    let _cli = Cli::parse();
    let config = load_cli_config();

    if let Err(e) = run_interactive_loop(config).await {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), cursor::Show);
        eprintln!("{}", style(format!("Fatal Error: {}", e)).red().bold());
        std::process::exit(1);
    }
}

async fn run_interactive_loop(config: CliConfig) -> Result<(), String> {
    enable_raw_mode().map_err(|e| e.to_string())?;
    let mut stdout = io::stdout();
    execute!(stdout, cursor::Hide).map_err(|e| e.to_string())?;

    // State
    let mut accounts = list_accounts().await?;
    let mut usages: Vec<UsageInfo> = Vec::new();
    let mut menu_state = MenuState::Main;
    let mut selected_index = 0;
    let mut last_refresh = Instant::now() - Duration::from_secs(config.refresh_interval_secs + 1); // trigger initial refresh
    let mut last_refresh_str = "Never".to_string();
    let mut is_refreshing = false;
    let mut should_render = true;

    // Channel for background refresh
    let (tx, mut rx) = mpsc::channel::<Result<Vec<UsageInfo>, String>>(1);

    loop {
        // Check if background refresh finished
        if let Ok(res) = rx.try_recv() {
            is_refreshing = false;
            should_render = true;
            match res {
                Ok(new_usages) => {
                    usages = new_usages;
                    last_refresh = Instant::now();
                    last_refresh_str = Local::now().format("%H:%M:%S").to_string();
                }
                Err(_e) => {}
            }
        }

        // Trigger auto refresh if due and not currently refreshing
        if !is_refreshing && last_refresh.elapsed() >= Duration::from_secs(config.refresh_interval_secs) {
            is_refreshing = true;
            should_render = true;
            let tx_clone = tx.clone();
            tokio::spawn(async move {
                let res = refresh_all_accounts_usage().await;
                let _ = tx_clone.send(res).await;
            });
        }

        // Build current choice list based on menu state
        let mut main_choices = Vec::new();
        let mut sub_choices = Vec::new();
        let choices_len = match menu_state {
            MenuState::Main => {
                main_choices.push(MainMenuChoice::SwitchAccount);
                main_choices.push(MainMenuChoice::AddAccount);
                main_choices.push(MainMenuChoice::RenameAccount);
                main_choices.push(MainMenuChoice::DeleteAccount);
                main_choices.push(MainMenuChoice::Exit);
                main_choices.len()
            }
            MenuState::SwitchSelect => {
                for acc in &accounts {
                    sub_choices.push(SubMenuChoice::Select(acc.clone()));
                }
                sub_choices.push(SubMenuChoice::Back);
                sub_choices.len()
            }
        };

        // Keep selected index within bounds
        if selected_index >= choices_len {
            selected_index = choices_len.saturating_sub(1);
        }

        // Render screen only if state changed
        if should_render {
            render_screen(
                &accounts,
                &usages,
                &menu_state,
                &main_choices,
                &sub_choices,
                selected_index,
                is_refreshing,
                &last_refresh_str,
                &config,
            )?;
            should_render = false;
        }

        // Poll for key events (non-blocking wait)
        if event::poll(Duration::from_millis(50)).map_err(|e| e.to_string())? {
            if let Event::Key(key_event) = event::read().map_err(|e| e.to_string())? {
                if key_event.kind == event::KeyEventKind::Press {
                    match key_event.code {
                        KeyCode::Up | KeyCode::Char('k') => {
                            if selected_index > 0 {
                                selected_index -= 1;
                            } else {
                                selected_index = choices_len.saturating_sub(1); // wrap around
                            }
                            should_render = true;
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            if selected_index < choices_len.saturating_sub(1) {
                                selected_index += 1;
                            } else {
                                selected_index = 0; // wrap around
                            }
                            should_render = true;
                        }
                        KeyCode::Enter => {
                            match menu_state {
                                MenuState::Main => {
                                    let choice = &main_choices[selected_index];
                                    match choice {
                                        MainMenuChoice::Exit => {
                                            break;
                                        }
                                        MainMenuChoice::SwitchAccount => {
                                            menu_state = MenuState::SwitchSelect;
                                            selected_index = 0;
                                            should_render = true;
                                        }
                                        MainMenuChoice::AddAccount => {
                                            let _ = disable_raw_mode();
                                            let _ = execute!(io::stdout(), cursor::Show);
                                            if let Err(e) = add_account_prompt().await {
                                                println!("{}", style(format!("Failed to add account: {}", e)).red());
                                                wait_for_enter();
                                            }
                                            let _ = enable_raw_mode();
                                            let _ = execute!(io::stdout(), cursor::Hide);
                                            accounts = list_accounts().await?;
                                            should_render = true;
                                            // Trigger instant refresh
                                            last_refresh = Instant::now() - Duration::from_secs(config.refresh_interval_secs + 1);
                                        }
                                        MainMenuChoice::RenameAccount => {
                                            let _ = disable_raw_mode();
                                            let _ = execute!(io::stdout(), cursor::Show);
                                            if let Err(e) = rename_account_prompt(&accounts).await {
                                                println!("{}", style(format!("Rename failed: {}", e)).red());
                                                wait_for_enter();
                                            }
                                            let _ = enable_raw_mode();
                                            let _ = execute!(io::stdout(), cursor::Hide);
                                            accounts = list_accounts().await?;
                                            should_render = true;
                                        }
                                        MainMenuChoice::DeleteAccount => {
                                            let _ = disable_raw_mode();
                                            let _ = execute!(io::stdout(), cursor::Show);
                                            if let Err(e) = delete_account_prompt(&accounts).await {
                                                println!("{}", style(format!("Delete failed: {}", e)).red());
                                                wait_for_enter();
                                            }
                                            let _ = enable_raw_mode();
                                            let _ = execute!(io::stdout(), cursor::Hide);
                                            accounts = list_accounts().await?;
                                            should_render = true;
                                            // Trigger instant refresh
                                            last_refresh = Instant::now() - Duration::from_secs(config.refresh_interval_secs + 1);
                                        }
                                    }
                                }
                                MenuState::SwitchSelect => {
                                    let choice = &sub_choices[selected_index];
                                    match choice {
                                        SubMenuChoice::Back => {
                                            menu_state = MenuState::Main;
                                            selected_index = 0;
                                            should_render = true;
                                        }
                                        SubMenuChoice::Select(acc) => {
                                            if !acc.is_active {
                                                let _ = disable_raw_mode();
                                                let _ = execute!(io::stdout(), cursor::Show);
                                                println!("\nSwitching active account to '{}'...", acc.name);
                                                if let Err(e) = switch_account(acc.id.clone()).await {
                                                    println!("{}", style(format!("Failed to switch: {}", e)).red());
                                                    wait_for_enter();
                                                } else {
                                                    println!("{}", style(format!("Successfully switched to '{}'!", acc.name)).green().bold());
                                                    // Trigger instant refresh
                                                    last_refresh = Instant::now() - Duration::from_secs(config.refresh_interval_secs + 1);
                                                }
                                                let _ = enable_raw_mode();
                                                let _ = execute!(io::stdout(), cursor::Hide);
                                                accounts = list_accounts().await?;
                                            }
                                            menu_state = MenuState::Main;
                                            selected_index = 0;
                                            should_render = true;
                                        }
                                    }
                                }
                            }
                        }
                        KeyCode::Char('c') if key_event.modifiers.contains(KeyModifiers::CONTROL) => {
                            break; // Ctrl+C to exit
                        }
                        KeyCode::Char('r') => {
                            // Manual refresh trigger
                            if !is_refreshing {
                                is_refreshing = true;
                                should_render = true;
                                let tx_clone = tx.clone();
                                tokio::spawn(async move {
                                    let res = refresh_all_accounts_usage().await;
                                    let _ = tx_clone.send(res).await;
                                });
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), cursor::Show);
    println!("\nGoodbye!");
    Ok(())
}

fn render_screen(
    accounts: &[AccountInfo],
    usages: &[UsageInfo],
    menu_state: &MenuState,
    main_choices: &[MainMenuChoice],
    sub_choices: &[SubMenuChoice],
    selected_index: usize,
    is_refreshing: bool,
    last_refresh_str: &str,
    config: &CliConfig,
) -> Result<(), String> {
    let mut stdout = io::stdout();
    // Clear the screen and move cursor to (0,0)
    execute!(stdout, Clear(ClearType::All), cursor::MoveTo(0, 0)).map_err(|e| e.to_string())?;

    println!("{}", style("=== Codex Account Switcher ===").bold().cyan());
    println!();

    // 1. Render Usage Limits Table
    let mut max_name = 4;
    for acc in accounts {
        max_name = max_name.max(acc.name.len());
    }
    max_name = max_name.min(40);

    let h_name = "Name";
    let h_mode = "Mode";
    let h_primary = "Primary (5h)";
    let h_secondary = "Secondary (Weekly)";
    let h_credits = "Credits / Balance / Details";

    let header_str = format!(
        " {:<name_w$} | {:<6} | {:<28} | {:<28} | {} ",
        h_name, h_mode, h_primary, h_secondary, h_credits,
        name_w = max_name
    );
    println!("{}", style(&header_str).bold().underlined().cyan());

    for acc in accounts {
        let usage = usages.iter().find(|u| u.account_id == acc.id);
        
        let mode_str = match acc.auth_mode {
            AuthMode::ApiKey => "APIKey",
            AuthMode::ChatGPT => "OAuth ",
        };

        let name_val = if acc.name.len() > max_name { format!("{}...", &acc.name[..max_name - 3]) } else { acc.name.clone() };

        let (primary_val, secondary_val, credit_val) = match usage {
            Some(u) => {
                if let Some(err) = &u.error {
                    ("-".to_string(), "-".to_string(), style(err.clone()).dim().to_string())
                } else {
                    let prim = u.primary_used_percent.map(|p| render_bar(get_remaining_percent(p))).unwrap_or_else(|| "-".to_string());
                    let sec = u.secondary_used_percent.map(|p| render_bar(get_remaining_percent(p))).unwrap_or_else(|| "-".to_string());
                    let cred = if let Some(true) = u.unlimited_credits {
                        style("Unlimited Credits").green().to_string()
                    } else if let Some(balance) = &u.credits_balance {
                        format!("Credits: {}", balance)
                    } else if let Some(true) = u.has_credits {
                        style("Has Credits").green().to_string()
                    } else {
                        "-".to_string()
                    };
                    (prim, sec, cred)
                }
            }
            None => ("-".to_string(), "-".to_string(), style("No usage data").dim().to_string()),
        };

        let primary_padded = pad_styled_string(&primary_val, 28);
        let secondary_padded = pad_styled_string(&secondary_val, 28);

        let line = format!(
            " {:<name_w$} | {:<6} | {} | {} | {} ",
            name_val, mode_str, primary_padded, secondary_padded, credit_val,
            name_w = max_name
        );

        if acc.is_active {
            println!("{}", style(line).bold());
        } else {
            println!("{}", line);
        }
    }

    println!();

    // Show refresh status info
    let status_line = if is_refreshing {
        style("Refreshing usage limits...").yellow().dim().to_string()
    } else {
        format!(
            "Auto refresh every {}s (Last refresh at {}). Press 'r' to manual refresh.",
            config.refresh_interval_secs, last_refresh_str
        )
    };
    println!("{}", style(status_line).dim());
    println!();

    // 2. Render Interactive Menu
    match menu_state {
        MenuState::Main => {
            println!("{}", style("Select an action:").bold().yellow());
            for (idx, choice) in main_choices.iter().enumerate() {
                let is_selected = idx == selected_index;
                let prefix = if is_selected {
                    style("> ").green().bold().to_string()
                } else {
                    "  ".to_string()
                };

                let text = match choice {
                    MainMenuChoice::SwitchAccount => style("[Switch Account]").green().to_string(),
                    MainMenuChoice::AddAccount => style("[Add Account]").cyan().to_string(),
                    MainMenuChoice::RenameAccount => style("[Rename Account]").magenta().to_string(),
                    MainMenuChoice::DeleteAccount => style("[Delete Account]").red().to_string(),
                    MainMenuChoice::Exit => style("[Exit]").red().to_string(),
                };

                if is_selected {
                    println!("{}{}", prefix, style(text).underlined().bold());
                } else {
                    println!("{}{}", prefix, text);
                }
            }
        }
        MenuState::SwitchSelect => {
            println!("{}", style("Select an account to switch:").bold().yellow());
            for (idx, choice) in sub_choices.iter().enumerate() {
                let is_selected = idx == selected_index;
                let prefix = if is_selected {
                    style("> ").green().bold().to_string()
                } else {
                    "  ".to_string()
                };

                let text = match choice {
                    SubMenuChoice::Select(acc) => {
                        let active_marker = if acc.is_active {
                            style("* [Active]").green().bold().to_string()
                        } else {
                            "".to_string()
                        };
                        let details = acc.email.as_deref().unwrap_or(match acc.auth_mode {
                            AuthMode::ApiKey => "API Key",
                            AuthMode::ChatGPT => "ChatGPT",
                        });
                        format!("{} ({}) {}", acc.name, details, active_marker)
                    }
                    SubMenuChoice::Back => style("[Back]").yellow().to_string(),
                };

                if is_selected {
                    println!("{}{}", prefix, style(text).underlined().bold());
                } else {
                    println!("{}{}", prefix, text);
                }
            }
        }
    }

    let _ = io::stdout().flush();
    Ok(())
}

fn get_remaining_percent(used_percent: f64) -> f64 {
    (100.0 - used_percent).clamp(0.0, 100.0)
}

fn render_bar(remaining: f64) -> String {
    let clamped = remaining.clamp(0.0, 100.0);
    let filled_blocks = (clamped / 5.0).round() as usize;
    let empty_blocks = 20 - filled_blocks;
    let bar = format!("{}{}", "█".repeat(filled_blocks), "░".repeat(empty_blocks));
    
    if clamped > 30.0 {
        format!("[{}] {:.1}%", style(bar).green().bold(), remaining)
    } else if clamped > 10.0 {
        format!("[{}] {:.1}%", style(bar).yellow().bold(), remaining)
    } else {
        format!("[{}] {:.1}%", style(bar).red().bold(), remaining)
    }
}

fn pad_styled_string(s: &str, width: usize) -> String {
    let vis_len = visual_len(s);
    if vis_len >= width {
        s.to_string()
    } else {
        format!("{}{}", s, " ".repeat(width - vis_len))
    }
}

fn visual_len(s: &str) -> usize {
    let mut len = 0;
    let mut in_escape = false;
    for c in s.chars() {
        if c == '\x1b' {
            in_escape = true;
        } else if in_escape {
            if c == 'm' {
                in_escape = false;
            }
        } else {
            len += 1;
        }
    }
    len
}

fn wait_for_enter() {
    println!("\nPress Enter to return to the menu...");
    let mut buf = String::new();
    let _ = std::io::stdin().read_line(&mut buf);
}

async fn add_account_prompt() -> Result<(), String> {
    println!();
    println!("{}", style("=== Add Account ===").bold().cyan());
    
    let name: String = Input::with_theme(&ColorfulTheme::default())
        .with_prompt("Enter name for this account")
        .interact_text()
        .map_err(|e| e.to_string())?;
        
    if name.trim().is_empty() {
        return Err("Name cannot be empty".to_string());
    }
    
    println!("Generating login link and starting local server...");
    let info = start_login(name.trim().to_string()).await?;
    
    println!("Opening browser to complete authorization...");
    if let Err(e) = webbrowser::open(&info.auth_url) {
        println!("{}", style(format!("Could not open browser automatically: {}", e)).yellow());
        println!("Please open this URL manually in your browser:\n{}", info.auth_url);
    }
    
    println!("\nWaiting for authentication in browser...");
    println!("{}", style("Press ESC to cancel.").dim());

    // Switch back to raw mode to handle Esc key
    enable_raw_mode().map_err(|e| e.to_string())?;
    execute!(io::stdout(), cursor::Hide).map_err(|e| e.to_string())?;

    let login_fut = complete_login();
    tokio::pin!(login_fut);

    let result = loop {
        tokio::select! {
            res = &mut login_fut => {
                break res;
            }
            _ = tokio::time::sleep(Duration::from_millis(100)) => {
                if event::poll(Duration::from_millis(0)).unwrap_or(false) {
                    if let Event::Key(key_event) = event::read().unwrap() {
                        if key_event.kind == event::KeyEventKind::Press && key_event.code == KeyCode::Esc {
                            // User wants to cancel
                            let _ = disable_raw_mode();
                            let _ = execute!(io::stdout(), cursor::Show);
                            println!("\nCancelling OAuth login...");
                            let _ = cancel_login().await;
                            return Err("Login cancelled by user".to_string());
                        }
                    }
                }
            }
        }
    };

    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), cursor::Show);

    match result {
        Ok(acc_info) => {
            println!("{}", style(format!("\nAccount '{}' successfully added and set as active!", acc_info.name)).green().bold());
            wait_for_enter();
            Ok(())
        }
        Err(e) => Err(e),
    }
}

async fn rename_account_prompt(accounts: &[AccountInfo]) -> Result<(), String> {
    println!();
    println!("{}", style("=== Rename Account ===").bold().cyan());
    
    let mut options = Vec::new();
    for acc in accounts {
        options.push(acc.name.clone());
    }
    options.push("Cancel".to_string());

    let selection = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Select an account to rename")
        .items(&options)
        .default(0)
        .interact()
        .map_err(|e| e.to_string())?;

    if selection == options.len() - 1 {
        return Ok(()); // Canceled
    }

    let target = &accounts[selection];
    let new_name: String = Input::with_theme(&ColorfulTheme::default())
        .with_prompt(format!("Enter new name for '{}'", target.name))
        .interact_text()
        .map_err(|e| e.to_string())?;

    if new_name.trim().is_empty() {
        return Err("Name cannot be empty".to_string());
    }

    rename_account(target.id.clone(), new_name.trim().to_string()).await?;
    println!("{}", style("Account renamed successfully!").green().bold());
    wait_for_enter();
    Ok(())
}

async fn delete_account_prompt(accounts: &[AccountInfo]) -> Result<(), String> {
    println!();
    println!("{}", style("=== Delete Account ===").bold().cyan());
    
    let mut options = Vec::new();
    for acc in accounts {
        let active_mark = if acc.is_active { " (Active)" } else { "" };
        options.push(format!("{}{}", acc.name, active_mark));
    }
    options.push("Cancel".to_string());

    let selection = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Select an account to delete")
        .items(&options)
        .default(0)
        .interact()
        .map_err(|e| e.to_string())?;

    if selection == options.len() - 1 {
        return Ok(()); // Canceled
    }

    let target = &accounts[selection];
    
    let confirm = Confirm::with_theme(&ColorfulTheme::default())
        .with_prompt(format!("Are you sure you want to delete account '{}'?", target.name))
        .default(false)
        .interact()
        .map_err(|e| e.to_string())?;

    if confirm {
        delete_account(target.id.clone()).await?;
        println!("{}", style("Account deleted successfully!").green().bold());
    }
    wait_for_enter();
    Ok(())
}
