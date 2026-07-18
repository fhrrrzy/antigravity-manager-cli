use ratatui::{
    Frame,
    layout::{Layout, Direction, Constraint, Rect},
    style::{Style, Modifier, Color},
    text::{Line, Span},
    widgets::{
        Block, Borders, BorderType, Paragraph, Clear, Wrap,
        Table, TableState, Row, Cell, Scrollbar, ScrollbarOrientation, ScrollbarState,
        List, ListItem
    }
};

use crate::types::{Focus, InputMode, QuotaData, COOLDOWN_SECONDS, SortMode};
use crate::tui::App;

pub const SORT_OPTIONS: &[(&str, SortMode, bool)] = &[
    ("Email (Ascending)", SortMode::Email, false),
    ("Email (Descending)", SortMode::Email, true),
    ("Gemini 5h (Ascending)", SortMode::Gemini5h, false),
    ("Gemini 5h (Descending)", SortMode::Gemini5h, true),
    ("Gemini Weekly (Ascending)", SortMode::GeminiWeekly, false),
    ("Gemini Weekly (Descending)", SortMode::GeminiWeekly, true),
    ("Claude 5h (Ascending)", SortMode::Claude5h, false),
    ("Claude 5h (Descending)", SortMode::Claude5h, true),
    ("Claude Weekly (Ascending)", SortMode::ClaudeWeekly, false),
    ("Claude Weekly (Descending)", SortMode::ClaudeWeekly, true),
];

