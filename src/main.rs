mod types;
mod config;
mod keyring;
mod google_api;
mod cli;
mod tui;

use std::io;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use crossterm::{
    execute,
    terminal::{enable_raw_mode, disable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    event::{self, EnableMouseCapture, DisableMouseCapture, KeyCode, MouseEventKind, MouseButton, Event as CEvent},
};
use ratatui::{
    backend::CrosstermBackend, Terminal,
    layout::{Layout, Direction, Constraint}
};
use serde_json::json;

use types::{AppEvent, InputMode, Focus, SortMode, NetworkResult, AddAccountAction};
use config::{load_accounts_list, get_active_email, load_cli_cache, load_warmup_history, get_data_dir, save_cli_cache, delete_account_from_db};
use tui::{App, spawn_network_task, ui::draw_ui};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (accounts, db_path, db_desc) = load_accounts_list();
    let active_email = get_active_email(&accounts);
    
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        let subcommand = &args[1];
        match subcommand.as_str() {
            "list" => {
                let mut is_json = false;
                for arg in args.iter().skip(2) {
                    if arg == "--json" {
                        is_json = true;
                    }
                }
                cli::cli_list(&accounts, active_email.as_deref(), &db_desc, is_json);
            }
            "switch" => {
                if args.len() < 3 {
                    cli::cli_switch_interactive(&accounts).await;
                } else {
                    cli::cli_switch(&accounts, &args[2]).await;
                }
            }
            "auto-switch" => {
                cli::cli_auto_switch(&accounts, active_email.as_deref()).await;
            }
            "quota" => {
                let mut identifier = None;
                let mut refresh = false;
                let mut is_json = false;
                for arg in args.iter().skip(2) {
                    if arg == "--refresh" || arg == "-r" {
                        refresh = true;
                    } else if arg == "--json" {
                        is_json = true;
                    } else if !arg.starts_with('-') {
                        identifier = Some(arg.as_str());
                    }
                }
                cli::cli_quota(&accounts, active_email.as_deref(), identifier, refresh, is_json).await;
            }
            "warmup" => {
                let mut identifier = None;
                let mut model = None;
                let mut force = false;
                let mut skip_next = false;
                
                for (i, arg) in args.iter().enumerate().skip(2) {
                    if skip_next {
                        skip_next = false;
                        continue;
                    }
                    if arg == "--force" {
                        force = true;
                    } else if arg == "--model" {
                        if i + 1 < args.len() {
                            model = Some(args[i + 1].clone());
                            skip_next = true;
                        } else {
                            eprintln!("Error: --model flag requires a value.");
                            std::process::exit(1);
                        }
                    } else if !arg.starts_with('-') {
                        identifier = Some(arg.as_str());
                    }
                }
                cli::cli_warmup(&accounts, active_email.as_deref(), identifier, model.as_deref(), force).await;
            }
            "add" => {
                cli::cli_add().await;
            }
            "check" => {
                cli::cli_check(&accounts).await;
            }
            "status" => {
                let mut is_json = false;
                for arg in args.iter().skip(2) {
                    if arg == "--json" {
                        is_json = true;
                    }
                }
                cli::cli_status(is_json);
            }
            "daemon" => {
                let action = args.get(2).map(|s| s.as_str()).unwrap_or("run");
                let mut quota = "gemini".to_string();
                let mut interval = 900;
                let mut skip = false;
                
                for (i, arg) in args.iter().enumerate().skip(3) {
                    if skip {
                        skip = false;
                        continue;
                    }
                    if arg == "--quota" {
                        if i + 1 < args.len() {
                            quota = args[i + 1].clone();
                            skip = true;
                        } else {
                            eprintln!("Error: --quota requires a value (gemini/claude/either).");
                            std::process::exit(1);
                        }
                    } else if arg == "--interval" {
                        if i + 1 < args.len() {
                            if let Ok(sec) = args[i + 1].parse::<u64>() {
                                interval = sec;
                            }
                            skip = true;
                        } else {
                            eprintln!("Error: --interval requires a value in seconds.");
                            std::process::exit(1);
                        }
                    }
                }
                
                match action {
                    "start" => cli::cli_daemon_start(&quota, interval),
                    "stop" => cli::cli_daemon_stop(),
                    "status" => cli::cli_daemon_status(),
                    "run" => cli::cli_daemon(&accounts, &quota, interval).await,
                    _ => {
                        let mut quota = "gemini".to_string();
                        let mut interval = 900;
                        let mut skip = false;
                        for (i, arg) in args.iter().enumerate().skip(2) {
                            if skip {
                                skip = false;
                                continue;
                            }
                            if arg == "--quota" {
                                if i + 1 < args.len() {
                                    quota = args[i + 1].clone();
                                    skip = true;
                                }
                            } else if arg == "--interval" {
                                if i + 1 < args.len() {
                                    if let Ok(sec) = args[i + 1].parse::<u64>() {
                                        interval = sec;
                                    }
                                    skip = true;
                                }
                            }
                        }
                        cli::cli_daemon(&accounts, &quota, interval).await;
                    }
                }
            }
            "remove" | "delete" => {
                cli::cli_remove(&accounts).await;
            }
            "backup" => {
                let filepath = args.get(2).map(|s| s.as_str());
                cli::cli_backup(&accounts, filepath);
            }
            "restore" => {
                if args.len() < 3 {
                    eprintln!("Usage: agm restore <backup_file_path>");
                    std::process::exit(1);
                }
                cli::cli_restore(&db_path, &args[2]);
            }
            "help" | "-h" | "--help" => {
                println!("Antigravity Manager (Rust Unified Edition)\n");
                println!("Usage:");
                println!("  agm                   Launch interactive terminal user interface (TUI)");
                println!("  agm list [--json]     List configured accounts (use --json for raw data)");
                println!("  agm add               Interactively add a new account and refresh token");
                println!("  agm remove            Interactively delete/remove an account from the database");
                println!("  agm check             Verify credentials and plans for all accounts concurrently");
                println!("  agm status [--json]   Get active account status and quotas (for tmux/sketchybar/prompts)");
                println!("  agm daemon run [...]  Start background failover daemon (--quota gemini/claude/either, --interval secs)");
                println!("  agm daemon start [...] Start background daemon in detached mode");
                println!("  agm daemon stop       Stop the running background daemon process");
                println!("  agm daemon status     Check if the background daemon is currently active");
                println!("  agm switch [id]       Switch the active account (runs interactive selector if no ID given)");
                println!("  agm auto-switch       Automatically switch to the healthiest/fullest standby account");
                println!("  agm quota [id] [-r]   Display quotas (use --refresh to update, --json for raw data)");
                println!("  agm quota all [-r]    Display/Refresh quotas for ALL accounts");
                println!("  agm warmup [id] [flg] Run warmup cycles (use --model <name> or --force)");
                println!("  agm warmup all        Sequentially warm up ALL configured accounts");
                println!("  agm backup [path]     Backup all configured accounts to a JSON file");
                println!("  agm restore <path>    Restore accounts from a JSON backup file");
                println!("\nExamples:");
                println!("  agm switch 3");
                println!("  agm switch");
                println!("  agm add");
                println!("  agm remove");
                println!("  agm status");
                println!("  agm daemon start --quota gemini --interval 300");
                println!("  agm daemon stop");
                println!("  agm daemon status");
                println!("  agm auto-switch");
                println!("  agm quota all --refresh --json");
                println!("  agm warmup all");
                println!("  agm backup ~/my_backup.json");
                println!("  agm restore ~/my_backup.json");
            }
            _ => {
                eprintln!("Unknown command '{}'. Type 'agm --help' for help.", subcommand);
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    // Default: Run TUI mode
    let cache = load_cli_cache();
    let history = load_warmup_history();
    
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    
    let tx = event_tx.clone();
    tokio::spawn(async move {
        loop {
            if event::poll(Duration::from_millis(30)).unwrap() {
                match event::read().unwrap() {
                    CEvent::Key(key) => {
                        let _ = tx.send(AppEvent::Key(key));
                    }
                    CEvent::Mouse(mouse) => {
                        let _ = tx.send(AppEvent::Mouse(mouse));
                    }
                    _ => {}
                }
            }
            let _ = tx.send(AppEvent::Tick);
        }
    });

    let mut app = App::new(accounts, db_path, db_desc, active_email, cache, history);

    if let Some(ref email) = app.active_email {
        if let Some(acc) = app.accounts.iter().find(|a| a.email == *email).cloned() {
            app.is_loading = true;
            app.set_status(&format!("Auto-verifying session validation for {}...", email));
            spawn_network_task(
                event_tx.clone(),
                Some(acc),
                Vec::new(),
                app.cli_cache.clone(),
                app.warmup_history.clone(),
                "switch",
                None,
                false,
                None,
            );
        }
    }

    loop {
        terminal.draw(|f| {
            draw_ui(f, &mut app);
        })?;

        while let Ok(event) = event_rx.try_recv() {
            match event {
                AppEvent::Key(key) => {
                    if key.code == KeyCode::Char('c') && key.modifiers.contains(event::KeyModifiers::CONTROL) {
                        disable_raw_mode()?;
                        execute!(
                            terminal.backend_mut(),
                            LeaveAlternateScreen,
                            DisableMouseCapture
                        )?;
                        terminal.show_cursor()?;
                        return Ok(());
                    }

                    if let InputMode::OAuthLogin { .. } = &app.input_mode {
                        if key.code == KeyCode::Esc {
                            app.input_mode = InputMode::Normal;
                            app.set_status("OAuth login session cancelled.");
                            app.is_loading = false;
                        }
                        continue;
                    }

                    if app.is_searching {
                        match key.code {
                            KeyCode::Esc => {
                                app.is_searching = false;
                                app.search_query.clear();
                                app.list_state.select(Some(0));
                                app.set_status("Filter cleared.");
                            }
                            KeyCode::Enter => {
                                app.is_searching = false;
                                app.set_status(&format!("Locked filter: {}", app.search_query));
                            }
                            KeyCode::Backspace => {
                                app.search_query.pop();
                                app.list_state.select(Some(0));
                            }
                            KeyCode::Char(c) => {
                                app.search_query.push(c);
                                app.list_state.select(Some(0));
                            }
                            _ => {}
                        }
                        continue;
                    }

                    if let InputMode::ConfirmDelete { email } = &app.input_mode {
                        match key.code {
                            KeyCode::Char('y') | KeyCode::Char('Y') => {
                                let email_clone = email.clone();
                                let res = delete_account_from_db(&app.db_path, &email_clone);
                                app.input_mode = InputMode::Normal;
                                match res {
                                    Ok(_) => {
                                        app.accounts.retain(|a| a.email != email_clone);
                                        app.cli_cache.tokens.remove(&email_clone);
                                        app.cli_cache.quotas.remove(&email_clone);
                                        
                                        if app.active_email.as_ref() == Some(&email_clone) {
                                            if !app.accounts.is_empty() {
                                                app.active_email = Some(app.accounts[0].email.clone());
                                                app.cli_cache.active_email = Some(app.accounts[0].email.clone());
                                            } else {
                                                app.active_email = None;
                                                app.cli_cache.active_email = None;
                                            }
                                            let _ = save_cli_cache(&app.cli_cache);
                                        }
                                        
                                        app.list_state.select(Some(0));
                                        app.set_status(&format!("✓ Account {} deleted successfully.", email_clone));
                                    }
                                    Err(e) => {
                                        app.set_status(&format!("✗ Delete failed: {}", e));
                                    }
                                }
                            }
                            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc | KeyCode::Char('q') => {
                                app.input_mode = InputMode::Normal;
                                app.set_status("Delete cancelled.");
                            }
                            _ => {}
                        }
                        continue;
                    }

                    let mut add_account_action = None;
                    if let InputMode::AddAccount { .. } = &app.input_mode {
                        match key.code {
                            KeyCode::Esc => {
                                add_account_action = Some(AddAccountAction::Cancel);
                            }
                            KeyCode::Tab => {
                                add_account_action = Some(AddAccountAction::CycleField);
                            }
                            KeyCode::Char(c) => {
                                add_account_action = Some(AddAccountAction::InputChar(c));
                            }
                            KeyCode::Backspace => {
                                add_account_action = Some(AddAccountAction::Backspace);
                            }
                            KeyCode::Enter => {
                                add_account_action = Some(AddAccountAction::Submit);
                            }
                            _ => {}
                        }
                    }

                    if let Some(action) = add_account_action {
                        let mut submit_data = None;
                        if let InputMode::AddAccount { email, refresh_token, active_field, error_message } = &mut app.input_mode {
                            match action {
                                AddAccountAction::Cancel => {
                                    app.input_mode = InputMode::Normal;
                                    app.set_status("Add account cancelled.");
                                }
                                AddAccountAction::CycleField => {
                                    *active_field = if *active_field == 0 { 1 } else { 0 };
                                }
                                AddAccountAction::InputChar(c) => {
                                    if *active_field == 0 {
                                        email.push(c);
                                    } else {
                                        refresh_token.push(c);
                                    }
                                }
                                AddAccountAction::Backspace => {
                                    if *active_field == 0 {
                                        email.pop();
                                    } else {
                                        refresh_token.pop();
                                    }
                                }
                                AddAccountAction::Submit => {
                                    if email.trim().is_empty() || refresh_token.trim().is_empty() {
                                        *error_message = Some("Both Email and Refresh Token are required.".to_string());
                                    } else if !email.contains('@') {
                                        *error_message = Some("Please enter a valid email address.".to_string());
                                    } else {
                                        submit_data = Some((email.clone(), refresh_token.clone()));
                                    }
                                }
                            }
                        }

                        if let Some((email, refresh_token)) = submit_data {
                            app.is_loading = true;
                            app.set_status(&format!("Initializing validation check for {}...", email));
                            spawn_network_task(
                                event_tx.clone(),
                                None,
                                Vec::new(),
                                app.cli_cache.clone(),
                                app.warmup_history.clone(),
                                "add_account",
                                None,
                                false,
                                Some((email, refresh_token, app.db_path.clone())),
                            );
                        }
                        continue;
                    }
                    if app.show_sort_menu {
                        match key.code {
                            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('s') | KeyCode::Char('S') => {
                                app.show_sort_menu = false;
                                app.set_status("Sort menu closed.");
                            }
                            KeyCode::Enter => {
                                let selected_idx = app.sort_menu_state.selected().unwrap_or(0);
                                if selected_idx < crate::tui::ui::SORT_OPTIONS.len() {
                                    let (_, mode, desc) = crate::tui::ui::SORT_OPTIONS[selected_idx];
                                    app.sort_mode = mode;
                                    app.sort_desc = desc;
                                    app.sort_accounts();
                                    let dir_str = if desc { "descending" } else { "ascending" };
                                    app.set_status(&format!("Sorted accounts by: {} ({})", mode.to_str(), dir_str));
                                }
                                app.show_sort_menu = false;
                            }
                            KeyCode::Down | KeyCode::Char('j') => {
                                let selected = app.sort_menu_state.selected().unwrap_or(0);
                                let next = if selected >= crate::tui::ui::SORT_OPTIONS.len() - 1 { 0 } else { selected + 1 };
                                app.sort_menu_state.select(Some(next));
                            }
                            KeyCode::Up | KeyCode::Char('k') => {
                                let selected = app.sort_menu_state.selected().unwrap_or(0);
                                let prev = if selected == 0 { crate::tui::ui::SORT_OPTIONS.len() - 1 } else { selected - 1 };
                                app.sort_menu_state.select(Some(prev));
                            }
                            KeyCode::Char('1') => {
                                app.sort_mode = SortMode::Email;
                                app.sort_desc = false;
                                app.sort_accounts();
                                app.show_sort_menu = false;
                                app.set_status("Sorted accounts by: Email (ascending)");
                            }
                            KeyCode::Char('2') => {
                                app.sort_mode = SortMode::Email;
                                app.sort_desc = true;
                                app.sort_accounts();
                                app.show_sort_menu = false;
                                app.set_status("Sorted accounts by: Email (descending)");
                            }
                            KeyCode::Char('3') => {
                                app.sort_mode = SortMode::Health;
                                app.sort_desc = false;
                                app.sort_accounts();
                                app.show_sort_menu = false;
                                app.set_status("Sorted accounts by: Health (ascending)");
                            }
                            KeyCode::Char('4') => {
                                app.sort_mode = SortMode::Health;
                                app.sort_desc = true;
                                app.sort_accounts();
                                app.show_sort_menu = false;
                                app.set_status("Sorted accounts by: Health (descending)");
                            }
                            KeyCode::Char('5') => {
                                app.sort_mode = SortMode::Gemini5h;
                                app.sort_desc = false;
                                app.sort_accounts();
                                app.show_sort_menu = false;
                                app.set_status("Sorted accounts by: Gemini 5h (ascending)");
                            }
                            KeyCode::Char('6') => {
                                app.sort_mode = SortMode::Gemini5h;
                                app.sort_desc = true;
                                app.sort_accounts();
                                app.show_sort_menu = false;
                                app.set_status("Sorted accounts by: Gemini 5h (descending)");
                            }
                            KeyCode::Char('7') => {
                                app.sort_mode = SortMode::GeminiWeekly;
                                app.sort_desc = false;
                                app.sort_accounts();
                                app.show_sort_menu = false;
                                app.set_status("Sorted accounts by: Gemini Weekly (ascending)");
                            }
                            KeyCode::Char('8') => {
                                app.sort_mode = SortMode::GeminiWeekly;
                                app.sort_desc = true;
                                app.sort_accounts();
                                app.show_sort_menu = false;
                                app.set_status("Sorted accounts by: Gemini Weekly (descending)");
                            }
                            KeyCode::Char('9') => {
                                app.sort_mode = SortMode::Claude5h;
                                app.sort_desc = false;
                                app.sort_accounts();
                                app.show_sort_menu = false;
                                app.set_status("Sorted accounts by: Claude 5h (ascending)");
                            }
                            KeyCode::Char('0') => {
                                app.sort_mode = SortMode::Claude5h;
                                app.sort_desc = true;
                                app.sort_accounts();
                                app.show_sort_menu = false;
                                app.set_status("Sorted accounts by: Claude 5h (descending)");
                            }
                            KeyCode::Char('-') => {
                                app.sort_mode = SortMode::ClaudeWeekly;
                                app.sort_desc = false;
                                app.sort_accounts();
                                app.show_sort_menu = false;
                                app.set_status("Sorted accounts by: Claude Weekly (ascending)");
                            }
                            KeyCode::Char('=') => {
                                app.sort_mode = SortMode::ClaudeWeekly;
                                app.sort_desc = true;
                                app.sort_accounts();
                                app.show_sort_menu = false;
                                app.set_status("Sorted accounts by: Claude Weekly (descending)");
                            }
                            _ => {}
                        }
                        continue;
                    }

                    if app.show_theme_selector {
                        match key.code {
                            KeyCode::Esc | KeyCode::Char('t') | KeyCode::Char('T') => {
                                app.show_theme_selector = false;
                                app.theme_search_query.clear();
                                app.set_status("Theme selector closed.");
                            }
                            KeyCode::Enter => {
                                let visible = app.get_visible_themes();
                                if let Some(idx) = app.theme_list_state.selected() {
                                    if let Some(&selected_theme) = visible.get(idx) {
                                        app.theme = selected_theme;
                                        app.cli_cache.theme = Some(selected_theme.to_str().to_string());
                                        let _ = save_cli_cache(&app.cli_cache);
                                        app.show_theme_selector = false;
                                        app.theme_search_query.clear();
                                        app.set_status(&format!("Successfully switched theme to: {}", selected_theme.to_str()));
                                    }
                                }
                            }
                            KeyCode::Backspace => {
                                app.theme_search_query.pop();
                                app.theme_list_state.select(Some(0));
                            }
                            KeyCode::Down => {
                                let visible = app.get_visible_themes();
                                if !visible.is_empty() {
                                    let i = match app.theme_list_state.selected() {
                                        Some(i) => {
                                            if i >= visible.len() - 1 {
                                                0
                                            } else {
                                                i + 1
                                            }
                                        }
                                        None => 0,
                                    };
                                    app.theme_list_state.select(Some(i));
                                }
                            }
                            KeyCode::Up => {
                                let visible = app.get_visible_themes();
                                if !visible.is_empty() {
                                    let i = match app.theme_list_state.selected() {
                                        Some(i) => {
                                            if i == 0 {
                                                visible.len() - 1
                                            } else {
                                                i - 1
                                            }
                                        }
                                        None => 0,
                                    };
                                    app.theme_list_state.select(Some(i));
                                }
                            }
                            KeyCode::Char('j') => {
                                let visible = app.get_visible_themes();
                                if !visible.is_empty() {
                                    let i = match app.theme_list_state.selected() {
                                        Some(i) => {
                                            if i >= visible.len() - 1 {
                                                0
                                            } else {
                                                i + 1
                                            }
                                        }
                                        None => 0,
                                    };
                                    app.theme_list_state.select(Some(i));
                                }
                            }
                            KeyCode::Char('k') => {
                                let visible = app.get_visible_themes();
                                if !visible.is_empty() {
                                    let i = match app.theme_list_state.selected() {
                                        Some(i) => {
                                            if i == 0 {
                                                visible.len() - 1
                                            } else {
                                                i - 1
                                            }
                                        }
                                        None => 0,
                                    };
                                    app.theme_list_state.select(Some(i));
                                }
                            }
                            KeyCode::Char('q') => {
                                if app.theme_search_query.is_empty() {
                                    app.show_theme_selector = false;
                                    app.set_status("Theme selector closed.");
                                } else {
                                    app.theme_search_query.push('q');
                                    app.theme_list_state.select(Some(0));
                                }
                            }
                            KeyCode::Char(c) => {
                                app.theme_search_query.push(c);
                                app.theme_list_state.select(Some(0));
                            }
                            _ => {}
                        }
                        continue;
                    }

                    if app.show_logs {
                        if app.is_searching_logs {
                            match key.code {
                                KeyCode::Esc => {
                                    app.is_searching_logs = false;
                                    app.log_search_query.clear();
                                    app.log_state.select(Some(0));
                                    app.set_status("Logs filter cleared.");
                                }
                                KeyCode::Enter => {
                                    app.is_searching_logs = false;
                                    app.set_status(&format!("Locked logs filter: {}", app.log_search_query));
                                }
                                KeyCode::Backspace => {
                                    app.log_search_query.pop();
                                    app.log_state.select(Some(0));
                                }
                                KeyCode::Char(c) => {
                                    app.log_search_query.push(c);
                                    app.log_state.select(Some(0));
                                }
                                _ => {}
                            }
                        } else {
                            match key.code {
                                KeyCode::Char('v') | KeyCode::Char('V') | KeyCode::Char('q') | KeyCode::Esc => {
                                    app.show_logs = false;
                                    app.log_search_query.clear();
                                    app.is_searching_logs = false;
                                    app.set_status("Closed session logs explorer.");
                                }
                                KeyCode::Char('/') => {
                                    app.is_searching_logs = true;
                                    app.log_search_query.clear();
                                    app.log_state.select(Some(0));
                                    app.set_status("Log Filter: Type query. Press Enter to lock, Esc to clear.");
                                }
                                KeyCode::Down | KeyCode::Char('j') => {
                                    let total_len = app.log_history.iter().filter(|log| {
                                        if app.log_search_query.is_empty() {
                                            true
                                        } else {
                                            log.to_lowercase().contains(&app.log_search_query.to_lowercase())
                                        }
                                    }).count();

                                    if total_len > 0 {
                                        let i = match app.log_state.selected() {
                                            Some(i) => {
                                                if i >= total_len - 1 {
                                                    0
                                                } else {
                                                    i + 1
                                                }
                                            }
                                            None => 0,
                                        };
                                        app.log_state.select(Some(i));
                                    }
                                }
                                KeyCode::Up | KeyCode::Char('k') => {
                                    let total_len = app.log_history.iter().filter(|log| {
                                        if app.log_search_query.is_empty() {
                                            true
                                        } else {
                                            log.to_lowercase().contains(&app.log_search_query.to_lowercase())
                                        }
                                    }).count();

                                    if total_len > 0 {
                                        let i = match app.log_state.selected() {
                                            Some(i) => {
                                                if i == 0 {
                                                    total_len - 1
                                                } else {
                                                    i - 1
                                                }
                                            }
                                            None => 0,
                                        };
                                        app.log_state.select(Some(i));
                                    }
                                }
                                _ => {}
                            }
                        }
                        continue;
                    }

                    if app.show_help {
                        match key.code {
                            KeyCode::Char('h') | KeyCode::Char('q') | KeyCode::Esc => {
                                app.show_help = false;
                            }
                            _ => {}
                        }
                        continue;
                    }

                    match key.code {

                        KeyCode::Char('h') => {
                            if !app.is_loading {
                                app.show_help = true;
                            }
                        }
                        KeyCode::Char('c') | KeyCode::Char('C') => {
                            if !app.is_loading {
                                app.compact_mode = !app.compact_mode;
                                let status = if app.compact_mode { "Compact Layout Mode enabled." } else { "Full Layout Mode enabled." };
                                app.set_status(status);
                            }
                        }
                        KeyCode::Char('p') | KeyCode::Char('P') => {
                            if !app.is_loading {
                                app.privacy_mode = !app.privacy_mode;
                                let status = if app.privacy_mode { "Privacy Mode enabled (emails masked)." } else { "Privacy Mode disabled (emails revealed)." };
                                app.set_status(status);
                            }
                        }
                        KeyCode::Char('v') | KeyCode::Char('V') => {
                            if !app.is_loading {
                                app.show_logs = true;
                                if !app.log_history.is_empty() {
                                    app.log_state.select(Some(app.log_history.len() - 1));
                                }
                                app.set_status("Viewing complete session logs history.");
                            }
                        }
                        KeyCode::Char('s') | KeyCode::Char('S') => {
                            if !app.is_loading {
                                app.show_sort_menu = true;
                                app.sort_menu_state.select(Some(0));
                                app.set_status("Open Sort Mode Selector. Use j/k to navigate or 1-0 hotkeys.");
                            }
                        }
                        KeyCode::Char('t') | KeyCode::Char('T') => {
                            if !app.is_loading {
                                app.show_theme_selector = true;
                                app.theme_search_query.clear();
                                app.theme_list_state.select(Some(0));
                                app.set_status("Open Color Theme Selector. Use up/down arrow keys or type search query.");
                            }
                        }
                        KeyCode::Char('b') | KeyCode::Char('B') => {
                            if !app.is_loading {
                                match app.backup_db() {
                                    Ok(path) => {
                                        let name = Path::new(&path).file_name().unwrap_or_default().to_string_lossy();
                                        app.set_status(&format!("✓ Backup saved: {}", name));
                                    }
                                    Err(e) => {
                                        app.set_status(&format!("✗ Backup failed: {}", e));
                                    }
                                }
                            }
                        }
                        KeyCode::Char('q') | KeyCode::Esc => {
                            disable_raw_mode()?;
                            execute!(
                                terminal.backend_mut(),
                                LeaveAlternateScreen,
                                DisableMouseCapture
                            )?;
                            terminal.show_cursor()?;
                            return Ok(());
                        }
                        KeyCode::Char(c) if c.is_digit(10) && c != '0' => {
                            if !app.is_loading {
                                let digit = c.to_digit(10).unwrap() as usize;
                                let idx = digit - 1;
                                let visible = app.get_visible_accounts();
                                if idx < visible.len() {
                                    app.list_state.select(Some(idx));
                                    if let Some(acc) = app.get_selected_account().cloned() {
                                        app.is_loading = true;
                                        app.set_status(&format!("Instantly switching to {}...", acc.email));
                                        spawn_network_task(
                                            event_tx.clone(),
                                            Some(acc),
                                            Vec::new(),
                                            app.cli_cache.clone(),
                                            app.warmup_history.clone(),
                                            "switch",
                                            None,
                                            false,
                                            None,
                                        );
                                    }
                                }
                            }
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            if !app.is_loading {
                                app.select_next();
                            }
                        }
                        KeyCode::Up | KeyCode::Char('k') => {
                            if !app.is_loading {
                                app.select_prev();
                            }
                        }
                        KeyCode::Enter => {
                            if !app.is_loading {
                                if let Some(acc) = app.get_selected_account().cloned() {
                                    app.is_loading = true;
                                    app.set_status(&format!("Activating and writing keyring credentials for {}...", acc.email));
                                    spawn_network_task(
                                        event_tx.clone(),
                                        Some(acc),
                                        Vec::new(),
                                        app.cli_cache.clone(),
                                        app.warmup_history.clone(),
                                        "switch",
                                        None,
                                        false,
                                        None,
                                    );
                                }
                            }
                        }
                        KeyCode::Char('r') => {
                            if !app.is_loading {
                                if let Some(acc) = app.get_selected_account().cloned() {
                                    app.is_loading = true;
                                    app.set_status(&format!("Refreshing quota statistics for {}...", acc.email));
                                    spawn_network_task(
                                        event_tx.clone(),
                                        Some(acc),
                                        Vec::new(),
                                        app.cli_cache.clone(),
                                        app.warmup_history.clone(),
                                        "quota",
                                        None,
                                        false,
                                        None,
                                    );
                                }
                            }
                        }
                        KeyCode::Char('R') => {
                            if !app.is_loading {
                                app.is_loading = true;
                                app.last_auto_refresh = Some(Instant::now());
                                app.set_status("Initializing non-blocking Quotas Reload for ALL accounts...");
                                spawn_network_task(
                                    event_tx.clone(),
                                    None,
                                    app.accounts.clone(),
                                    app.cli_cache.clone(),
                                    app.warmup_history.clone(),
                                    "quota_all",
                                    None,
                                    false,
                                    None,
                                );
                            }
                        }
                        KeyCode::Char('w') => {
                            if !app.is_loading {
                                if let Some(acc) = app.get_selected_account().cloned() {
                                    app.is_loading = true;
                                    app.set_status(&format!("Triggering smart warm up sequence for {}...", acc.email));
                                    spawn_network_task(
                                        event_tx.clone(),
                                        Some(acc),
                                        Vec::new(),
                                        app.cli_cache.clone(),
                                        app.warmup_history.clone(),
                                        "warmup",
                                        None,
                                        false,
                                        None,
                                    );
                                }
                            }
                        }
                        KeyCode::Char('W') => {
                            if !app.is_loading {
                                app.is_loading = true;
                                app.set_status("Initializing Smart Warm Up cycle for ALL accounts...");
                                spawn_network_task(
                                    event_tx.clone(),
                                    None,
                                    app.accounts.clone(),
                                    app.cli_cache.clone(),
                                    app.warmup_history.clone(),
                                    "warmup_all",
                                    None,
                                    false,
                                    None,
                                );
                            }
                        }
                        KeyCode::Char('f') => {
                            if !app.is_loading {
                                if let Some(acc) = app.get_selected_account().cloned() {
                                    app.is_loading = true;
                                    app.set_status(&format!("FORCE warming up all models for {} (ignoring cooldown)...", acc.email));
                                    spawn_network_task(
                                        event_tx.clone(),
                                        Some(acc),
                                        Vec::new(),
                                        app.cli_cache.clone(),
                                        app.warmup_history.clone(),
                                        "warmup",
                                        None,
                                        true,
                                        None,
                                    );
                                }
                            }
                        }
                        KeyCode::Char('a') => {
                            if !app.is_loading {
                                app.input_mode = InputMode::AddAccount {
                                    email: String::new(),
                                    refresh_token: String::new(),
                                    active_field: 0,
                                    error_message: None,
                                };
                            }
                        }
                        KeyCode::Char('l') => {
                            if !app.is_loading {
                                let auth_url = format!(
                                    "https://accounts.google.com/o/oauth2/v2/auth?client_id={}&redirect_uri=http://localhost:{}&response_type=code&scope=openid%20https://www.googleapis.com/auth/cloud-platform%20https://www.googleapis.com/auth/userinfo.email%20https://www.googleapis.com/auth/userinfo.profile%20https://www.googleapis.com/auth/cclog%20https://www.googleapis.com/auth/experimentsandconfigs&access_type=offline&prompt=consent",
                                    types::CLIENT_ID, types::OAUTH_PORT
                                );
                                
                                let url_clone = auth_url.clone();
                                tokio::spawn(async move {
                                    let _ = keyring::open_browser(&url_clone);
                                });
                                
                                app.is_loading = true;
                                app.input_mode = InputMode::OAuthLogin { auth_url };
                                app.set_status("Starting Google OAuth loopback session on port 14210. Check browser...");
                                
                                spawn_network_task(
                                    event_tx.clone(),
                                    None,
                                    Vec::new(),
                                    app.cli_cache.clone(),
                                    app.warmup_history.clone(),
                                    "oauth_login",
                                    None,
                                    false,
                                    Some((String::new(), String::new(), app.db_path.clone())),
                                );
                            }
                        }
                        KeyCode::Char('d') | KeyCode::Backspace => {
                            if !app.is_loading {
                                if let Some(acc) = app.get_selected_account() {
                                    app.input_mode = InputMode::ConfirmDelete {
                                        email: acc.email.clone(),
                                    };
                                }
                            }
                        }
                        KeyCode::Char('/') => {
                            if !app.is_loading {
                                app.is_searching = true;
                                app.search_query.clear();
                                app.set_status("Filter: Type query. Press Enter to lock, Esc to clear.");
                            }
                        }
                        _ => {}
                    }
                }
                AppEvent::Mouse(mouse) => {
                    if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                        let size: ratatui::layout::Rect = terminal.size().unwrap_or_default().into();
                        let modal_active = app.show_theme_selector 
                            || app.show_help 
                            || app.show_logs 
                            || !matches!(app.input_mode, InputMode::Normal);
                        
                        if !modal_active {
                            let chunks = Layout::default()
                                .direction(Direction::Vertical)
                                .constraints([
                                    Constraint::Length(3),
                                    Constraint::Min(10),
                                    Constraint::Length(3),
                                    Constraint::Length(1),
                                ])
                                .split(size);

                            let content_chunks = Layout::default()
                                .direction(Direction::Horizontal)
                                .constraints([
                                    Constraint::Percentage(60),
                                    Constraint::Percentage(40),
                                ])
                                .split(chunks[1]);

                            let table_area = content_chunks[0];
                            
                            if mouse.column >= table_area.x && mouse.column < table_area.x + table_area.width {
                                if mouse.row == table_area.y + 1 {
                                    if let Some(col_idx) = app.get_column_index(mouse.column, table_area) {
                                        if !app.is_loading {
                                            match col_idx {
                                                0 | 1 => {
                                                    if app.sort_mode == SortMode::Email {
                                                        app.sort_desc = !app.sort_desc;
                                                    } else {
                                                        app.sort_mode = SortMode::Email;
                                                        app.sort_desc = false;
                                                    }
                                                }
                                                2 => {
                                                    if app.sort_mode == SortMode::Gemini5h {
                                                        app.sort_desc = !app.sort_desc;
                                                    } else {
                                                        app.sort_mode = SortMode::Gemini5h;
                                                        app.sort_desc = false;
                                                    }
                                                }
                                                3 => {
                                                    if app.sort_mode == SortMode::GeminiWeekly {
                                                        app.sort_desc = !app.sort_desc;
                                                    } else {
                                                        app.sort_mode = SortMode::GeminiWeekly;
                                                        app.sort_desc = false;
                                                    }
                                                }
                                                4 => {
                                                    if app.sort_mode == SortMode::Claude5h {
                                                        app.sort_desc = !app.sort_desc;
                                                    } else {
                                                        app.sort_mode = SortMode::Claude5h;
                                                        app.sort_desc = false;
                                                    }
                                                }
                                                5 => {
                                                    if app.sort_mode == SortMode::ClaudeWeekly {
                                                        app.sort_desc = !app.sort_desc;
                                                    } else {
                                                        app.sort_mode = SortMode::ClaudeWeekly;
                                                        app.sort_desc = false;
                                                    }
                                                }
                                                _ => {}
                                            }
                                            app.sort_accounts();
                                            let dir_str = if app.sort_desc { "descending" } else { "ascending" };
                                            app.set_status(&format!("Sorted accounts by: {} ({})", app.sort_mode.to_str(), dir_str));
                                        }
                                    }
                                } else if mouse.row >= table_area.y + 3 {
                                    let col_idx = app.get_column_index(mouse.column, table_area);
                                    let rel_row = (mouse.row - (table_area.y + 3)) as usize;
                                    let clicked_idx = app.list_state.offset() + (rel_row / 3);
                                    let within_row_text = (rel_row % 3) < 2; // only row lines 0 and 1 have text, line 2 is margin spacing!
                                    
                                    let clicked_account = if within_row_text {
                                        let visible = app.get_visible_accounts();
                                        if clicked_idx < visible.len() {
                                            Some((*visible[clicked_idx]).clone())
                                        } else {
                                            None
                                        }
                                    } else {
                                        None
                                    };
                                    
                                    if let Some(acc) = clicked_account {
                                        // Log click details for debugging coordinate alignment
                                        app.set_status(&format!(
                                            "Click: row={}, col={}, col_idx={:?}, resolved_idx={}, email={}",
                                            mouse.row, mouse.column, col_idx, clicked_idx, acc.email
                                        ));
                                        
                                        // Only select/switch if clicking Col 0 (Active mark) or Col 1 (Email address)
                                        if let Some(c_idx) = col_idx {
                                            if c_idx == 0 || c_idx == 1 {
                                                if !app.is_loading {
                                                    // Select and focus the clicked account
                                                    app.list_state.select(Some(clicked_idx));
                                                    app.focused_panel = Focus::Accounts;
                                                    
                                                    // Instantly switch and activate the session
                                                    let is_currently_active = app.active_email.as_ref() == Some(&acc.email);
                                                    if is_currently_active {
                                                        app.set_status(&format!("Session is already active for {}.", acc.email));
                                                    } else {
                                                        app.is_loading = true;
                                                        app.set_status(&format!("Activating and writing keyring credentials for {}...", acc.email));
                                                        spawn_network_task(
                                                            event_tx.clone(),
                                                            Some(acc),
                                                            Vec::new(),
                                                            app.cli_cache.clone(),
                                                            app.warmup_history.clone(),
                                                            "switch",
                                                            None,
                                                            false,
                                                            None,
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                AppEvent::Progress(msg) => {
                    app.set_status(&msg);
                }
                AppEvent::NetworkSuccess(result) => {
                    app.is_loading = false;
                    app.cli_cache = config::load_cli_cache();
                    match result {
                        NetworkResult::AddAccountComplete { new_account } => {
                            app.input_mode = InputMode::Normal;
                            
                            let (reload_accs, _, _) = load_accounts_list();
                            app.accounts = reload_accs;
                            app.sort_accounts();
                            
                            if let Some(pos) = app.accounts.iter().position(|a| a.email == new_account.email) {
                                app.list_state.select(Some(pos));
                            }
                            
                            app.set_status(&format!("✓ Account {} successfully validated and added to database!", new_account.email));
                        }
                        NetworkResult::SwitchComplete { email, keyring_success } => {
                            app.active_email = Some(email.clone());
                            app.cli_cache.active_email = Some(email.clone());
                            save_cli_cache(&app.cli_cache);
                            
                            if let Some(acc) = app.accounts.iter().find(|a| a.email == email) {
                                if let Some(ref acc_id) = acc.id {
                                    let data_dir = get_data_dir();
                                    let index_path = data_dir.join("accounts.json");
                                    if index_path.exists() {
                                        if let Ok(content) = fs::read_to_string(&index_path) {
                                            let cleaned = content.replace("\u{feff}", "").replace('\x00', "");
                                            if let Ok(mut val) = serde_json::from_str::<serde_json::Value>(&cleaned) {
                                                if let Some(obj) = val.as_object_mut() {
                                                    obj.insert("current_account_id".to_string(), json!(acc_id));
                                                    obj.insert("current_target_ide".to_string(), json!("agy"));
                                                    if let Ok(new_content) = serde_json::to_string_pretty(&val) {
                                                        let _ = fs::write(&index_path, new_content);
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            
                            if keyring_success {
                                app.set_status(&format!("Account changed to {}. Keyring credentials written successfully.", email));
                            } else {
                                app.set_status(&format!("Account changed to {} (keyring write failed, fallback active).", email));
                            }
                        }
                        NetworkResult::QuotaRefreshed { email, quota, project_id } => {
                            if let Some(pid) = project_id {
                                if let Some(tc) = app.cli_cache.tokens.get_mut(&email) {
                                    tc.project_id = Some(pid);
                                }
                            }
                            if !quota.models.is_empty() {
                                app.cli_cache.quotas.insert(email.clone(), quota);
                            } else if let Some(tc) = app.cli_cache.tokens.get(&email) {
                                if let Some(q_entry) = app.cli_cache.quotas.get_mut(&email) {
                                    q_entry.subscription_tier = tc.subscription_tier.clone();
                                }
                            }
                            save_cli_cache(&app.cli_cache);
                            app.sort_accounts();
                            
                            if app.status_message.starts_with("Ready") || app.status_message.contains("Reloading") {
                                app.set_status(&format!("Quota statistics refreshed for {}.", email));
                            }
                        }
                        NetworkResult::WarmupComplete { email, warmup_count, skipped_count, logs } => {
                            app.warmup_history = load_warmup_history();
                            let summary = format!(
                                "Warmup completed for {}: triggered {}, skipped {}.",
                                email, warmup_count, skipped_count
                            );
                            app.set_status(&summary);
                            
                            for log in logs {
                                app.set_status(&log);
                            }
                            
                            if warmup_count > 0 && email != "All Accounts" {
                                if let Some(acc) = app.accounts.iter().find(|a| a.email == email).cloned() {
                                    app.is_loading = true;
                                    spawn_network_task(
                                        event_tx.clone(),
                                        Some(acc),
                                        Vec::new(),
                                        app.cli_cache.clone(),
                                        app.warmup_history.clone(),
                                        "quota",
                                        None,
                                        false,
                                        None,
                                    );
                                }
                            }
                        }
                    }
                }
                AppEvent::NetworkError(err) => {
                    app.is_loading = false;
                    app.cli_cache = load_cli_cache();
                    app.set_status(&err);
                    
                    if let InputMode::AddAccount { email, refresh_token, active_field, .. } = &app.input_mode {
                        app.input_mode = InputMode::AddAccount {
                            email: email.clone(),
                            refresh_token: refresh_token.clone(),
                            active_field: *active_field,
                            error_message: Some(err.clone()),
                        };
                    } else if let InputMode::OAuthLogin { .. } = &app.input_mode {
                        app.input_mode = InputMode::Normal;
                    }
                }
                AppEvent::Tick => {
                    app.tick_count += 1;
                    app.update_status_decay();
                    
                    if let Some(last) = app.last_auto_refresh {
                        if last.elapsed() >= Duration::from_secs(300) {
                            app.last_auto_refresh = Some(Instant::now());
                            if !app.is_loading && !app.accounts.is_empty() {
                                app.is_loading = true;
                                app.set_status("Auto-refreshing quotas for all accounts (5 min interval)...");
                                spawn_network_task(
                                    event_tx.clone(),
                                    None,
                                    app.accounts.clone(),
                                    app.cli_cache.clone(),
                                    app.warmup_history.clone(),
                                    "quota_all",
                                    None,
                                    false,
                                    None,
                                );
                            }
                        }
                    } else {
                        app.last_auto_refresh = Some(Instant::now());
                    }
                }
            }
        }
    }
}
