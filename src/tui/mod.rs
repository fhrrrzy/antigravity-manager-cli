pub mod ui;

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use ratatui::layout::Rect;
use ratatui::widgets::{TableState, ListState};
use tokio::sync::mpsc;

use crate::types::{
    Account, CliCache, InputMode, SortMode, Focus, ThemeType, AppEvent, NetworkResult,
    QuotaData, COOLDOWN_SECONDS, TokenCache
};
use crate::config::{get_data_dir, save_cli_cache, load_warmup_history, add_account_to_db, save_warmup_history, record_health_failure};
use crate::google_api::{
    ensure_valid_token, async_fetch_project_and_tier, async_fetch_quota_summary,
    async_fetch_quota, async_trigger_warmup, listen_for_oauth_code, exchange_oauth_code, fetch_user_email,
};
use crate::keyring::{write_to_system_keyring, write_oauth_token_file};

pub struct App {
    pub accounts: Vec<Account>,
    pub db_path: PathBuf,
    pub db_desc: String,
    pub active_email: Option<String>,
    pub list_state: TableState,
    pub focused_panel: Focus,
    pub status_message: String,
    pub status_timestamp: Option<Instant>,
    pub is_loading: bool,
    pub tick_count: u64,
    pub cli_cache: CliCache,
    pub breakdown_state: ListState,
    pub warmup_history: HashMap<String, i64>,
    pub input_mode: InputMode,
    pub search_query: String,
    pub is_searching: bool,
    pub sort_mode: SortMode,
    pub sort_desc: bool,
    pub compact_mode: bool,
    pub show_help: bool,
    pub theme: ThemeType,
    pub show_logs: bool,
    pub log_history: Vec<String>,
    pub log_state: ListState,
    pub log_search_query: String,
    pub is_searching_logs: bool,
    pub show_theme_selector: bool,
    pub theme_search_query: String,
    pub theme_list_state: ListState,
    pub last_auto_refresh: Option<Instant>,
    pub show_sort_menu: bool,
    pub sort_menu_state: ListState,
    pub privacy_mode: bool,
}

impl App {
    pub fn new(
        accounts: Vec<Account>,
        db_path: PathBuf,
        db_desc: String,
        active_email: Option<String>,
        cli_cache: CliCache,
        warmup_history: HashMap<String, i64>,
    ) -> Self {
        let theme_str = cli_cache.theme.clone().unwrap_or_else(|| "kanagawa dragon".to_string());
        let theme = ThemeType::from_str(&theme_str);
        
        let mut list_state = TableState::default();
        if !accounts.is_empty() {
            let active_idx = if let Some(ref email) = active_email {
                accounts.iter().position(|a| a.email.to_lowercase() == email.to_lowercase()).unwrap_or(0)
            } else {
                0
            };
            list_state.select(Some(active_idx));
        }

        let mut breakdown_state = ListState::default();
        breakdown_state.select(Some(0));

        let mut theme_list_state = ListState::default();
        theme_list_state.select(Some(0));

        let mut log_state = ListState::default();
        log_state.select(Some(0));

        let mut app = App {
            accounts,
            db_path,
            db_desc,
            active_email,
            list_state,
            focused_panel: Focus::Accounts,
            status_message: "System initialized. Ready.".to_string(),
            status_timestamp: Some(Instant::now()),
            is_loading: false,
            tick_count: 0,
            cli_cache,
            breakdown_state,
            warmup_history,
            input_mode: InputMode::Normal,
            search_query: String::new(),
            is_searching: false,
            sort_mode: SortMode::Email,
            sort_desc: false,
            compact_mode: false,
            show_help: false,
            theme,
            show_logs: false,
            log_history: vec!["[INFO] System initialized successfully.".to_string()],
            log_state,
            log_search_query: String::new(),
            is_searching_logs: false,
            show_theme_selector: false,
            theme_search_query: String::new(),
            theme_list_state,
            last_auto_refresh: None,
            show_sort_menu: false,
            sort_menu_state: {
                let mut s = ListState::default();
                s.select(Some(0));
                s
            },
            privacy_mode: false,
        };
        app.sort_accounts();
        app
    }