fn format_countdown(reset_time: &str) -> Option<String> {
    let now = chrono::Utc::now();
    if let Ok(rt) = chrono::DateTime::parse_from_rfc3339(reset_time) {
        let diff = rt.with_timezone(&chrono::Utc) - now;
        if diff.num_seconds() > 0 {
            let h = diff.num_seconds() / 3600;
            let m = (diff.num_seconds() % 3600) / 60;
            let s = diff.num_seconds() % 60;
            return Some(format!("{:02}:{:02}:{:02}", h, m, s));
        }
    }
    None
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

pub fn draw_ui(f: &mut Frame, app: &mut App) {
    let palette = app.theme.get_palette();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header
            Constraint::Min(10),   // Content splits
            Constraint::Length(3), // Status logs
            Constraint::Length(1), // Footer/Keyboard tips
        ])
        .split(f.size());

    let local_time = chrono::Local::now().format("%H:%M:%S").to_string();
    let active_str = app.active_email.as_deref().unwrap_or("None");
    let title = Paragraph::new(format!(
        " Antigravity Manager TUI | Active: {} | db: {} | 🐉 {} | 🕒 {} | 🟢 Online ",
        active_str, app.db_desc, palette.name, local_time
    ))
    .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" System Control Dashboard ").style(Style::default().fg(palette.border_active)))
    .style(Style::default().fg(palette.fg).add_modifier(Modifier::BOLD));
    f.render_widget(title, chunks[0]);

    let content_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(60), // Left panel: Account list & Quota summary
            Constraint::Percentage(40), // Right panel: Details
        ])
        .split(chunks[1]);

    let col_email_text = if app.sort_mode == SortMode::Email {
        format!("Email {}", if app.sort_desc { "▼" } else { "▲" })
    } else {
        "Email".to_string()
    };
    let col_email_style = if app.sort_mode == SortMode::Email {
        Style::default().fg(palette.blue_reset_5h).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(palette.border_active).add_modifier(Modifier::BOLD)
    };

    let col_gemini5h_text = if app.sort_mode == SortMode::Gemini5h {
        format!("Gemini 5h {}", if app.sort_desc { "▼" } else { "▲" })
    } else {
        "Gemini 5h".to_string()
    };
    let col_gemini5h_style = if app.sort_mode == SortMode::Gemini5h {
        Style::default().fg(palette.blue_reset_5h).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(palette.border_active).add_modifier(Modifier::BOLD)
    };

    let col_geminiwk_text = if app.sort_mode == SortMode::GeminiWeekly {
        format!("Gemini Wk {}", if app.sort_desc { "▼" } else { "▲" })
    } else {
        "Gemini Wk".to_string()
    };
    let col_geminiwk_style = if app.sort_mode == SortMode::GeminiWeekly {
        Style::default().fg(palette.blue_reset_5h).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(palette.border_active).add_modifier(Modifier::BOLD)
    };

    let col_claude5h_text = if app.sort_mode == SortMode::Claude5h {
        format!("Claude 5h {}", if app.sort_desc { "▼" } else { "▲" })
    } else {
        "Claude 5h".to_string()
    };
    let col_claude5h_style = if app.sort_mode == SortMode::Claude5h {
        Style::default().fg(palette.blue_reset_5h).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(palette.border_active).add_modifier(Modifier::BOLD)
    };

    let col_claudewk_text = if app.sort_mode == SortMode::ClaudeWeekly {
        format!("Claude Wk {}", if app.sort_desc { "▼" } else { "▲" })
    } else {
        "Claude Wk".to_string()
    };
    let col_claudewk_style = if app.sort_mode == SortMode::ClaudeWeekly {
        Style::default().fg(palette.blue_reset_5h).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(palette.border_active).add_modifier(Modifier::BOLD)
    };

    let header_cells = vec![
        Cell::from("Active").style(Style::default().fg(palette.border_inactive).add_modifier(Modifier::BOLD)),
        Cell::from(col_email_text).style(col_email_style),
        Cell::from(col_gemini5h_text).style(col_gemini5h_style),
        Cell::from(col_geminiwk_text).style(col_geminiwk_style),
        Cell::from(col_claude5h_text).style(col_claude5h_style),
        Cell::from(col_claudewk_text).style(col_claudewk_style),
    ];
    let header = Row::new(header_cells)
        .style(Style::default().bg(palette.selection_bg))
        .height(1)
        .bottom_margin(1);

    let mut rows = Vec::new();
    for (idx, acc) in app.get_visible_accounts().iter().enumerate() {
        let is_active = app.active_email.as_ref() == Some(&acc.email);
        let active_mark = if is_active { "★" } else { " " };
        
        let quota_cache = app.cli_cache.quotas.get(&acc.email);
        
        let gemini_pct = quota_cache.and_then(|q| {
            q.models.iter()
                .find(|m| m.name.contains("gemini") || m.display_name.as_ref().map(|n| n.contains("Gemini")).unwrap_or(false))
                .map(|m| m.percentage)
        });
        
        let claude_pct = quota_cache.and_then(|q| {
            q.models.iter()
                .find(|m| m.name.contains("claude") || m.display_name.as_ref().map(|n| n.contains("Claude")).unwrap_or(false))
                .map(|m| m.percentage)
        });

        let get_weekly_pct = |quota_cache: Option<&QuotaData>, is_claude: bool| -> Option<i32> {
            let q = quota_cache?;
            let groups = q.quota_groups.as_ref()?;
            for group in groups {
                let gp_name = group.display_name.to_lowercase();
                let target_match = if is_claude {
                    gp_name.contains("claude") || gp_name.contains("anthropic")
                } else {
                    gp_name.contains("gemini") || gp_name.contains("google")
                };
                
                for bucket in &group.buckets {
                    let b_id = bucket.bucket_id.to_lowercase();
                    let b_disp = bucket.display_name.as_ref().map(|s| s.to_lowercase()).unwrap_or_default();
                    let is_weekly = bucket.window == "weekly" || b_id.contains("weekly") || b_disp.contains("weekly");
                    
                    let name_match = target_match 
                        || (is_claude && (b_id.contains("claude") || b_disp.contains("claude")))
                        || (!is_claude && (b_id.contains("gemini") || b_disp.contains("gemini")));
                        
                    if is_weekly && name_match {
                        return Some((bucket.remaining_fraction * 100.0).round() as i32);
                    }
                }
            }
            None
        };

        let gemini_wk_pct = get_weekly_pct(quota_cache, false);
        let claude_wk_pct = get_weekly_pct(quota_cache, true);

        let bar_width = 8;
        let make_bar = |pct_opt: Option<i32>| -> (String, Color) {
            match pct_opt {
                Some(pct) => {
                    let filled = ((pct as f64 / 100.0) * bar_width as f64).round() as usize;
                    let empty = bar_width - filled;
                    let bar_color = if pct >= 80 {
                        palette.green_success
                    } else if pct >= 30 {
                        palette.yellow_warning
                    } else {
                        palette.red_danger
                    };
                    (format!("{} {:>3}%", "█".repeat(filled) + &"░".repeat(empty), pct), bar_color)
                }
                None => ("N/A".to_string(), palette.border_inactive),
            }
        };

        let (gemini_5h_bar, gemini_5h_color) = make_bar(gemini_pct);
        let (gemini_wk_bar, gemini_wk_color) = make_bar(gemini_wk_pct);
        let (claude_5h_bar, claude_5h_color) = make_bar(claude_pct);
        let (claude_wk_bar, claude_wk_color) = make_bar(claude_wk_pct);

        let is_selected = app.list_state.selected() == Some(idx);
        let row_bg = if is_selected { palette.selection_bg } else { Color::Reset };

        let top_row_style = Style::default().bg(row_bg).fg(if is_active { palette.green_success } else { palette.fg });
        let top_cells = vec![
            Cell::from(active_mark).style(if is_active { Style::default().fg(palette.green_success) } else { Style::default() }),
            Cell::from(acc.email.clone()).style(if is_active { Style::default().add_modifier(Modifier::BOLD) } else { Style::default() }),
            Cell::from(gemini_5h_bar).style(Style::default().fg(gemini_5h_color)),
            Cell::from(gemini_wk_bar).style(Style::default().fg(gemini_wk_color)),
            Cell::from(claude_5h_bar).style(Style::default().fg(claude_5h_color)),
            Cell::from(claude_wk_bar).style(Style::default().fg(claude_wk_color)),
        ];
        rows.push(Row::new(top_cells).style(top_row_style).bottom_margin(1));
    }

    let widths: &[Constraint] = &[
        Constraint::Percentage(8),
        Constraint::Percentage(32),
        Constraint::Percentage(15),
        Constraint::Percentage(15),
        Constraint::Percentage(15),
        Constraint::Percentage(15),
    ];

    let table_border_color = if app.focused_panel == Focus::Accounts { palette.border_active } else { palette.border_inactive };
    let table_title = if app.is_searching {
        format!(" Accounts Summary (Sorted by: {}) | 🔍 Find: {}_ ", app.sort_mode.to_str(), app.search_query)
    } else if !app.search_query.is_empty() {
        format!(" Accounts Summary (Sorted by: {}) | 🔍 Filter: {} (Esc to Clear) ", app.sort_mode.to_str(), app.search_query)
    } else if app.focused_panel == Focus::Accounts {
        format!(" Accounts Summary (Active Panel - Sorted by: {}) | [/] Find ", app.sort_mode.to_str())
    } else {
        format!(" Accounts Summary (Sorted by: {}) | [/] Find ", app.sort_mode.to_str())
    };

    let account_table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(table_title).style(Style::default().fg(table_border_color)))
        .highlight_style(Style::default());

    let mut render_state = TableState::default();
    if let Some(selected_idx) = app.list_state.selected() {
        render_state.select(Some(selected_idx));
    }
    f.render_stateful_widget(account_table, content_chunks[0], &mut render_state);

    let total_rows = app.get_visible_accounts().len();
    let current_pos = app.list_state.selected().unwrap_or(0);
    let mut scrollbar_state = ScrollbarState::new(total_rows).position(current_pos);
    let scrollbar = Scrollbar::default()
        .orientation(ScrollbarOrientation::VerticalRight)
        .begin_symbol(Some("▲"))
        .end_symbol(Some("▼"));
    f.render_stateful_widget(scrollbar, content_chunks[0], &mut scrollbar_state);

    if let Some(selected_acc) = app.get_selected_account() {
        let email = &selected_acc.email;
        let token_cache = app.cli_cache.tokens.get(email);
        let quota_cache = app.cli_cache.quotas.get(email);
        
        let project_id = token_cache.and_then(|t| t.project_id.as_deref()).unwrap_or("N/A");
        let tier = quota_cache.and_then(|q| q.subscription_tier.as_deref()).unwrap_or(token_cache.and_then(|t| t.subscription_tier.as_deref()).unwrap_or("N/A"));

        let details_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(5), // Account profile info + status banner
                Constraint::Min(5),    // Quota models list
            ])
            .split(content_chunks[1]);

        let is_highlight_active = app.active_email.as_ref() == Some(email);
        let status_span = if is_highlight_active {
            Span::styled(" ★ ACTIVE SESSION ", Style::default().bg(palette.green_success).fg(palette.bg).add_modifier(Modifier::BOLD))
        } else {
            Span::styled(" ○ INACTIVE ", Style::default().fg(palette.border_inactive))
        };
        
        let header_text = vec![
            Line::from(vec![Span::raw(" Email: "), Span::styled(email, Style::default().add_modifier(Modifier::BOLD))]),
            Line::from(vec![Span::raw(" Subscription Tier: "), Span::styled(tier, Style::default().fg(palette.border_active))]),
            Line::from(vec![Span::raw(" Project ID: "), Span::styled(project_id, Style::default().fg(palette.yellow_warning))]),
            Line::from(vec![Span::raw(" Status: "), status_span]),
        ];
        
        let details_header = Paragraph::new(header_text)
            .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" Account Profile ").style(Style::default().fg(palette.border_inactive)));
        f.render_widget(details_header, details_chunks[0]);

        if app.is_loading {
            let loading_msg = Paragraph::new(
                "\n\n\n\n       ⏳  PROCESSING TRANSACTION...\n\n       Contacting Google Companion API and updating active session credentials.\n       Please wait, the interface will automatically refresh."
            )
            .alignment(ratatui::layout::Alignment::Center)
            .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" Pending Action ").style(Style::default().fg(palette.border_active)));
            f.render_widget(loading_msg, details_chunks[1]);
        } else if let Some(q) = quota_cache {
            let mut quota_items = Vec::new();
            
            if q.models.is_empty() {
                quota_items.push(ListItem::new("No model quota details cached. Press [r] to refresh quotas."));
            } else {
                let mut sorted_models = q.models.clone();
                sorted_models.sort_by(|a, b| {
                    let a_is_claude = a.name.contains("claude");
                    let b_is_claude = b.name.contains("claude");
                    match (a_is_claude, b_is_claude) {
                        (true, false) => std::cmp::Ordering::Greater,
                        (false, true) => std::cmp::Ordering::Less,
                        _ => a.name.cmp(&b.name),
                    }
                });

                for m in sorted_models {
                    let name = &m.name;
                    let display = m.display_name.as_deref().unwrap_or(name);
                    let pct = m.percentage;
                    
                    let bar_color = if pct >= 80 {
                        palette.green_success
                    } else if pct >= 30 {
                        palette.yellow_warning
                    } else {
                        palette.red_danger
                    };

                    let bar_width = 15;
                    let filled = ((pct as f64 / 100.0) * bar_width as f64).round() as usize;
                    let empty = bar_width - filled;
                    let bar_str = format!(
                        "[{}{}] {:>3}%",
                        "█".repeat(filled),
                        "░".repeat(empty),
                        pct
                    );

                    let history_key = format!("{}:{}:100", email, name);
                    let mut cooldown_str = String::new();
                    if let Some(&last_ts) = app.warmup_history.get(&history_key) {
                        let elapsed = chrono::Utc::now().timestamp() - last_ts;
                        if elapsed < COOLDOWN_SECONDS {
                            let rem = COOLDOWN_SECONDS - elapsed;
                            let h = rem / 3600;
                            let min = (rem % 3600) / 60;
                            cooldown_str = format!(" [Cooldown: {}h {}m]", h, min);
                        }
                    }

                    let mut reset_str = String::new();
                    if !m.reset_time.is_empty() {
                        if let Some(cd) = format_countdown(&m.reset_time) {
                            reset_str = format!(" [Reset in: {}]", cd);
                        }
                    }

                    quota_items.push(ListItem::new(Line::from(vec![
                        Span::styled(format!("{:<28}", display), Style::default().fg(palette.fg)),
                        Span::styled(bar_str, Style::default().fg(bar_color)),
                        Span::styled(cooldown_str, Style::default().fg(palette.border_inactive)),
                        Span::styled(reset_str, Style::default().fg(palette.blue_reset_5h)),
                    ])));
                }
            }

            let breakdown_border_color = if app.focused_panel == Focus::Breakdown { palette.border_active } else { palette.border_inactive };
            let breakdown_title = if app.focused_panel == Focus::Breakdown { " Quotas Breakdown (Active Panel) " } else { " Quotas Breakdown " };

            let total_quotas = quota_items.len();
            let quota_list = List::new(quota_items)
                .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(breakdown_title).style(Style::default().fg(breakdown_border_color)))
                .highlight_style(Style::default().bg(palette.selection_bg).add_modifier(Modifier::BOLD));
            f.render_stateful_widget(quota_list, details_chunks[1], &mut app.breakdown_state);

            let current_quota_pos = app.breakdown_state.selected().unwrap_or(0);
            let mut quota_scrollbar_state = ScrollbarState::new(total_quotas).position(current_quota_pos);
            let quota_scrollbar = Scrollbar::default()
                .orientation(ScrollbarOrientation::VerticalRight)
                .begin_symbol(Some("▲"))
                .end_symbol(Some("▼"));
            f.render_stateful_widget(quota_scrollbar, details_chunks[1], &mut quota_scrollbar_state);
        } else {
            let breakdown_border_color = if app.focused_panel == Focus::Breakdown { palette.border_active } else { palette.border_inactive };
            let breakdown_title = if app.focused_panel == Focus::Breakdown { " Quotas Breakdown (Active Panel) " } else { " Quotas Breakdown " };
            let empty_quota = Paragraph::new("\n No quota metrics cached in database. Press [r] to refresh active quotas.")
                .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(breakdown_title).style(Style::default().fg(breakdown_border_color)));
            f.render_widget(empty_quota, details_chunks[1]);
        }
    } else {
        let fallback = Paragraph::new("\n Please select or configure an account first.")
            .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" Profile Details ").style(Style::default().fg(palette.border_inactive)));
        f.render_widget(fallback, content_chunks[1]);
    }

    let loader_prefix = if app.is_loading {
        let spin_chars = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        let idx = (app.tick_count as usize) % spin_chars.len();
        format!("{} ", spin_chars[idx])
    } else {
        "".to_string()
    };
    let status_block = Paragraph::new(format!("{}{}", loader_prefix, app.status_message))
        .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" Logger Console ").style(Style::default().fg(palette.green_success)))
        .wrap(Wrap { trim: true });
    f.render_widget(status_block, chunks[2]);

    let footer = Paragraph::new(" [Enter] Switch | [r] Refresh | [w] Warm Up | [/] Find | [s] Sort | [c] Compact | [v] Logs | [t] Theme | [h] Help")
        .style(Style::default().fg(palette.border_inactive));
    f.render_widget(footer, chunks[3]);

    if app.show_help {
        let block = Block::default()
            .title(" Keyboard Help Guide ")
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .style(Style::default().bg(palette.bg).fg(palette.border_active));
        
        let area = centered_rect(65, 58, f.size());
        f.render_widget(Clear, area);
        f.render_widget(block, area);

        let help_text = vec![
            Line::from(vec![Span::styled("Navigation & Layout:", Style::default().fg(palette.yellow_warning).add_modifier(Modifier::BOLD))]),
            Line::from(vec![Span::raw("  Tab           Switch panel focus (Accounts Table <-> Quotas Breakdown)")]),
            Line::from(vec![Span::raw("  j / Down      Select next item in active panel")]),
            Line::from(vec![Span::raw("  k / Up        Select previous item in active panel")]),
            Line::from(vec![Span::raw("  s             Open keyboard-driven Sort Mode Selector menu")]),
            Line::from(vec![Span::raw("  /             Search / Filter accounts by typing email address")]),
            Line::from(vec![Span::raw("  c             Toggle Compact layout view (hides reset countdowns for tablet/portrait)")]),
            Line::from(vec![Span::raw("  v             Open scrollable Session Logs History Explorer overlay")]),
            Line::from(vec![Span::raw("  Enter         Activate/Switch session to selected account")]),
            Line::from(vec![Span::raw("")]),
            Line::from(vec![Span::styled("Quota & Session actions:", Style::default().fg(palette.yellow_warning).add_modifier(Modifier::BOLD))]),
            Line::from(vec![Span::raw("  r             Refresh selected account's Google API quotas")]),
            Line::from(vec![Span::raw("  R             Batch refresh ALL accounts' quotas (asynchronously)")]),
            Line::from(vec![Span::raw("  w             Trigger smart warm up sequence for selected account")]),
            Line::from(vec![Span::raw("  W             Trigger smart warm up sequence for ALL accounts")]),
            Line::from(vec![Span::raw("  f             Force warm up selected account (ignores cooldowns)")]),
            Line::from(vec![Span::raw("")]),
            Line::from(vec![Span::styled("Account Management:", Style::default().fg(palette.yellow_warning).add_modifier(Modifier::BOLD))]),
            Line::from(vec![Span::raw("  a             Add custom account with manual refresh token")]),
            Line::from(vec![Span::raw("  l             Login via Google OAuth browser integration link")]),
            Line::from(vec![Span::raw("  d / Backspace Open account deletion confirmation prompt")]),
            Line::from(vec![Span::raw("  b             Create a local database backup JSON snapshot")]),
            Line::from(vec![Span::raw("")]),
            Line::from(vec![Span::styled("Press [h], [Esc] or [q] to close this help guide", Style::default().fg(palette.green_success))]),
        ];

        let help_para = Paragraph::new(help_text)
            .wrap(Wrap { trim: true });
        
        let help_area = Layout::default()
            .margin(2)
            .constraints([Constraint::Percentage(100)])
            .split(area)[0];
        f.render_widget(help_para, help_area);
    }

    if let InputMode::AddAccount { email, refresh_token, active_field, error_message } = &app.input_mode {
        let block = Block::default()
            .title(" Add Custom Account ")
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .style(Style::default().bg(palette.bg).fg(palette.border_active));
        
        let area = centered_rect(65, 45, f.size());
        f.render_widget(Clear, area);
        
        let modal_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Length(3),
                Constraint::Length(3),
                Constraint::Min(1),
            ])
            .margin(2)
            .split(area);
        
        f.render_widget(block, area);

        let email_block = Block::default()
            .title(" 1. Email Address ")
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .style(if *active_field == 0 { Style::default().fg(palette.yellow_warning) } else { Style::default().fg(palette.border_inactive) });
        let email_para = Paragraph::new(email.as_str()).block(email_block);
        f.render_widget(email_para, modal_chunks[0]);

        let token_block = Block::default()
            .title(" 2. OAuth Refresh Token ")
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .style(if *active_field == 1 { Style::default().fg(palette.yellow_warning) } else { Style::default().fg(palette.border_inactive) });
        let token_para = Paragraph::new(refresh_token.as_str()).block(token_block);
        f.render_widget(token_para, modal_chunks[1]);

        if let Some(err) = error_message {
            let err_para = Paragraph::new(format!("Error: {}", err))
                .style(Style::default().fg(palette.red_danger).add_modifier(Modifier::BOLD));
            f.render_widget(err_para, modal_chunks[2]);
        }

        let help_text = Paragraph::new(
            " [Tab] Switch Fields  |  [Enter] Verify & Add Account  |  [Esc] Cancel Modal\n (The refresh token will be validated with Google prior to saving.)"
        )
        .style(Style::default().fg(palette.border_inactive));
        f.render_widget(help_text, modal_chunks[3]);
    }

    if let InputMode::OAuthLogin { auth_url } = &app.input_mode {
        let block = Block::default()
            .title(" Google OAuth Authentication ")
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .style(Style::default().bg(palette.bg).fg(palette.border_active));
        
        let area = centered_rect(75, 55, f.size());
        f.render_widget(Clear, area);
        
        let modal_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Length(5),
                Constraint::Length(2),
                Constraint::Min(1),
            ])
            .margin(2)
            .split(area);
        
        f.render_widget(block, area);

        let intro = Paragraph::new("We have attempted to launch your default web browser for Google authentication.\nIf the browser did not open automatically, please visit the URL below:");
        f.render_widget(intro, modal_chunks[0]);

        let url_block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .title(" Copy & Paste URL ")
            .style(Style::default().fg(palette.yellow_warning));
        let url_para = Paragraph::new(auth_url.as_str())
            .block(url_block)
            .wrap(Wrap { trim: false });
        f.render_widget(url_para, modal_chunks[1]);

        let status_desc = Paragraph::new("Status: Awaiting authorization callback from Google loopback listener...")
            .style(Style::default().fg(palette.blue_reset_5h).add_modifier(Modifier::BOLD));
        f.render_widget(status_desc, modal_chunks[2]);

        let footer_help = Paragraph::new(" [Esc] Cancel OAuth Login Session\n Listening on local loopback TCP port 14210.")
            .style(Style::default().fg(palette.border_inactive));
        f.render_widget(footer_help, modal_chunks[3]);
    }

    if let InputMode::ConfirmDelete { email } = &app.input_mode {
        let block = Block::default()
            .title(" Delete Account Confirmation ")
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .style(Style::default().bg(palette.bg).fg(palette.red_danger));
        
        let area = centered_rect(50, 35, f.size());
        f.render_widget(Clear, area);
        f.render_widget(block, area);

        let modal_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Length(3),
                Constraint::Min(1),
            ])
            .margin(2)
            .split(area);

        let warn_desc = Paragraph::new(format!(
            "Are you sure you want to permanently delete the following account from your database?\n\n  {}",
            email
        ))
        .wrap(Wrap { trim: true })
        .style(Style::default().fg(palette.fg));
        f.render_widget(warn_desc, modal_chunks[0]);

        let alert = Paragraph::new("This action cannot be undone and will delete the account file!")
            .style(Style::default().fg(palette.red_danger).add_modifier(Modifier::BOLD));
        f.render_widget(alert, modal_chunks[1]);

        let prompt = Paragraph::new(" [y] Yes, Delete Account  |  [n] No, Cancel (Esc)")
            .style(Style::default().fg(palette.border_inactive));
        f.render_widget(prompt, modal_chunks[2]);
    }

    if app.show_logs {
        let block = Block::default()
            .title(" Session Logs History Explorer ")
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .style(Style::default().bg(palette.bg).fg(palette.border_active));
        
        let area = centered_rect(80, 70, f.size());
        f.render_widget(Clear, area);
        f.render_widget(block, area);

        let list_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(1),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .margin(2)
            .split(area);

        let filtered_logs: Vec<&String> = app.log_history.iter().filter(|log| {
            if app.log_search_query.is_empty() {
                true
            } else {
                log.to_lowercase().contains(&app.log_search_query.to_lowercase())
            }
        }).collect();

        let log_items: Vec<ListItem> = filtered_logs.iter().map(|log| {
            let log_lower = log.to_lowercase();
            let log_style = if log_lower.contains("error") || log_lower.contains("failed") || log_lower.contains("fail") {
                Style::default().fg(palette.red_danger)
            } else if log_lower.contains("warn") || log_lower.contains("warning") {
                Style::default().fg(palette.yellow_warning)
            } else if log_lower.contains("success") || log_lower.contains("activated") || log_lower.contains("active") {
                Style::default().fg(palette.green_success)
            } else if log_lower.contains("info") {
                Style::default().fg(palette.border_active)
            } else {
                Style::default().fg(palette.fg)
            };
            ListItem::new(Line::from(vec![
                Span::styled((*log).clone(), log_style)
            ]))
        }).collect();

        let list_widget = List::new(log_items)
            .highlight_style(Style::default().bg(palette.selection_bg).add_modifier(Modifier::BOLD));
        f.render_stateful_widget(list_widget, list_chunks[0], &mut app.log_state);

        let total_logs = filtered_logs.len();
        let current_log_pos = app.log_state.selected().unwrap_or(0);
        let mut log_scrollbar_state = ScrollbarState::new(total_logs).position(current_log_pos);
        let log_scrollbar = Scrollbar::default()
            .orientation(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("▲"))
            .end_symbol(Some("▼"));
        f.render_stateful_widget(log_scrollbar, list_chunks[0], &mut log_scrollbar_state);

        let search_text = if app.is_searching_logs {
            format!(" 🔍 Filter: {}_", app.log_search_query)
        } else if !app.log_search_query.is_empty() {
            format!(" 🔍 Filter: {} (Press [/] to edit, [Esc] to clear)", app.log_search_query)
        } else {
            " Press [/] to filter logs".to_string()
        };
        let search_bar = Paragraph::new(search_text)
            .style(Style::default().fg(palette.yellow_warning));
        f.render_widget(search_bar, list_chunks[1]);

        let tips = Paragraph::new(" [Esc/q/v] Close Logs Explorer  |  [j/k, Up/Down] Scroll History")
            .style(Style::default().fg(palette.border_inactive));
        f.render_widget(tips, list_chunks[2]);
    }

    if app.show_theme_selector {
        let block = Block::default()
            .title(" 🎨 Select Color Theme ")
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .style(Style::default().bg(palette.bg).fg(palette.border_active));
        
        let area = centered_rect(60, 50, f.size());
        f.render_widget(Clear, area);
        f.render_widget(block, area);

        let list_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Search input
                Constraint::Min(1),    // Themes list
                Constraint::Length(1), // Footer tips
            ])
            .margin(2)
            .split(area);

        let search_block = Block::default()
            .title(" 🔍 Search Palette Name ")
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .style(Style::default().fg(palette.yellow_warning));
        let search_para = Paragraph::new(format!("{}_", app.theme_search_query)).block(search_block);
        f.render_widget(search_para, list_chunks[0]);

        let visible_themes = app.get_visible_themes();
        let theme_items: Vec<ListItem> = visible_themes.iter().map(|t| {
            let active_indicator = if app.theme == *t { "● " } else { "  " };
            ListItem::new(Line::from(vec![
                Span::styled(active_indicator, Style::default().fg(palette.green_success)),
                Span::raw(t.to_str()),
            ]))
        }).collect();

        let list_widget = List::new(theme_items)
            .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).title(" Palette Presets ").style(Style::default().fg(palette.border_inactive)))
            .highlight_style(Style::default().bg(palette.selection_bg).add_modifier(Modifier::BOLD));
        f.render_stateful_widget(list_widget, list_chunks[1], &mut app.theme_list_state);

        let total_themes = visible_themes.len();
        let current_theme_pos = app.theme_list_state.selected().unwrap_or(0);
        let mut theme_scrollbar_state = ScrollbarState::new(total_themes).position(current_theme_pos);
        let theme_scrollbar = Scrollbar::default()
            .orientation(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("▲"))
            .end_symbol(Some("▼"));
        f.render_stateful_widget(theme_scrollbar, list_chunks[1], &mut theme_scrollbar_state);

        let tips = Paragraph::new(" [Esc/q/t] Cancel  |  [Enter] Select Theme  |  [j/k, Up/Down] Select preset")
            .style(Style::default().fg(palette.border_inactive));
        f.render_widget(tips, list_chunks[2]);
    }

    if app.show_sort_menu {
        let block = Block::default()
            .title(" ⇅ Select Sort Mode ")
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .style(Style::default().bg(palette.bg).fg(palette.border_active));
        
        let area = centered_rect(50, 45, f.size());
        f.render_widget(Clear, area);
        f.render_widget(block, area);

        let list_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(1),    // Options list
                Constraint::Length(1), // Footer tips
            ])
            .margin(2)
            .split(area);

        let sort_items: Vec<ListItem> = SORT_OPTIONS.iter().enumerate().map(|(idx, &(label, mode, desc))| {
            let is_active = app.sort_mode == mode && app.sort_desc == desc;
            let active_indicator = if is_active { "● " } else { "  " };
            ListItem::new(Line::from(vec![
                Span::styled(format!("[{}] ", (idx + 1) % 10), Style::default().fg(palette.border_inactive)),
                Span::styled(active_indicator, Style::default().fg(palette.green_success)),
                Span::raw(label),
            ]))
        }).collect();

        let list_widget = List::new(sort_items)
            .highlight_style(Style::default().bg(palette.selection_bg).add_modifier(Modifier::BOLD));
        f.render_stateful_widget(list_widget, list_chunks[0], &mut app.sort_menu_state);

        let tips = Paragraph::new(" [Esc/q/s] Cancel  |  [Enter] Select  |  [1-0] Select by Hotkey  |  [j/k] Scroll")
            .style(Style::default().fg(palette.border_inactive));
        f.render_widget(tips, list_chunks[1]);
    }
}
