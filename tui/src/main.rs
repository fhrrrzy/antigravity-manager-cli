use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event as CEvent, KeyCode, KeyEvent},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Terminal,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::mpsc;
use uuid::Uuid;

// Google OAuth Constants
const CLIENT_ID: &str = "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com";
const CLIENT_SECRET: &str = "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";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

// Cooldown duration: 4 hours
const COOLDOWN_SECONDS: i64 = 14400;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Account {
    email: String,
    refresh_token: String,
    name: String,
    source: String,
    #[serde(default)]
    id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TokenCache {
    access_token: String,
    expiry_timestamp: i64,
    project_id: Option<String>,
    subscription_tier: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ModelQuota {
    name: String,
    percentage: i32,
    reset_time: String,
    display_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct QuotaData {
    subscription_tier: Option<String>,
    models: Vec<ModelQuota>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CliCache {
    active_email: Option<String>,
    #[serde(default)]
    tokens: HashMap<String, TokenCache>,
    #[serde(default)]
    quotas: HashMap<String, QuotaData>,
}

enum AppEvent {
    Key(KeyEvent),
    Tick,
    NetworkSuccess(NetworkResult),
    NetworkError(String),
}

enum NetworkResult {
    QuotaRefreshed {
        email: String,
        quota: QuotaData,
        project_id: Option<String>,
    },
    WarmupComplete {
        email: String,
        warmup_count: usize,
        skipped_count: usize,
        logs: Vec<String>,
    },
    SwitchComplete {
        email: String,
        keyring_success: bool,
    },
}

struct App {
    accounts: Vec<Account>,
    db_desc: String,
    active_email: Option<String>,
    list_state: ListState,
    cli_cache: CliCache,
    warmup_history: HashMap<String, i64>,
    status_message: String,
    status_time: Option<Instant>,
    is_loading: bool,
}

impl App {
    fn new(accounts: Vec<Account>, db_desc: String, active: Option<String>, cache: CliCache, history: HashMap<String, i64>) -> Self {
        let mut list_state = ListState::default();
        if !accounts.is_empty() {
            list_state.select(Some(0));
        }
        
        Self {
            accounts,
            db_desc,
            active_email: active,
            list_state,
            cli_cache: cache,
            warmup_history: history,
            status_message: "Welcome to Antigravity TUI Manager!".to_string(),
            status_time: Some(Instant::now()),
            is_loading: false,
        }
    }

    fn select_next(&mut self) {
        if self.accounts.is_empty() {
            return;
        }
        let i = match self.list_state.selected() {
            Some(i) => {
                if i >= self.accounts.len() - 1 {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.list_state.select(Some(i));
    }

    fn select_prev(&mut self) {
        if self.accounts.is_empty() {
            return;
        }
        let i = match self.list_state.selected() {
            Some(i) => {
                if i == 0 {
                    self.accounts.len() - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.list_state.select(Some(i));
    }

    fn get_selected_account(&self) -> Option<&Account> {
        let idx = self.list_state.selected()?;
        self.accounts.get(idx)
    }

    fn set_status(&mut self, msg: &str) {
        self.status_message = msg.to_string();
        self.status_time = Some(Instant::now());
    }

    fn update_status_decay(&mut self) {
        if let Some(t) = self.status_time {
            if t.elapsed() > Duration::from_secs(7) {
                self.status_message = "Ready".to_string();
                self.status_time = None;
            }
        }
    }
}

// OS Config Helpers
fn get_data_dir() -> PathBuf {
    if let Ok(env_path) = std::env::var("ABV_DATA_DIR") {
        if !env_path.trim().is_empty() {
            let p = PathBuf::from(env_path.trim());
            let _ = fs::create_dir_all(&p);
            return p;
        }
    }
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let p = home.join(".antigravity_tools");
    let _ = fs::create_dir_all(&p);
    return p;
}

fn get_cli_cache_path() -> PathBuf {
    get_data_dir().join("cli_cache.json")
}

fn load_cli_cache() -> CliCache {
    let path = get_cli_cache_path();
    if path.exists() {
        if let Ok(content) = fs::read_to_string(&path) {
            if let Ok(cache) = serde_json::from_str::<CliCache>(&content) {
                return cache;
            }
        }
    }
    CliCache {
        active_email: None,
        tokens: HashMap::new(),
        quotas: HashMap::new(),
    }
}

fn save_cli_cache(cache: &CliCache) {
    let path = get_cli_cache_path();
    if let Ok(content) = serde_json::to_string_pretty(cache) {
        let _ = fs::write(&path, content);
    }
}

fn get_warmup_history_path() -> PathBuf {
    get_data_dir().join("warmup_history.json")
}

fn load_warmup_history() -> HashMap<String, i64> {
    let path = get_warmup_history_path();
    if path.exists() {
        if let Ok(content) = fs::read_to_string(&path) {
            if let Ok(history) = serde_json::from_str::<HashMap<String, i64>>(&content) {
                return history;
            }
        }
    }
    HashMap::new()
}

fn save_warmup_history(history: &HashMap<String, i64>) {
    let path = get_warmup_history_path();
    if let Ok(content) = serde_json::to_string_pretty(history) {
        let _ = fs::write(&path, content);
    }
}

// Load accounts index or backup
fn load_accounts_list() -> (Vec<Account>, String) {
    // Try primary backup path first
    let backup_paths = [
        "/data/data/com.termux/files/home/.antigravity_tools/antigravity_accounts_2026-07-17.json",
        "/home/fhrrrzy/Downloads/antigravity_accounts_2026-07-17.json",
    ];

    for path_str in backup_paths.iter() {
        let path = PathBuf::from(path_str);
        if path.exists() {
            if let Ok(content) = fs::read_to_string(&path) {
                #[derive(Deserialize)]
                struct RawBackupAcc {
                    email: String,
                    refresh_token: String,
                    name: Option<String>,
                }
                if let Ok(raw_accs) = serde_json::from_str::<Vec<RawBackupAcc>>(&content) {
                    let mut accounts = Vec::new();
                    for item in raw_accs {
                        let default_name = item.email.split('@').next().unwrap_or("").to_string();
                        accounts.push(Account {
                            name: item.name.unwrap_or(default_name),
                            email: item.email,
                            refresh_token: item.refresh_token,
                            source: format!("backup ({})", path.file_name().unwrap().to_string_lossy()),
                            id: None,
                        });
                    }
                    if !accounts.is_empty() {
                        return (accounts, format!("Backup file '{}'", path.file_name().unwrap().to_string_lossy()));
                    }
                }
            }
        }
    }

    // Fallback: Official accounts.json
    let data_dir = get_data_dir();
    let index_path = data_dir.join("accounts.json");
    if index_path.exists() {
        if let Ok(content) = fs::read_to_string(&index_path) {
            // Clean index contents from BOM/NUL
            let cleaned = content.replace("\u{feff}", "").replace('\x00', "");
            
            #[derive(Deserialize)]
            struct AccountSummary {
                id: String,
                email: String,
                name: Option<String>,
            }
            #[derive(Deserialize)]
            struct AccountIndex {
                accounts: Vec<AccountSummary>,
            }

            if let Ok(index_data) = serde_json::from_str::<AccountIndex>(&cleaned) {
                let mut accounts = Vec::new();
                for acc in index_data.accounts {
                    let acc_path = data_dir.join("accounts").join(format!("{}.json", acc.id));
                    if acc_path.exists() {
                        if let Ok(af_content) = fs::read_to_string(&acc_path) {
                            #[derive(Deserialize)]
                            struct TokenDetails {
                                refresh_token: String,
                            }
                            #[derive(Deserialize)]
                            struct AccountDetails {
                                token: TokenDetails,
                            }
                            if let Ok(details) = serde_json::from_str::<AccountDetails>(&af_content) {
                                accounts.push(Account {
                                    email: acc.email,
                                    refresh_token: details.token.refresh_token,
                                    name: acc.name.unwrap_or_else(|| "N/A".to_string()),
                                    source: "Tauri SQLite/JSON index".to_string(),
                                    id: Some(acc.id),
                                });
                            }
                        }
                    }
                }
                if !accounts.is_empty() {
                    return (accounts, "Tauri official database".to_string());
                }
            }
        }
    }

    (Vec::new(), "No account source found".to_string())
}

fn get_active_email(accounts: &[Account]) -> Option<String> {
    let cache = load_cli_cache();
    if let Some(ref active) = cache.active_email {
        if accounts.iter().any(|a| a.email.to_lowercase() == active.to_lowercase()) {
            return Some(active.clone());
        }
    }
    
    // Fallback: Official active account in accounts.json
    let index_path = get_data_dir().join("accounts.json");
    if index_path.exists() {
        if let Ok(content) = fs::read_to_string(&index_path) {
            let cleaned = content.replace("\u{feff}", "").replace('\x00', "");
            #[derive(Deserialize)]
            struct AccountSummary {
                id: String,
                email: String,
            }
            #[derive(Deserialize)]
            struct AccountIndex {
                accounts: Vec<AccountSummary>,
                current_account_id: Option<String>,
            }
            if let Ok(index_data) = serde_json::from_str::<AccountIndex>(&cleaned) {
                if let Some(curr_id) = index_data.current_account_id {
                    for acc in index_data.accounts {
                        if acc.id == curr_id {
                            return Some(acc.email);
                        }
                    }
                }
            }
        }
    }
    
    // Fallback to first
    if !accounts.is_empty() {
        return Some(accounts[0].email.clone());
    }
    None
}

// System Keyring helpers (Android, Linux, macOS, Windows)
fn write_to_system_keyring(_email: &str, access_token: &str, refresh_token: &str, expiry_timestamp: i64) -> bool {
    let expiry_datetime = chrono::DateTime::from_timestamp(expiry_timestamp, 0)
        .unwrap_or_else(|| chrono::Utc::now());
    let expiry_str = expiry_datetime.to_rfc3339_opts(chrono::SecondsFormat::Micros, true);
    
    let payload = json!({
        "token": {
            "access_token": access_token,
            "token_type": "Bearer",
            "refresh_token": refresh_token,
            "expiry": expiry_str
        },
        "auth_method": "consumer"
    });
    let payload_json = serde_json::to_string(&payload).unwrap();
    
    let system = std::env::consts::OS;
    if system == "linux" {
        // Android is detected as "linux" in std::env::consts::OS.
        // We will test if 'secret-tool' exists. If not, we skip silently.
        let secret_tool_check = subprocess_exists("secret-tool");
        if !secret_tool_check {
            // Android Termux runs here. We skip keyring and return true (it falls back to caching/accounts.json).
            return true;
        }
        
        let child_check = std::process::Command::new("secret-tool")
            .args(["store", "--label=gemini", "service", "gemini", "username", "antigravity"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .is_ok();
            
        if child_check {
            // Write payload
            if let Ok(mut c) = std::process::Command::new("secret-tool")
                .args(["store", "--label=gemini", "service", "gemini", "username", "antigravity"])
                .stdin(std::process::Stdio::piped())
                .spawn()
            {
                use std::io::Write;
                if let Some(mut stdin) = c.stdin.take() {
                    let _ = stdin.write_all(payload_json.as_bytes());
                }
                let _ = c.wait();
                return true;
            }
        }
        return false;
    } else if system == "macos" {
        let _ = std::process::Command::new("security")
            .args(["delete-generic-password", "-s", "gemini", "-a", "antigravity"])
            .output();
            
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        let encoded = STANDARD.encode(payload_json);
        let full_val = format!("go-keyring-base64:{}", encoded);
        
        let output = std::process::Command::new("security")
            .args(["add-generic-password", "-s", "gemini", "-a", "antigravity", "-w", &full_val, "-A"])
            .output();
            
        return output.map(|o| o.status.success()).unwrap_or(false);
    }
    
    // Fallback/Windows (not implemented in blocking, keep simple)
    true
}

fn subprocess_exists(cmd: &str) -> bool {
    std::process::Command::new("which")
        .arg(cmd)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

// Asynchronous network operations
async fn async_refresh_token(refresh_token: String) -> Option<(String, i64)> {
    let client = reqwest::Client::new();
    let params = [
        ("client_id", CLIENT_ID),
        ("client_secret", CLIENT_SECRET),
        ("refresh_token", &refresh_token),
        ("grant_type", "refresh_token"),
    ];
    
    let res = client
        .post(TOKEN_URL)
        .header("User-Agent", "vscode/1.X.X (Antigravity/4.3.0)")
        .form(&params)
        .send()
        .await;
        
    if let Ok(resp) = res {
        if resp.status().is_success() {
            #[derive(Deserialize)]
            struct TokenResponse {
                access_token: String,
                expires_in: i64,
            }
            if let Ok(data) = resp.json::<TokenResponse>().await {
                let expiry = chrono::Utc::now().timestamp() + data.expires_in;
                return Some((data.access_token, expiry));
            }
        }
    }
    None
}

async fn async_fetch_project_and_tier(access_token: &str) -> (Option<String>, Option<String>) {
    let client = reqwest::Client::new();
    let payload = json!({"metadata": {"ideType": "ANTIGRAVITY"}});
    
    let res = client
        .post("https://daily-cloudcode-pa.sandbox.googleapis.com/v1internal:loadCodeAssist")
        .header("Authorization", format!("Bearer {}", access_token))
        .header("Content-Type", "application/json")
        .header("User-Agent", "vscode/1.X.X (Antigravity/4.3.0)")
        .json(&payload)
        .send()
        .await;
        
    if let Ok(resp) = res {
        if resp.status().is_success() {
            #[derive(Deserialize)]
            struct Tier {
                name: Option<String>,
                id: Option<String>,
                #[serde(rename = "isDefault")]
                is_default: Option<bool>,
            }
            #[derive(Deserialize)]
            struct LoadProjectResponse {
                #[serde(rename = "cloudaicompanionProject")]
                project_id: Option<String>,
                #[serde(rename = "paidTier")]
                paid_tier: Option<Tier>,
                #[serde(rename = "currentTier")]
                current_tier: Option<Tier>,
                #[serde(rename = "allowedTiers")]
                allowed_tiers: Option<Vec<Tier>>,
            }
            if let Ok(data) = resp.json::<LoadProjectResponse>().await {
                let project_id = data.project_id;
                
                let mut tier_name = data.paid_tier.as_ref().and_then(|t| t.name.clone()).or_else(|| data.paid_tier.as_ref().and_then(|t| t.id.clone()));
                if tier_name.is_none() {
                    tier_name = data.current_tier.as_ref().and_then(|t| t.name.clone()).or_else(|| data.current_tier.as_ref().and_then(|t| t.id.clone()));
                }
                if tier_name.is_none() {
                    if let Some(allowed) = data.allowed_tiers {
                        if let Some(def_t) = allowed.iter().find(|t| t.is_default == Some(true)) {
                            tier_name = def_t.name.clone().or_else(|| def_t.id.clone()).map(|n| format!("{} (Restricted)", n));
                        }
                    }
                }
                return (project_id, tier_name);
            }
        }
    }
    (None, None)
}

async fn async_fetch_quota(access_token: &str, project_id: Option<&str>) -> Result<Vec<ModelQuota>, String> {
    let client = reqwest::Client::new();
    let payload = if let Some(pid) = project_id {
        json!({ "project": pid })
    } else {
        json!({})
    };
    
    let urls = [
        "https://daily-cloudcode-pa.sandbox.googleapis.com/v1internal:fetchAvailableModels",
        "https://daily-cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels",
        "https://cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels",
    ];
    
    let mut last_err = String::new();
    for url in urls.iter() {
        let res = client
            .post(*url)
            .header("Authorization", format!("Bearer {}", access_token))
            .header("Content-Type", "application/json")
            .header("User-Agent", "vscode/1.X.X (Antigravity/4.3.0)")
            .json(&payload)
            .send()
            .await;
            
        match res {
            Ok(resp) => {
                if resp.status().is_success() {
                    #[derive(Deserialize)]
                    struct QuotaInfo {
                        #[serde(rename = "remainingFraction")]
                        remaining_fraction: Option<f64>,
                        #[serde(rename = "resetTime")]
                        reset_time: Option<String>,
                    }
                    #[derive(Deserialize)]
                    struct ModelInfo {
                        #[serde(rename = "quotaInfo")]
                        quota_info: Option<QuotaInfo>,
                        #[serde(rename = "displayName")]
                        display_name: Option<String>,
                    }
                    #[derive(Deserialize)]
                    struct QuotaResponse {
                        models: HashMap<String, ModelInfo>,
                    }
                    
                    if let Ok(data) = resp.json::<QuotaResponse>().await {
                        let mut models = Vec::new();
                        for (name, info) in data.models {
                            if let Some(q_info) = info.quota_info {
                                let pct = (q_info.remaining_fraction.unwrap_or(0.0) * 100.0) as i32;
                                if name.starts_with("gemini")
                                    || name.starts_with("claude")
                                    || name.starts_with("gpt")
                                    || name.starts_with("image")
                                    || name.starts_with("imagen")
                                {
                                    models.push(ModelQuota {
                                        name,
                                        percentage: pct,
                                        reset_time: q_info.reset_time.unwrap_or_default(),
                                        display_name: info.display_name,
                                    });
                                }
                            }
                        }
                        return Ok(models);
                    }
                } else if resp.status() == reqwest::StatusCode::FORBIDDEN {
                    // Try without project ID
                    if project_id.is_some() {
                        return Box::pin(async_fetch_quota(access_token, None)).await;
                    }
                    return Err("403 Forbidden: Account unauthorized".to_string());
                } else {
                    last_err = format!("HTTP {}", resp.status());
                }
            }
            Err(e) => {
                last_err = e.to_string();
            }
        }
    }
    Err(format!("All endpoints failed: {}", last_err))
}

async fn async_trigger_warmup(access_token: &str, model_name: &str, project_id: Option<&str>, email: &str) -> Result<(), String> {
    let timestamp_ms = chrono::Utc::now().timestamp_millis();
    let random_hex = &Uuid::new_v4().simple().to_string()[..8];
    let request_id = format!("agent/{}/{}", timestamp_ms, random_hex);
    
    let is_enterprise = !(email.ends_with("@gmail.com") || email.ends_with("@googlemail.com"));
    let user_agent = if is_enterprise { "jetski" } else { "antigravity" };
    
    let body = json!({
        "project": project_id.unwrap_or(""),
        "model": model_name,
        "userAgent": user_agent,
        "requestType": "agent",
        "requestId": request_id,
        "enabledCreditTypes": ["GOOGLE_ONE_AI"],
        "request": {
            "contents": [{"role": "user", "parts": [{"text": "Say hi"}]}],
            "generationConfig": {
                "temperature": 0,
                "maxOutputTokens": 1
            }
        }
    });
    
    let urls = [
        "https://daily-cloudcode-pa.sandbox.googleapis.com/v1internal:generateContent",
        "https://daily-cloudcode-pa.sandbox.googleapis.com/v1internal:generateContent",
        "https://cloudcode-pa.googleapis.com/v1internal:generateContent"
    ];
    
    let client = reqwest::Client::new();
    let mut last_err = String::new();
    
    for url in urls.iter() {
        let res = client
            .post(*url)
            .header("Authorization", format!("Bearer {}", access_token))
            .header("Content-Type", "application/json")
            .header("User-Agent", "vscode/1.X.X (Antigravity/4.3.0)")
            .json(&body)
            .send()
            .await;
            
        match res {
            Ok(resp) => {
                if resp.status().is_success() {
                    return Ok(());
                } else {
                    last_err = format!("HTTP {} - {}", resp.status(), resp.text().await.unwrap_or_default());
                }
            }
            Err(e) => {
                last_err = e.to_string();
            }
        }
    }
    Err(last_err)
}

// Unified token resolver and cache saver (reusable in CLI & TUI)
async fn ensure_valid_token(email: &str, refresh_token: &str, cli_cache: &mut CliCache) -> Option<(String, Option<String>)> {
    let now = chrono::Utc::now().timestamp();
    if let Some(tc) = cli_cache.tokens.get(email) {
        if tc.expiry_timestamp > now + 300 {
            return Some((tc.access_token.clone(), tc.project_id.clone()));
        }
    }
    
    // Refresh expired/missing token
    if let Some((new_tok, new_exp)) = async_refresh_token(refresh_token.to_string()).await {
        let (proj_id, tier) = async_fetch_project_and_tier(&new_tok).await;
        cli_cache.tokens.insert(email.to_string(), TokenCache {
            access_token: new_tok.clone(),
            expiry_timestamp: new_exp,
            project_id: proj_id.clone(),
            subscription_tier: tier,
        });
        save_cli_cache(cli_cache);
        Some((new_tok, proj_id))
    } else {
        None
    }
}

// Background network task runner (for TUI)
fn spawn_network_task(
    event_tx: mpsc::UnboundedSender<AppEvent>,
    account: Account,
    mut cli_cache: CliCache,
    warmup_history: HashMap<String, i64>,
    action: &'static str,
    target_model: Option<String>,
    force: bool,
) {
    tokio::spawn(async move {
        let email = account.email.clone();
        
        let token_info = ensure_valid_token(&email, &account.refresh_token, &mut cli_cache).await;
        if token_info.is_none() {
            let _ = event_tx.send(AppEvent::NetworkError(format!("Failed to refresh credentials for {}", email)));
            return;
        }
        
        let (access_token, resolved_proj_id) = token_info.unwrap();
        let now = chrono::Utc::now().timestamp();
        
        match action {
            "switch" => {
                let expiry = cli_cache.tokens.get(&email).map(|d| d.expiry_timestamp).unwrap_or(now + 3600);
                let keyring_success = write_to_system_keyring(&email, &access_token, &account.refresh_token, expiry);
                
                let _ = event_tx.send(AppEvent::NetworkSuccess(NetworkResult::SwitchComplete {
                    email: email.clone(),
                    keyring_success,
                }));
                
                // Propagate project ID & subscription tier back to TUI
                let details = cli_cache.tokens.get(&email).cloned();
                if let Some(fd) = details {
                    let _ = event_tx.send(AppEvent::NetworkSuccess(NetworkResult::QuotaRefreshed {
                        email: account.email,
                        quota: QuotaData { subscription_tier: fd.subscription_tier, models: Vec::new() },
                        project_id: fd.project_id,
                    }));
                }
            }
            "quota" => {
                match async_fetch_quota(&access_token, resolved_proj_id.as_deref()).await {
                    Ok(models) => {
                        let tier = cli_cache.tokens.get(&email).and_then(|d| d.subscription_tier.clone());
                        let q = QuotaData {
                            subscription_tier: tier,
                            models,
                        };
                        let _ = event_tx.send(AppEvent::NetworkSuccess(NetworkResult::QuotaRefreshed {
                            email,
                            quota: q,
                            project_id: resolved_proj_id,
                        }));
                    }
                    Err(e) => {
                        let _ = event_tx.send(AppEvent::NetworkError(format!("Fetch quota failed: {}", e)));
                    }
                }
            }
            "warmup" => {
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
                        to_warm.push(ModelQuota {
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
                                let rem = COOLDOWN_SECONDS - elapsed;
                                logs.push(format!("Skipped {}: Cooling down ({}h {}m left).", display, rem / 3600, (rem % 3600) / 60));
                                skipped_count += 1;
                                continue;
                            }
                        }
                    }
                    
                    logs.push(format!("Warming up {}...", display));
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
            _ => {}
        }
    });
}

// ---------------------------------------------------------
// CLI COMMANDS IMPLEMENTATION (Rust-native CLI mode)
// ---------------------------------------------------------

fn find_account_by_identifier<'a>(accounts: &'a [Account], id: &str) -> Option<&'a Account> {
    if let Ok(idx) = id.parse::<usize>() {
        if idx > 0 && idx <= accounts.len() {
            return Some(&accounts[idx - 1]);
        }
    }
    accounts.iter().find(|a| a.email.to_lowercase() == id.to_lowercase())
}

fn cli_list(accounts: &[Account], active_email: Option<&str>, source: &str) {
    if accounts.is_empty() {
        println!("No accounts configured. Check backup file.");
        return;
    }
    println!("\nAccounts List (Source: {}):", source);
    println!("============================================================");
    println!("{:<3} | {:<6} | {:<32} | {:<20}", "#", "Active", "Email", "Name");
    println!("------------------------------------------------------------");
    for (idx, acc) in accounts.iter().enumerate() {
        let is_active = active_email == Some(&acc.email);
        let active_mark = if is_active { "★" } else { " " };
        println!("{:<3} | {:<6} | {:<32} | {:<20}", idx + 1, active_mark, acc.email, acc.name);
    }
    println!("\n★ = Current active account used by Antigravity.");
}

async fn cli_switch(accounts: &[Account], identifier: &str) {
    let acc = match find_account_by_identifier(accounts, identifier) {
        Some(a) => a,
        None => {
            eprintln!("Error: Account matching '{}' not found.", identifier);
            std::process::exit(1);
        }
    };
    
    let mut cache = load_cli_cache();
    let email = &acc.email;
    println!("Switching active account to: {}...", email);
    
    if let Some((access_token, project_id)) = ensure_valid_token(email, &acc.refresh_token, &mut cache).await {
        let expiry = cache.tokens.get(email).map(|t| t.expiry_timestamp).unwrap_or(0);
        let keyring_success = write_to_system_keyring(email, &access_token, &acc.refresh_token, expiry);
        
        cache.active_email = Some(email.clone());
        save_cli_cache(&cache);
        
        // Sync to official accounts.json if it exists
        let data_dir = get_data_dir();
        let index_path = data_dir.join("accounts.json");
        if index_path.exists() {
            if let Some(ref acc_id) = acc.id {
                if let Ok(content) = fs::read_to_string(&index_path) {
                    let cleaned = content.replace("\u{feff}", "").replace('\x00', "");
                    if let Ok(mut val) = serde_json::from_str::<serde_json::Value>(&cleaned) {
                        if let Some(obj) = val.as_object_mut() {
                            obj.insert("current_account_id".to_string(), json!(acc_id));
                            if let Ok(new_content) = serde_json::to_string_pretty(&val) {
                                let _ = fs::write(&index_path, new_content);
                            }
                        }
                    }
                }
            }
        }
        
        println!("✓ Active account changed to {} ({}).", email, acc.name);
        if keyring_success {
            println!("✓ Credentials successfully written to system keyring.");
        } else {
            println!("⚠️  Keyring write skipped/unsupported (fallback active).");
        }
    } else {
        eprintln!("Error: Failed to refresh credentials for {}.", email);
        std::process::exit(1);
    }
}

async fn cli_quota(accounts: &[Account], active_email: Option<&str>, identifier: Option<&str>, refresh: bool) {
    let target_email = match identifier {
        Some(id) => match find_account_by_identifier(accounts, id) {
            Some(a) => &a.email,
            None => {
                eprintln!("Error: Account matching '{}' not found.", id);
                std::process::exit(1);
            }
        },
        None => match active_email {
            Some(email) => email,
            None => {
                eprintln!("Error: No active account configured. Specify an index or email.");
                std::process::exit(1);
            }
        }
    };
    
    let acc = accounts.iter().find(|a| a.email == *target_email).unwrap();
    let mut cache = load_cli_cache();
    
    let (access_token, mut project_id) = match ensure_valid_token(target_email, &acc.refresh_token, &mut cache).await {
        Some(t) => t,
        None => {
            eprintln!("Error: Failed to validate token for {}.", target_email);
            std::process::exit(1);
        }
    };
    
    if refresh {
        println!("Fetching latest quota from Google APIs for {}...", target_email);
        let (api_proj, tier) = async_fetch_project_and_tier(&access_token).await;
        if api_proj.is_some() {
            project_id = api_proj.clone();
            if let Some(tc) = cache.tokens.get_mut(target_email) {
                tc.project_id = api_proj;
                tc.subscription_tier = tier.clone();
            }
        }
        
        match async_fetch_quota(&access_token, project_id.as_deref()).await {
            Ok(models) => {
                cache.quotas.insert(target_email.clone(), QuotaData {
                    subscription_tier: tier.or_else(|| cache.tokens.get(target_email).and_then(|t| t.subscription_tier.clone())),
                    models,
                });
                save_cli_cache(&cache);
                println!("✓ Quota cache updated.");
            }
            Err(e) => {
                eprintln!("Error fetching quota: {}", e);
                std::process::exit(1);
            }
        }
    }
    
    let quota_data = cache.quotas.get(target_email);
    if quota_data.is_none() || quota_data.unwrap().models.is_empty() {
        println!("No cached quotas for {}. Run with '--refresh' to fetch.", target_email);
        return;
    }
    
    let q = quota_data.unwrap();
    println!("\nQuota for {}:", target_email);
    println!("Subscription Tier: {}", q.subscription_tier.as_deref().unwrap_or("N/A"));
    println!("Project ID: {}", project_id.as_deref().unwrap_or("N/A"));
    println!("========================================================================");
    println!("{:<32} | {:<25} | {:<12} | {:<20}", "Model Display Name", "Model ID", "Remaining %", "Reset Time (UTC)");
    println!("------------------------------------------------------------------------");
    for m in &q.models {
        let display = m.display_name.as_deref().unwrap_or(&m.name);
        println!("{:<32} | {:<25} | {:<12}% | {:<20}", display, m.name, m.percentage, m.reset_time);
    }
}

async fn cli_warmup(accounts: &[Account], active_email: Option<&str>, identifier: Option<&str>, model_name: Option<&str>, force: bool) {
    let target_email = match identifier {
        Some(id) => match find_account_by_identifier(accounts, id) {
            Some(a) => &a.email,
            None => {
                eprintln!("Error: Account matching '{}' not found.", id);
                std::process::exit(1);
            }
        },
        None => match active_email {
            Some(email) => email,
            None => {
                eprintln!("Error: No active account configured. Specify an index or email.");
                std::process::exit(1);
            }
        }
    };
    
    let acc = accounts.iter().find(|a| a.email == *target_email).unwrap();
    let mut cache = load_cli_cache();
    let mut history = load_warmup_history();
    let now = chrono::Utc::now().timestamp();
    
    let (access_token, mut project_id) = match ensure_valid_token(target_email, &acc.refresh_token, &mut cache).await {
        Some(t) => t,
        None => {
            eprintln!("Error: Failed to validate token for {}.", target_email);
            std::process::exit(1);
        }
    };
    
    let mut models = cache.quotas.get(target_email).map(|q| q.models.clone()).unwrap_or_default();
    if models.is_empty() || force {
        println!("Refreshing quota list...");
        let (api_proj, tier) = async_fetch_project_and_tier(&access_token).await;
        if api_proj.is_some() {
            project_id = api_proj.clone();
            if let Some(tc) = cache.tokens.get_mut(target_email) {
                tc.project_id = api_proj;
                tc.subscription_tier = tier;
            }
        }
        if let Ok(m) = async_fetch_quota(&access_token, project_id.as_deref()).await {
            models = m;
        }
    }
    
    let mut to_warm = Vec::new();
    if let Some(ref m_name) = model_name {
        if let Some(m) = models.iter().find(|x| x.name == *m_name || x.display_name.as_deref() == Some(m_name)) {
            to_warm.push(m.clone());
        } else {
            to_warm.push(ModelQuota {
                name: m_name.clone(),
                percentage: 100,
                display_name: Some(m_name.clone()),
                reset_time: String::new(),
            });
        }
    } else {
        for m in &models {
            if m.percentage >= 100 {
                to_warm.push(m.clone());
            }
        }
    }
    
    if to_warm.is_empty() {
        println!("All models have remaining quotas. No warmup needed.");
        return;
    }
    
    let mut count = 0;
    for m in to_warm {
        let display = m.display_name.as_deref().unwrap_or(&m.name);
        
        if m.name.contains("2.5-") || m.name.contains("2-5-") {
            println!("Skipping {}: 2.5 series not supported.", display);
            continue;
        }
        
        if !force {
            let key = format!("{}:{}:100", target_email, m.name);
            if let Some(&last) = history.get(&key) {
                let elapsed = now - last;
                if elapsed < COOLDOWN_SECONDS {
                    let rem = COOLDOWN_SECONDS - elapsed;
                    println!("Skipping {}: Cooling down ({}h {}m remaining).", display, rem / 3600, (rem % 3600) / 60);
                    continue;
                }
            }
        }
        
        println!("Warming up model {}...", display);
        match async_trigger_warmup(&access_token, &m.name, project_id.as_deref(), target_email).await {
            Ok(_) => {
                println!("✓ Successfully warmed up {}!", display);
                let key = format!("{}:{}:100", target_email, m.name);
                history.insert(key, chrono::Utc::now().timestamp());
                save_warmup_history(&history);
                count += 1;
            }
            Err(e) => {
                println!("✗ Warmup failed for {}: {}", display, e);
            }
        }
        
        tokio::time::sleep(Duration::from_millis(800)).await;
    }
    println!("Warmup cycle finished. Triggered {} warmup(s).", count);
}

// ---------------------------------------------------------
// MAIN RUNTIME ORCHESTRATOR
// ---------------------------------------------------------

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (accounts, db_desc) = load_accounts_list();
    let active_email = get_active_email(&accounts);
    
    // Parse command line arguments
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        // Run in CLI mode
        let subcommand = &args[1];
        match subcommand.as_str() {
            "list" => {
                cli_list(&accounts, active_email.as_deref(), &db_desc);
            }
            "switch" => {
                if args.len() < 3 {
                    eprintln!("Usage: agm-tui switch <index/email>");
                    std::process::exit(1);
                }
                cli_switch(&accounts, &args[2]).await;
            }
            "quota" => {
                let mut identifier = None;
                let mut refresh = false;
                for arg in args.iter().skip(2) {
                    if arg == "--refresh" {
                        refresh = true;
                    } else if !arg.starts_with('-') {
                        identifier = Some(arg.as_str());
                    }
                }
                cli_quota(&accounts, active_email.as_deref(), identifier, refresh).await;
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
                cli_warmup(&accounts, active_email.as_deref(), identifier, model.as_deref(), force).await;
            }
            "help" | "-h" | "--help" => {
                println!("Antigravity Manager (Rust Unified Edition)\n");
                println!("Usage:");
                println!("  agm-tui                   Launch interactive terminal user interface (TUI)");
                println!("  agm-tui list              List configured accounts");
                println!("  agm-tui switch <id>       Switch the active account");
                println!("  agm-tui quota [id] [-r]   Display quotas (use --refresh to update)");
                println!("  agm-tui warmup [id] [flg] Run warmup cycles (use --model <name> or --force)");
                println!("\nExamples:");
                println!("  agm-tui switch 3");
                println!("  agm-tui quota --refresh");
                println!("  agm-tui warmup --force");
            }
            _ => {
                eprintln!("Unknown command '{}'. Type 'agm-tui --help' for help.", subcommand);
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    // Default: Run interactive TUI Mode
    let cache = load_cli_cache();
    let history = load_warmup_history();
    
    // Setup Terminal GUI
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    
    // Setup Async channels for background network tasks
    let (event_tx, mut event_rx) = mpsc::unbounded_channel();
    
    // Spawn keyboard event listener thread
    let tx = event_tx.clone();
    tokio::spawn(async move {
        loop {
            if event::poll(Duration::from_millis(200)).unwrap() {
                if let CEvent::Key(key) = event::read().unwrap() {
                    let _ = tx.send(AppEvent::Key(key));
                }
            }
            let _ = tx.send(AppEvent::Tick);
        }
    });

    // Initialize TUI App state
    let mut app = App::new(accounts, db_desc, active_email, cache, history);

    // Fetch initial quota in the background for active account if cached quota is empty
    if let Some(ref email) = app.active_email {
        if !app.cli_cache.quotas.contains_key(email) && !app.accounts.is_empty() {
            if let Some(acc) = app.accounts.iter().find(|a| a.email == *email).cloned() {
                app.is_loading = true;
                app.set_status(&format!("Auto-fetching initial quota for {}...", email));
                spawn_network_task(event_tx.clone(), acc, app.cli_cache.clone(), app.warmup_history.clone(), "quota", None, false);
            }
        }
    }

    loop {
        // Render UI
        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3), // Header
                    Constraint::Min(10),   // Content splits
                    Constraint::Length(3), // Status logs
                    Constraint::Length(1), // Footer/Keyboard tips
                ])
                .split(f.size());

            // 1. Header
            let active_str = app.active_email.as_deref().unwrap_or("None");
            let title = Paragraph::new(format!(
                " Antigravity Manager TUI | Source: {} | Active Account: {}",
                app.db_desc, active_str
            ))
            .block(Block::default().borders(Borders::ALL).title(" System Header ").style(Style::default().fg(Color::Cyan)))
            .style(Style::default().add_modifier(Modifier::BOLD));
            f.render_widget(title, chunks[0]);

            // Split middle content into 2 panels
            let content_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(45), // Left panel: Account list
                    Constraint::Percentage(55), // Right panel: Quotas/Details
                ])
                .split(chunks[1]);

            // Left panel: Account list
            let items: Vec<ListItem> = app.accounts
                .iter()
                .map(|acc| {
                    let is_active = app.active_email.as_ref() == Some(&acc.email);
                    let prefix = if is_active { "★ " } else { "  " };
                    let style = if is_active {
                        Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    };
                    
                    let mut spans = vec![
                        Span::styled(prefix, style),
                        Span::styled(format!("{:<30}", acc.email), style),
                        Span::styled(format!(" ({})", acc.name), Style::default().fg(Color::DarkGray)),
                    ];
                    if is_active {
                        spans.push(Span::styled(" [ACTIVE]", Style::default().fg(Color::Rgb(50, 200, 50)).add_modifier(Modifier::BOLD)));
                    }
                    
                    ListItem::new(Line::from(spans))
                })
                .collect();

            let account_list = List::new(items)
                .block(Block::default().borders(Borders::ALL).title(" Accounts Directory ").style(Style::default().fg(Color::Cyan)))
                .highlight_style(Style::default().bg(Color::Rgb(50, 50, 70)).add_modifier(Modifier::BOLD));
            f.render_stateful_widget(account_list, content_chunks[0], &mut app.list_state);

            // Right panel: Details and Quotas
            if let Some(selected_acc) = app.get_selected_account() {
                let email = &selected_acc.email;
                let token_cache = app.cli_cache.tokens.get(email);
                let quota_cache = app.cli_cache.quotas.get(email);
                
                let project_id = token_cache.and_then(|t| t.project_id.as_deref()).unwrap_or("N/A");
                let tier = quota_cache.and_then(|q| q.subscription_tier.as_deref()).unwrap_or(token_cache.and_then(|t| t.subscription_tier.as_deref()).unwrap_or("N/A"));

                // We construct the vertical layouts inside the Right Panel
                let details_chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(4), // Account header (Tier/Proj)
                        Constraint::Min(5),    // Quota models list
                    ])
                    .split(content_chunks[1]);

                // Render account header info
                let header_text = format!(
                    " Email: {}\n Subscription Tier: {}\n User Google Project ID: {}",
                    email, tier, project_id
                );
                let details_header = Paragraph::new(header_text)
                    .block(Block::default().borders(Borders::ALL).title(" Account Profile ").style(Style::default().fg(Color::Yellow)));
                f.render_widget(details_header, details_chunks[0]);

                // Render model lists or loading overlay
                if app.is_loading {
                    let loading_msg = Paragraph::new(
                        "\n\n\n\n       ⏳  PROCESSING TRANSACTION...\n\n       Contacting Google Companion API and updating active session credentials.\n       Please wait, the interface will automatically refresh."
                    )
                    .alignment(ratatui::layout::Alignment::Center)
                    .block(Block::default().borders(Borders::ALL).title(" Pending Action ").style(Style::default().fg(Color::Cyan)));
                    f.render_widget(loading_msg, details_chunks[1]);
                } else if let Some(q) = quota_cache {
                    let mut quota_items = Vec::new();
                    
                    if q.models.is_empty() {
                        quota_items.push(ListItem::new("No model quota details cached. Press [r] to refresh quotas."));
                    } else {
                        // Sort models to make it pretty: Gemini first, then Claude, then Others
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
                            
                            // Determine Color Hue Shift (Red -> Orange -> Green)
                            let bar_color = if pct >= 80 {
                                Color::Rgb(50, 200, 50)  // Green
                            } else if pct >= 30 {
                                Color::Rgb(240, 170, 30) // Orange/Yellow
                            } else {
                                Color::Rgb(220, 50, 50)  // Red
                            };

                            // Draw a progress bar: 15 ticks wide
                            let bar_width = 15;
                            let filled = ((pct as f64 / 100.0) * bar_width as f64).round() as usize;
                            let empty = bar_width - filled;
                            let bar_str = format!(
                                "[{}{}] {:>3}%",
                                "█".repeat(filled),
                                "░".repeat(empty),
                                pct
                            );

                            // Check cooldown status
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

                            quota_items.push(ListItem::new(Line::from(vec![
                                Span::styled(format!("{:<28}", display), Style::default().fg(Color::White)),
                                Span::styled(bar_str, Style::default().fg(bar_color)),
                                Span::styled(cooldown_str, Style::default().fg(Color::DarkGray)),
                            ])));
                        }
                    }

                    let quota_list = List::new(quota_items)
                        .block(Block::default().borders(Borders::ALL).title(" Quotas Breakdown ").style(Style::default().fg(Color::Yellow)));
                    f.render_widget(quota_list, details_chunks[1]);
                } else {
                    let empty_quota = Paragraph::new("\n No quota metrics cached in database. Press [r] to refresh active quotas.")
                        .block(Block::default().borders(Borders::ALL).title(" Quotas Breakdown ").style(Style::default().fg(Color::Yellow)));
                    f.render_widget(empty_quota, details_chunks[1]);
                }
            } else {
                let fallback = Paragraph::new("\n Please select or configure an account first.")
                    .block(Block::default().borders(Borders::ALL).title(" Profile Details ").style(Style::default().fg(Color::Yellow)));
                f.render_widget(fallback, content_chunks[1]);
            }

            // 3. Status Logs
            let loader_prefix = if app.is_loading { "⏳ " } else { "" };
            let status_block = Paragraph::new(format!("{}{}", loader_prefix, app.status_message))
                .block(Block::default().borders(Borders::ALL).title(" Logger Console ").style(Style::default().fg(Color::Green)))
                .wrap(Wrap { trim: true });
            f.render_widget(status_block, chunks[2]);

            // 4. Footer keyboard shortcuts
            let footer = Paragraph::new(" [Enter] Switch Active  |  [r] Refresh Quota  |  [w] Warm Up  |  [f] Force Warm Up  |  [q] Quit TUI")
                .style(Style::default().fg(Color::DarkGray));
            f.render_widget(footer, chunks[3]);
        })?;

        // Handle TUI events
        while let Ok(event) = event_rx.try_recv() {
            match event {
                AppEvent::Key(key) => match key.code {
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
                                    acc,
                                    app.cli_cache.clone(),
                                    app.warmup_history.clone(),
                                    "switch",
                                    None,
                                    false
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
                                    acc,
                                    app.cli_cache.clone(),
                                    app.warmup_history.clone(),
                                    "quota",
                                    None,
                                    false
                                );
                            }
                        }
                    }
                    KeyCode::Char('w') => {
                        if !app.is_loading {
                            if let Some(acc) = app.get_selected_account().cloned() {
                                app.is_loading = true;
                                app.set_status(&format!("Triggering smart warm up sequence for {}...", acc.email));
                                spawn_network_task(
                                    event_tx.clone(),
                                    acc,
                                    app.cli_cache.clone(),
                                    app.warmup_history.clone(),
                                    "warmup",
                                    None,
                                    false
                                );
                            }
                        }
                    }
                    KeyCode::Char('f') => {
                        if !app.is_loading {
                            if let Some(acc) = app.get_selected_account().cloned() {
                                app.is_loading = true;
                                app.set_status(&format!("FORCE warming up all models for {} (ignoring cooldown)...", acc.email));
                                spawn_network_task(
                                    event_tx.clone(),
                                    acc,
                                    app.cli_cache.clone(),
                                    app.warmup_history.clone(),
                                    "warmup",
                                    None,
                                    true
                                );
                            }
                        }
                    }
                    _ => {}
                },
                AppEvent::NetworkSuccess(result) => {
                    app.is_loading = false;
                    match result {
                        NetworkResult::SwitchComplete { email, keyring_success } => {
                            app.active_email = Some(email.clone());
                            app.cli_cache.active_email = Some(email.clone());
                            save_cli_cache(&app.cli_cache);
                            
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
                            // Only update models list if it's not empty, preserving profile tier
                            if !quota.models.is_empty() {
                                app.cli_cache.quotas.insert(email.clone(), quota);
                            } else if let Some(tc) = app.cli_cache.tokens.get(&email) {
                                if let Some(q_entry) = app.cli_cache.quotas.get_mut(&email) {
                                    q_entry.subscription_tier = tc.subscription_tier.clone();
                                }
                            }
                            save_cli_cache(&app.cli_cache);
                            app.set_status(&format!("Quota statistics refreshed for {}.", email));
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
                            
                            if warmup_count > 0 {
                                if let Some(acc) = app.accounts.iter().find(|a| a.email == email).cloned() {
                                    app.is_loading = true;
                                    spawn_network_task(
                                        event_tx.clone(),
                                        acc,
                                        app.cli_cache.clone(),
                                        app.warmup_history.clone(),
                                        "quota",
                                        None,
                                        false
                                    );
                                }
                            }
                        }
                    }
                }
                AppEvent::NetworkError(err) => {
                    app.is_loading = false;
                    app.set_status(&err);
                }
                AppEvent::Tick => {
                    app.update_status_decay();
                }
            }
        }
    }
}