    pub fn set_status(&mut self, msg: &str) {
        let formatted = format!("[{}] {}", chrono::Local::now().format("%H:%M:%S"), msg);
        self.status_message = formatted.clone();
        self.status_timestamp = Some(Instant::now());
        self.log_history.push(formatted);
        if self.log_history.len() > 1000 {
            self.log_history.remove(0);
        }
    }

    pub fn update_status_decay(&mut self) {
        if let Some(ts) = self.status_timestamp {
            if ts.elapsed() >= Duration::from_secs(8) && !self.is_loading {
                self.status_message = "Ready.".to_string();
                self.status_timestamp = None;
            }
        }
    }

    pub fn select_next(&mut self) {
        match self.focused_panel {
            Focus::Accounts => {
                let visible = self.get_visible_accounts();
                if !visible.is_empty() {
                    let i = match self.list_state.selected() {
                        Some(i) => {
                            if i >= visible.len() - 1 {
                                0
                            } else {
                                i + 1
                            }
                        }
                        None => 0,
                    };
                    self.list_state.select(Some(i));
                }
            }
            Focus::Breakdown => {
                if let Some(acc) = self.get_selected_account() {
                    if let Some(q) = self.cli_cache.quotas.get(&acc.email) {
                        let total = q.models.len();
                        if total > 0 {
                            let i = match self.breakdown_state.selected() {
                                Some(i) => {
                                    if i >= total - 1 {
                                        0
                                    } else {
                                        i + 1
                                    }
                                }
                                None => 0,
                            };
                            self.breakdown_state.select(Some(i));
                        }
                    }
                }
            }
        }
    }

    pub fn select_prev(&mut self) {
        match self.focused_panel {
            Focus::Accounts => {
                let visible = self.get_visible_accounts();
                if !visible.is_empty() {
                    let i = match self.list_state.selected() {
                        Some(i) => {
                            if i == 0 {
                                visible.len() - 1
                            } else {
                                i - 1
                            }
                        }
                        None => 0,
                    };
                    self.list_state.select(Some(i));
                }
            }
            Focus::Breakdown => {
                if let Some(acc) = self.get_selected_account() {
                    if let Some(q) = self.cli_cache.quotas.get(&acc.email) {
                        let total = q.models.len();
                        if total > 0 {
                            let i = match self.breakdown_state.selected() {
                                Some(i) => {
                                    if i == 0 {
                                        total - 1
                                    } else {
                                        i - 1
                                    }
                                }
                                None => 0,
                            };
                            self.breakdown_state.select(Some(i));
                        }
                    }
                }
            }
        }
    }

    pub fn get_selected_account(&self) -> Option<&Account> {
        let visible = self.get_visible_accounts();
        let idx = self.list_state.selected()?;
        if idx < visible.len() {
            Some(visible[idx])
        } else {
            None
        }
    }

    pub fn get_visible_accounts(&self) -> Vec<&Account> {
        self.accounts.iter().filter(|acc| {
            if self.search_query.is_empty() {
                true
            } else {
                acc.email.to_lowercase().contains(&self.search_query.to_lowercase())
            }
        }).collect()
    }

    pub fn sort_accounts(&mut self) {
        let get_model_pct = |email: &str, target: &str| -> i32 {
            self.cli_cache.quotas.get(email)
                .and_then(|q| q.models.iter().find(|m| m.name.contains(target) || m.display_name.as_ref().map(|n| n.contains(target)).unwrap_or(false)))
                .map(|m| m.percentage)
                .unwrap_or(999) // Undefined quotas go to end
        };

        let get_weekly_pct = |email: &str, is_claude: bool| -> i32 {
            let q = match self.cli_cache.quotas.get(email) {
                Some(q) => q,
                None => return 999,
            };
            let groups = match &q.quota_groups {
                Some(g) => g,
                None => return 999,
            };
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
                        return (bucket.remaining_fraction * 100.0).round() as i32;
                    }
                }
            }
            999
        };

        self.accounts.sort_by(|a, b| {
            let res = match self.sort_mode {
                SortMode::Email => a.email.cmp(&b.email),
                SortMode::Gemini5h => {
                    let pct_a = get_model_pct(&a.email, "gemini");
                    let pct_b = get_model_pct(&b.email, "gemini");
                    pct_a.cmp(&pct_b)
                }
                SortMode::GeminiWeekly => {
                    let pct_a = get_weekly_pct(&a.email, false);
                    let pct_b = get_weekly_pct(&b.email, false);
                    pct_a.cmp(&pct_b)
                }
                SortMode::Claude5h => {
                    let pct_a = get_model_pct(&a.email, "claude");
                    let pct_b = get_model_pct(&b.email, "claude");
                    pct_a.cmp(&pct_b)
                }
                SortMode::ClaudeWeekly => {
                    let pct_a = get_weekly_pct(&a.email, true);
                    let pct_b = get_weekly_pct(&b.email, true);
                    pct_a.cmp(&pct_b)
                }
            };
            if self.sort_desc {
                res.reverse()
            } else {
                res
            }
        });
    }

    pub fn get_column_index(&self, col_x: u16, area: Rect) -> Option<usize> {
        if col_x <= area.x || col_x >= area.x + area.width - 1 {
            return None;
        }

        let inner_width = if area.width > 2 { area.width - 2 } else { return None; };
        
        let fixed_cols_w = 68; // 8 (col0) + 15 * 4 (cols 2-5)
        let gaps_w = 5;        // 5 spacers of 1 char each
        
        let col1_w = if inner_width > fixed_cols_w + gaps_w {
            inner_width - (fixed_cols_w + gaps_w)
        } else {
            30
        };
        
        let widths = [8, col1_w, 15, 15, 15, 15];
        
        let mut cur_x = area.x + 1;
        for (idx, &w) in widths.iter().enumerate() {
            if col_x >= cur_x && col_x < cur_x + w {
                return Some(idx);
            }
            cur_x += w + 1; // width of column + 1 spacer gap
        }
        None
    }

    pub fn get_visible_themes(&self) -> Vec<ThemeType> {
        let all_themes = vec![
            ThemeType::KanagawaDragon,
            ThemeType::GruvboxDark,
            ThemeType::Nord,
            ThemeType::Dracula,
            ThemeType::OneDark,
            ThemeType::RetroMatrix,
            ThemeType::SolarizedDark,
            ThemeType::Catppuccin,
            ThemeType::RosePine,
            ThemeType::TokyoNight,
            ThemeType::AyuDark,
        ];
        if self.theme_search_query.is_empty() {
            all_themes
        } else {
            all_themes.into_iter().filter(|t| t.to_str().to_lowercase().contains(&self.theme_search_query.to_lowercase())).collect()
        }
    }

    pub fn backup_db(&self) -> Result<String, String> {
        let default_path = get_data_dir().join(format!("backup_antigravity_accounts_{}.json", chrono::Local::now().format("%Y-%m-%d")));
        
        #[derive(serde::Serialize)]
        struct BackupAcc {
            email: String,
            refresh_token: String,
            name: String,
        }
        
        let backup_data: Vec<BackupAcc> = self.accounts.iter().map(|a| BackupAcc {
            email: a.email.clone(),
            refresh_token: a.refresh_token.clone(),
            name: a.name.clone(),
        }).collect();
        
        let json_str = serde_json::to_string_pretty(&backup_data).map_err(|e| e.to_string())?;
        fs::write(&default_path, json_str).map_err(|e| e.to_string())?;
        Ok(default_path.to_string_lossy().to_string())
    }
}

// Spawns thread safe background task for networking calls
pub fn spawn_network_task(
    event_tx: mpsc::UnboundedSender<AppEvent>,
    account: Option<Account>,
    accounts_all: Vec<Account>,
    mut cli_cache: CliCache,
    warmup_history: HashMap<String, i64>,
    action: &'static str,
    target_model: Option<String>,
    force: bool,
    custom_submit: Option<(String, String, PathBuf)>,
) {
    tokio::spawn(async move {
        let now = chrono::Utc::now().timestamp();
        match action {
            "add_account" => {
                let Some((email, refresh_token, db_path)) = custom_submit else {
                    return;
                };
                match ensure_valid_token(&email, &refresh_token, &mut cli_cache).await {
                    Some((access_token, mut project_id)) => {
                        let (api_proj, tier) = async_fetch_project_and_tier(&access_token).await;
                        if api_proj.is_some() {
                            project_id = api_proj.clone();
                            if let Some(tc) = cli_cache.tokens.get_mut(&email) {
                                tc.project_id = api_proj;
                                tc.subscription_tier = tier.clone();
                            }
                        }
                        
                        let summary = async_fetch_quota_summary(&access_token, project_id.as_deref()).await;
                        match async_fetch_quota(&access_token, project_id.as_deref()).await {
                            Ok(models) => {
                                cli_cache.quotas.insert(email.clone(), QuotaData {
                                    subscription_tier: tier,
                                    models,
                                    quota_groups: summary,
                                });
                                save_cli_cache(&cli_cache);
                                
                                match add_account_to_db(&db_path, &email, &refresh_token) {
                                    Ok(new_acc) => {
                                        let _ = event_tx.send(AppEvent::NetworkSuccess(NetworkResult::AddAccountComplete { new_account: new_acc }));
                                    }
                                    Err(e) => {
                                        let _ = event_tx.send(AppEvent::NetworkError(format!("Add to DB failed: {}", e)));
                                    }
                                }
                            }
                            Err(e) => {
                                record_health_failure(&email, &format!("Fetch quota failed: {}", e), &mut cli_cache);
                                let _ = event_tx.send(AppEvent::NetworkError(format!("Fetch quota validation failed: {}", e)));
                            }
                        }
                    }
                    None => {
                        let _ = event_tx.send(AppEvent::NetworkError("Validation failed. Check refresh token.".to_string()));
                    }
                }
            }
            "oauth_login" => {
                let Some((_, _, db_path)) = custom_submit else {
                    return;
                };
                let port = 14210;
                match listen_for_oauth_code(port).await {
                    Ok(code) => {
                        let _ = event_tx.send(AppEvent::Progress("Exchanging code for tokens...".to_string()));
                        match exchange_oauth_code(&code, port).await {
                            Ok((access_token, refresh_token, expiry)) => {
                                let _ = event_tx.send(AppEvent::Progress("Fetching user profile...".to_string()));
                                match fetch_user_email(&access_token).await {
                                    Ok(email) => {
                                        let (proj_id, tier) = async_fetch_project_and_tier(&access_token).await;
                                        cli_cache.tokens.insert(email.clone(), TokenCache {
                                            access_token: access_token.clone(),
                                            expiry_timestamp: expiry,
                                            project_id: proj_id.clone(),
                                            subscription_tier: tier.clone(),
                                        });
                                        save_cli_cache(&cli_cache);
                                        
                                        let summary = async_fetch_quota_summary(&access_token, proj_id.as_deref()).await;
                                        if let Ok(models) = async_fetch_quota(&access_token, proj_id.as_deref()).await {
                                            cli_cache.quotas.insert(email.clone(), QuotaData {
                                                subscription_tier: tier,
                                                models,
                                                quota_groups: summary,
                                            });
                                            save_cli_cache(&cli_cache);
                                        }
                                        
                                        match add_account_to_db(&db_path, &email, &refresh_token) {
                                            Ok(new_acc) => {
                                                let _ = event_tx.send(AppEvent::NetworkSuccess(NetworkResult::AddAccountComplete { new_account: new_acc }));
                                            }
                                            Err(e) => {
                                                let _ = event_tx.send(AppEvent::NetworkError(format!("Failed saving OAuth: {}", e)));
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        let _ = event_tx.send(AppEvent::NetworkError(format!("Failed to retrieve email: {}", e)));
                                    }
                                }
                            }
                            Err(e) => {
                                let _ = event_tx.send(AppEvent::NetworkError(format!("Code exchange failed: {}", e)));
                            }
                        }
                    }
                    Err(e) => {
                        let _ = event_tx.send(AppEvent::NetworkError(format!("OAuth loopback error: {}", e)));
                    }
                }
            }
            "switch" => {
                let Some(account) = account else {
                    return;
                };
                let email = account.email.clone();
                match ensure_valid_token(&email, &account.refresh_token, &mut cli_cache).await {
                    Some((access_token, _)) => {
                        let expiry = cli_cache.tokens.get(&email).map(|t| t.expiry_timestamp).unwrap_or(0);
                        let keyring_success = write_to_system_keyring(&email, &access_token, &account.refresh_token, expiry);
                        write_oauth_token_file(&access_token, &account.refresh_token, expiry);
                        
                        cli_cache.active_email = Some(email.clone());
                        save_cli_cache(&cli_cache);
                        
                        let _ = event_tx.send(AppEvent::NetworkSuccess(NetworkResult::SwitchComplete { email, keyring_success }));
                    }
                    None => {
                        let _ = event_tx.send(AppEvent::NetworkError(format!("Failed to validate credentials for {}", email)));
                    }
                }
            }
            "quota" => {
                let Some(account) = account else {
                    return;
                };
                let email = account.email.clone();
                match ensure_valid_token(&email, &account.refresh_token, &mut cli_cache).await {
                    Some((access_token, mut project_id)) => {
                        let (api_proj, tier) = async_fetch_project_and_tier(&access_token).await;
                        if api_proj.is_some() {
                            project_id = api_proj.clone();
                            if let Some(tc) = cli_cache.tokens.get_mut(&email) {
                                tc.project_id = api_proj;
                                tc.subscription_tier = tier.clone();
                            }
                        }
                        
                        let summary = async_fetch_quota_summary(&access_token, project_id.as_deref()).await;
                        match async_fetch_quota(&access_token, project_id.as_deref()).await {
                            Ok(models) => {
                                let quota_data = QuotaData {
                                    subscription_tier: tier,
                                    models,
                                    quota_groups: summary,
                                };
                                cli_cache.quotas.insert(email.clone(), quota_data.clone());
                                save_cli_cache(&cli_cache);
                                let _ = event_tx.send(AppEvent::NetworkSuccess(NetworkResult::QuotaRefreshed {
                                    email,
                                    quota: quota_data,
                                    project_id,
                                }));
                            }
                            Err(e) => {
                                record_health_failure(&email, &format!("Fetch quota failed: {}", e), &mut cli_cache);
                                let _ = event_tx.send(AppEvent::NetworkError(format!("Fetch quota failed: {}", e)));
                            }
                        }
                    }
                    None => {
                        let _ = event_tx.send(AppEvent::NetworkError(format!("Failed to validate credentials for {}", email)));
                    }
                }
            }
            "quota_all" => {
                for acc in accounts_all {
                    let email = acc.email.clone();
                    let _ = event_tx.send(AppEvent::Progress(format!("Refreshing quota statistics for {}...", email)));
                    
                    if let Some((access_token, mut project_id)) = ensure_valid_token(&email, &acc.refresh_token, &mut cli_cache).await {
                        let (api_proj, tier) = async_fetch_project_and_tier(&access_token).await;
                        if api_proj.is_some() {
                            project_id = api_proj.clone();
                            if let Some(tc) = cli_cache.tokens.get_mut(&email) {
                                tc.project_id = api_proj;
                                tc.subscription_tier = tier.clone();
                            }
                        }
                        
                        let summary = async_fetch_quota_summary(&access_token, project_id.as_deref()).await;
                        if let Ok(models) = async_fetch_quota(&access_token, project_id.as_deref()).await {
                            let quota_data = QuotaData {
                                subscription_tier: tier,
                                models,
                                quota_groups: summary,
                            };
                            cli_cache.quotas.insert(email.clone(), quota_data.clone());
                            save_cli_cache(&cli_cache);
                            let _ = event_tx.send(AppEvent::NetworkSuccess(NetworkResult::QuotaRefreshed {
                                email,
                                quota: quota_data,
                                project_id,
                            }));
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(300)).await;
                }
                let _ = event_tx.send(AppEvent::Progress("All accounts quotas reloaded.".to_string()));
            }
            "warmup" => {
                let Some(account) = account else {
                    return;
                };
                let email = account.email.clone();
                
                let Some((access_token, resolved_proj_id)) = ensure_valid_token(&email, &account.refresh_token, &mut cli_cache).await else {
                    let _ = event_tx.send(AppEvent::NetworkError(format!("Failed to refresh credentials for {}", email)));
                    return;
                };
                
                let mut models = cli_cache.quotas.get(&email).map(|q| q.models.clone()).unwrap_or_default();
                if models.is_empty() || force {
                    if let Ok(m) = async_fetch_quota(&access_token, resolved_proj_id.as_deref()).await {
                        models = m;
                    }
                }
                
                if models.is_empty() && target_model.is_none() {
                    let _ = event_tx.send(AppEvent::NetworkError("No models available. Refresh quota first.".to_string()));
                    return;
                }
                
                let mut to_warm = Vec::new();
                if let Some(ref target) = target_model {
                    if let Some(m) = models.iter().find(|x| x.name == *target || x.display_name.as_deref() == Some(target)) {
                        to_warm.push(m.clone());
                    } else {
                        to_warm.push(crate::types::ModelQuota {
                            name: target.clone(),
                            percentage: 100,
                            display_name: Some(target.clone()),
                            reset_time: String::new(),
                        });
                    }
                } else {
                    for m in models.iter() {
                        if m.percentage >= 100 {
                            to_warm.push(m.clone());
                        }
                    }
                }
                
                if to_warm.is_empty() {
                    let _ = event_tx.send(AppEvent::NetworkSuccess(NetworkResult::WarmupComplete {
                        email,
                        warmup_count: 0,
                        skipped_count: 0,
                        logs: vec!["All models have remaining quotas, no warmup needed.".to_string()],
                    }));
                    return;
                }
                
                let mut warmup_count = 0;
                let mut skipped_count = 0;
                let mut logs = Vec::new();
                let mut record_success_models = Vec::new();
                
                for m in to_warm {
                    let name = m.name;
                    let display = m.display_name.unwrap_or_else(|| name.clone());
                    
                    if name.contains("2.5-") || name.contains("2-5-") {
                        logs.push(format!("Skipped {}: 2.5 series not supported.", display));
                        skipped_count += 1;
                        continue;
                    }
                    
                    if !force {
                        let key = format!("{}:{}:100", email, name);
                        if let Some(&last_ts) = warmup_history.get(&key) {
                            let elapsed = now - last_ts;
                            if elapsed < COOLDOWN_SECONDS {
                                logs.push(format!("Skipped {}: Cooling down ({}h {}m left).", display, (COOLDOWN_SECONDS - elapsed) / 3600, ((COOLDOWN_SECONDS - elapsed) % 3600) / 60));
                                skipped_count += 1;
                                continue;
                            }
                        }
                    }
                    
                    let _ = event_tx.send(AppEvent::Progress(format!("Warming up model {}...", display)));
                    match async_trigger_warmup(&access_token, &name, resolved_proj_id.as_deref(), &email).await {
                        Ok(_) => {
                            logs.push(format!("✓ Warmup successful for {}!", display));
                            warmup_count += 1;
                            record_success_models.push(name);
                        }
                        Err(e) => {
                            logs.push(format!("✗ Warmup failed for {}: {}", display, e));
                        }
                    }
                    
                    tokio::time::sleep(Duration::from_millis(800)).await;
                }
                
                let _ = event_tx.send(AppEvent::NetworkSuccess(NetworkResult::WarmupComplete {
                    email: email.clone(),
                    warmup_count,
                    skipped_count,
                    logs,
                }));
                
                for m_name in record_success_models {
                    let mut history = load_warmup_history();
                    let key = format!("{}:{}:100", email, m_name);
                    history.insert(key, chrono::Utc::now().timestamp());
                    save_warmup_history(&history);
                }
            }
            "warmup_all" => {
                let mut total_warmups = 0;
                let mut total_skipped = 0;
                let mut total_logs = Vec::new();
                let count_accs = accounts_all.len();
                
                for (idx, acc) in accounts_all.iter().enumerate() {
                    let email = &acc.email;
                    let _ = event_tx.send(AppEvent::Progress(format!("[{}/{}] Refreshing token for {}...", idx + 1, count_accs, email)));
                    
                    let Some((access_token, resolved_proj_id)) = ensure_valid_token(email, &acc.refresh_token, &mut cli_cache).await else {
                        total_logs.push(format!("Skipped {}: Token refresh failed.", email));
                        total_skipped += 1;
                        continue;
                    };
                    
                    let mut models = cli_cache.quotas.get(email).map(|q| q.models.clone()).unwrap_or_default();
                    if models.is_empty() || force {
                        if let Ok(m) = async_fetch_quota(&access_token, resolved_proj_id.as_deref()).await {
                            models = m;
                        }
                    }
                    
                    let mut to_warm = Vec::new();
                    for m in &models {
                        if m.percentage >= 100 {
                            to_warm.push(m.clone());
                        }
                    }
                    
                    if to_warm.is_empty() {
                        total_logs.push(format!("✓ {}: All models have remaining usage.", email));
                        continue;
                    }
                    
                    for m in to_warm {
                        let name = m.name;
                        let display = m.display_name.unwrap_or_else(|| name.clone());
                        
                        if name.contains("2.5-") || name.contains("2-5-") {
                            continue;
                        }
                        
                        if !force {
                            let key = format!("{}:{}:100", email, name);
                            if let Some(&last_ts) = warmup_history.get(&key) {
                                let elapsed = now - last_ts;
                                if elapsed < COOLDOWN_SECONDS {
                                    total_skipped += 1;
                                    continue;
                                }
                            }
                        }
                        
                        let _ = event_tx.send(AppEvent::Progress(format!("[{}/{}] Warmup {} on {}...", idx + 1, count_accs, display, email)));
                        match async_trigger_warmup(&access_token, &name, resolved_proj_id.as_deref(), email).await {
                            Ok(_) => {
                                total_logs.push(format!("✓ {} [{}]: Warmup successful!", email, display));
                                total_warmups += 1;
                                
                                let mut history = load_warmup_history();
                                let key = format!("{}:{}:100", email, name);
                                history.insert(key, chrono::Utc::now().timestamp());
                                save_warmup_history(&history);
                            }
                            Err(e) => {
                                total_logs.push(format!("✗ {} [{}]: Failed: {}", email, display, e));
                            }
                        }
                        tokio::time::sleep(Duration::from_millis(800)).await;
                    }
                }
                
                let _ = event_tx.send(AppEvent::NetworkSuccess(NetworkResult::WarmupComplete {
                    email: "All Accounts".to_string(),
                    warmup_count: total_warmups,
                    skipped_count: total_skipped,
                    logs: total_logs,
                }));
            }
            _ => {}
        }
    });
}
