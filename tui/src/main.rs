use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event as CEvent, KeyCode, KeyEvent},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
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

// Redirect Port for Local Auth listener
const OAUTH_PORT: u16 = 14210;

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
struct QuotaBucket {
    bucket_id: String,
    window: String,
    remaining_fraction: f64,
    reset_time: String,
    display_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct QuotaGroup {
    display_name: String,
    buckets: Vec<QuotaBucket>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct QuotaData {
    subscription_tier: Option<String>,
    models: Vec<ModelQuota>,
    #[serde(default)]
    quota_groups: Option<Vec<QuotaGroup>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CliCache {
    active_email: Option<String>,
    #[serde(default)]
    tokens: HashMap<String, TokenCache>,
    #[serde(default)]
    quotas: HashMap<String, QuotaData>,
}

#[derive(Clone, PartialEq)]
enum InputMode {
    Normal,
    AddAccount {
        email: String,
        refresh_token: String,
        active_field: usize, // 0 for Email, 1 for Refresh Token
        error_message: Option<String>,
    },
    OAuthLogin {
        auth_url: String,
    },
}

enum AddAccountAction {
    Cancel,
    CycleField,
    InputChar(char),
    Backspace,
    Submit,
}

enum AppEvent {
    Key(KeyEvent),
    Tick,
    Progress(String),
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
    AddAccountComplete {
        new_account: Account,
    },
}

struct App {
    accounts: Vec<Account>,
    db_path: PathBuf,
    db_desc: String,
    active_email: Option<String>,
    list_state: ListState,
    cli_cache: CliCache,
    warmup_history: HashMap<String, i64>,
    status_message: String,
    status_time: Option<Instant>,
    is_loading: bool,
    input_mode: InputMode,
}

impl App {
    fn new(accounts: Vec<Account>, db_path: PathBuf, db_desc: String, active: Option<String>, cache: CliCache, history: HashMap<String, i64>) -> Self {
        let mut list_state = ListState::default();
        if !accounts.is_empty() {
            list_state.select(Some(0));
        }
        
        Self {
            accounts,
            db_path,
            db_desc,
            active_email: active,
            list_state,
            cli_cache: cache,
            warmup_history: history,
            status_message: "Welcome to Antigravity TUI Manager!".to_string(),
            status_time: Some(Instant::now()),
            is_loading: false,
            input_mode: InputMode::Normal,
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

fn format_countdown(reset_time_str: &str) -> Option<String> {
    if reset_time_str.is_empty() {
        return None;
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(reset_time_str) {
        let now = chrono::Utc::now().with_timezone(&dt.timezone());
        let duration = dt.signed_duration_since(now);
        let secs = duration.num_seconds();
        if secs <= 0 {
            return Some("ready".to_string());
        }
        let days = duration.num_days();
        let h = duration.num_hours() % 24;
        let m = duration.num_minutes() % 60;
        let s = secs % 60;
        if days > 0 {
            Some(format!("{}d {}h", days, h))
        } else if h > 0 {
            Some(format!("{}h {}m", h, m))
        } else if m > 0 {
            Some(format!("{}m {}s", m, s))
        } else {
            Some(format!("{}s", s))
        }
    } else {
        None
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
fn load_accounts_list() -> (Vec<Account>, PathBuf, String) {
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
                        return (accounts, path.clone(), format!("Backup file '{}'", path.file_name().unwrap().to_string_lossy()));
                    }
                }
            }
        }
    }

    let data_dir = get_data_dir();
    let index_path = data_dir.join("accounts.json");
    if index_path.exists() {
        if let Ok(content) = fs::read_to_string(&index_path) {
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
                                    source: "Tauri official database".to_string(),
                                    id: Some(acc.id),
                                });
                            }
                        }
                    }
                }
                if !accounts.is_empty() {
                    return (accounts, index_path.clone(), "Tauri official database".to_string());
                }
            }
        }
    }

    (Vec::new(), PathBuf::from(""), "No account source found".to_string())
}

fn get_active_email(accounts: &[Account]) -> Option<String> {
    let cache = load_cli_cache();
    if let Some(ref active) = cache.active_email {
        if accounts.iter().any(|a| a.email.to_lowercase() == active.to_lowercase()) {
            return Some(active.clone());
        }
    }
    
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
        let secret_tool_check = subprocess_exists("secret-tool");
        if !secret_tool_check {
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
    
    true
}

// Writes OAuth credentials directly to active files of Antigravity CLI and IDEs to sync active sessions
fn write_oauth_token_file(access_token: &str, refresh_token: &str, expiry_timestamp: i64) {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    
    let dirs_to_sync = [
        home.join(".gemini").join("antigravity-cli"),
        home.join(".gemini").join("antigravity"),
        home.join(".gemini").join("antigravity-ide"),
    ];
    
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
    
    if let Ok(content) = serde_json::to_string(&payload) {
        for cli_dir in dirs_to_sync.iter() {
            if cli_dir.exists() {
                let token_path = cli_dir.join("antigravity-oauth-token");
                let _ = fs::write(&token_path, &content);
            }
        }
    }
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

// Browser Launcher Helper
fn open_browser(url: &str) -> bool {
    let system = std::env::consts::OS;
    if system == "linux" {
        if subprocess_exists("termux-open") {
            std::process::Command::new("termux-open")
                .arg(url)
                .spawn()
                .is_ok()
        } else {
            std::process::Command::new("xdg-open")
                .arg(url)
                .spawn()
                .is_ok()
        }
    } else if system == "macos" {
        std::process::Command::new("open")
            .arg(url)
            .spawn()
            .is_ok()
    } else if system == "windows" {
        std::process::Command::new("cmd")
            .args(["/C", "start", url])
            .spawn()
            .is_ok()
    } else {
        false
    }
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
                                    || name.contains("flash")
                                    || name.contains("lite")
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

async fn async_fetch_quota_summary(access_token: &str, project_id: Option<&str>) -> Option<Vec<QuotaGroup>> {
    let client = reqwest::Client::new();
    let payload = if let Some(pid) = project_id {
        json!({ "project": pid })
    } else {
        json!({})
    };
    
    let urls = [
        "https://daily-cloudcode-pa.sandbox.googleapis.com/v1internal:retrieveUserQuotaSummary",
        "https://daily-cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary",
        "https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary",
    ];
    
    for url in urls.iter() {
        let res = client
            .post(*url)
            .header("Authorization", format!("Bearer {}", access_token))
            .header("Content-Type", "application/json")
            .header("User-Agent", "vscode/1.X.X (Antigravity/4.3.0)")
            .json(&payload)
            .send()
            .await;
            
        if let Ok(resp) = res {
            if resp.status().is_success() {
                #[derive(Deserialize)]
                struct RawBucket {
                    #[serde(rename = "bucketId")]
                    bucket_id: Option<String>,
                    window: Option<String>,
                    #[serde(rename = "remainingFraction")]
                    remaining_fraction: Option<f64>,
                    #[serde(rename = "resetTime")]
                    reset_time: Option<String>,
                    #[serde(rename = "displayName")]
                    display_name: Option<String>,
                }
                #[derive(Deserialize)]
                struct RawGroup {
                    #[serde(rename = "displayName")]
                    display_name: Option<String>,
                    buckets: Option<Vec<RawBucket>>,
                }
                #[derive(Deserialize)]
                struct RawResponse {
                    groups: Option<Vec<RawGroup>>,
                }
                
                if let Ok(data) = resp.json::<RawResponse>().await {
                    if let Some(raw_groups) = data.groups {
                        let mut groups = Vec::new();
                        for rg in raw_groups {
                            let mut buckets = Vec::new();
                            if let Some(raw_buckets) = rg.buckets {
                                for rb in raw_buckets {
                                    buckets.push(QuotaBucket {
                                        bucket_id: rb.bucket_id.unwrap_or_default(),
                                        window: rb.window.unwrap_or_default(),
                                        remaining_fraction: rb.remaining_fraction.unwrap_or(0.0),
                                        reset_time: rb.reset_time.unwrap_or_default(),
                                        display_name: rb.display_name,
                                    });
                                }
                            }
                            groups.push(QuotaGroup {
                                display_name: rg.display_name.unwrap_or_default(),
                                buckets,
                            });
                        }
                        return Some(groups);
                    }
                }
            }
        }
    }
    None
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
        if tc.expiry_timestamp > now + 900 {
            return Some((tc.access_token.clone(), tc.project_id.clone()));
        }
    }
    
    if let Some((new_tok, new_exp)) = async_refresh_token(refresh_token.to_string()).await {
        let (proj_id, tier) = async_fetch_project_and_tier(&new_tok).await;
        cli_cache.tokens.insert(email.to_string(), TokenCache {
            access_token: new_tok.clone(),
            expiry_timestamp: new_exp,
            project_id: proj_id.clone(),
            subscription_tier: tier,
        });
        save_cli_cache(cli_cache);
        
        // Sync refreshed active account token immediately to system files/keyrings
        if cli_cache.active_email.as_ref() == Some(&email.to_string()) {
            write_to_system_keyring(email, &new_tok, refresh_token, new_exp);
            write_oauth_token_file(&new_tok, refresh_token, new_exp);
        }
        
        Some((new_tok, proj_id))
    } else {
        None
    }
}

// Write a new account directly to the database file
fn add_account_to_db(path: &Path, email: &str, refresh_token: &str) -> Result<Account, String> {
    if !path.exists() {
        return Err("Database file does not exist.".to_string());
    }
    
    let content = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let cleaned = content.replace("\u{feff}", "").replace('\x00', "");
    
    if let Ok(mut val) = serde_json::from_str::<serde_json::Value>(&cleaned) {
        let name = email.split('@').next().unwrap_or("").to_string();
        
        if let Some(arr) = val.as_array_mut() {
            if arr.iter().any(|x| x.get("email").and_then(|e| e.as_str()) == Some(email)) {
                return Err("Account email already exists in database.".to_string());
            }
            arr.push(json!({
                "email": email,
                "refresh_token": refresh_token
            }));
            let new_content = serde_json::to_string_pretty(&val).map_err(|e| e.to_string())?;
            fs::write(path, new_content).map_err(|e| e.to_string())?;
            return Ok(Account {
                email: email.to_string(),
                refresh_token: refresh_token.to_string(),
                name: name.clone(),
                source: format!("backup ({})", path.file_name().unwrap().to_string_lossy()),
                id: None,
            });
        } else if let Some(obj) = val.as_object_mut() {
            let accounts_arr = obj.get_mut("accounts").and_then(|a| a.as_array_mut());
            if let Some(arr) = accounts_arr {
                if arr.iter().any(|x| x.get("email").and_then(|e| e.as_str()) == Some(email)) {
                    return Err("Account email already exists in database.".to_string());
                }
                
                let new_id = Uuid::new_v4().to_string();
                arr.push(json!({
                    "id": new_id,
                    "email": email,
                    "name": name.clone()
                }));
                
                let data_dir = path.parent().unwrap();
                let acc_dir = data_dir.join("accounts");
                let _ = fs::create_dir_all(&acc_dir);
                let acc_path = acc_dir.join(format!("{}.json", new_id));
                let acc_details = json!({
                    "id": new_id,
                    "email": email,
                    "name": name.clone(),
                    "token": {
                        "refresh_token": refresh_token
                    }
                });
                let acc_content = serde_json::to_string_pretty(&acc_details).map_err(|e| e.to_string())?;
                fs::write(acc_path, acc_content).map_err(|e| e.to_string())?;
                
                let new_content = serde_json::to_string_pretty(&val).map_err(|e| e.to_string())?;
                fs::write(path, new_content).map_err(|e| e.to_string())?;
                
                return Ok(Account {
                    email: email.to_string(),
                    refresh_token: refresh_token.to_string(),
                    name: name.clone(),
                    source: "Tauri official database".to_string(),
                    id: Some(new_id),
                });
            }
        }
    }
    
    Err("Unknown/Unsupported database format.".to_string())
}

// Listen for OAuth Code from Google redirect on local loopback TCP port
async fn listen_for_oauth_code(port: u16) -> Result<String, String> {
    use tokio::net::TcpListener;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let addr = format!("127.0.0.1:{}", port);
    let listener = TcpListener::bind(&addr).await.map_err(|e| e.to_string())?;
    
    let timeout_duration = Duration::from_secs(120);
    let start_time = Instant::now();
    
    loop {
        if start_time.elapsed() > timeout_duration {
            return Err("OAuth login timed out (120 seconds).".to_string());
        }
        
        if let Ok(Ok((mut stream, _))) = tokio::time::timeout(Duration::from_millis(500), listener.accept()).await {
            let mut buffer = [0; 2048];
            if let Ok(n) = stream.read(&mut buffer).await {
                let request = String::from_utf8_lossy(&buffer[..n]);
                
                if let Some(line) = request.lines().next() {
                    if line.starts_with("GET ") {
                        let parts: Vec<&str> = line.split_whitespace().collect();
                        if parts.len() > 1 {
                            let path = parts[1];
                            let mut code = None;
                            if let Some(query_idx) = path.find('?') {
                                let query = &path[query_idx + 1..];
                                for pair in query.split('&') {
                                    let mut kv = pair.split('=');
                                    if let (Some(k), Some(v)) = (kv.next(), kv.next()) {
                                        if k == "code" {
                                            code = Some(v.to_string());
                                        }
                                    }
                                }
                            }
                            
                            if let Some(auth_code) = code {
                                let response = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n\
                                <html>\
                                <head><style>body { font-family: sans-serif; background: #121214; color: #e1e1e6; text-align: center; padding-top: 50px; }</style></head>\
                                <body>\
                                  <h2>Antigravity Manager</h2>\
                                  <p style=\"color: #4ade80; font-weight: bold;\">✓ Authentication successful!</p>\
                                  <p>You can now close this browser tab and return to the terminal.</p>\
                                </body>\
                                </html>";
                                let _ = stream.write_all(response.as_bytes()).await;
                                let _ = stream.flush().await;
                                return Ok(auth_code);
                            }
                        }
                    }
                }
                
                let response = "HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\nBad Request";
                let _ = stream.write_all(response.as_bytes()).await;
            }
        }
    }
}

// Exchange Google OAuth Code for Refresh & Access tokens
async fn exchange_oauth_code(code: &str, port: u16) -> Result<(String, String, i64), String> {
    let client = reqwest::Client::new();
    let params = [
        ("client_id", CLIENT_ID),
        ("client_secret", CLIENT_SECRET),
        ("code", code),
        ("grant_type", "authorization_code"),
        ("redirect_uri", &format!("http://localhost:{}", port)),
    ];
    
    let resp = client
        .post(TOKEN_URL)
        .header("User-Agent", "vscode/1.X.X (Antigravity/4.3.0)")
        .form(&params)
        .send()
        .await
        .map_err(|e| e.to_string())?;
        
    if resp.status().is_success() {
        #[derive(Deserialize)]
        struct TokenResponse {
            access_token: String,
            refresh_token: String,
            expires_in: i64,
        }
        let data = resp.json::<TokenResponse>().await.map_err(|e| e.to_string())?;
        let expiry = chrono::Utc::now().timestamp() + data.expires_in;
        Ok((data.access_token, data.refresh_token, expiry))
    } else {
        Err(format!("OAuth exchange returned status {}: {}", resp.status(), resp.text().await.unwrap_or_default()))
    }
}

// Query Userinfo endpoint to get email address
async fn fetch_user_email(access_token: &str) -> Result<String, String> {
    let client = reqwest::Client::new();
    let resp = client
        .get("https://www.googleapis.com/oauth2/v2/userinfo")
        .header("Authorization", format!("Bearer {}", access_token))
        .header("User-Agent", "vscode/1.X.X (Antigravity/4.3.0)")
        .send()
        .await
        .map_err(|e| e.to_string())?;
        
    if resp.status().is_success() {
        #[derive(Deserialize)]
        struct UserInfo {
            email: String,
        }
        let data = resp.json::<UserInfo>().await.map_err(|e| e.to_string())?;
        Ok(data.email)
    } else {
        Err(format!("Google UserInfo returned status {}: {}", resp.status(), resp.text().await.unwrap_or_default()))
    }
}

// Background network task runner (for TUI)
fn spawn_network_task(
    event_tx: mpsc::UnboundedSender<AppEvent>,
    account: Option<Account>,
    accounts_all: Vec<Account>,
    mut cli_cache: CliCache,
    warmup_history: HashMap<String, i64>,
    action: &'static str,
    target_model: Option<String>,
    force: bool,
    new_acc_details: Option<(String, String, PathBuf)>, // (email, token, db_path) for adding
) {
    tokio::spawn(async move {
        let now = chrono::Utc::now().timestamp();
        
        match action {
            "quota_all" => {
                let mut total_refreshed = 0;
                let count_accs = accounts_all.len();
                
                for (idx, acc) in accounts_all.iter().enumerate() {
                    let email = &acc.email;
                    let _ = event_tx.send(AppEvent::Progress(format!("[{}/{}] Reloading quota for {}...", idx + 1, count_accs, email)));
                    
                    let token_info = ensure_valid_token(email, &acc.refresh_token, &mut cli_cache).await;
                    if token_info.is_none() {
                        continue;
                    }
                    let (access_token, resolved_proj_id) = token_info.unwrap();
                    
                    let summary = async_fetch_quota_summary(&access_token, resolved_proj_id.as_deref()).await;
                    if let Ok(models) = async_fetch_quota(&access_token, resolved_proj_id.as_deref()).await {
                        let tier = cli_cache.tokens.get(email).and_then(|d| d.subscription_tier.clone());
                        let q = QuotaData {
                            subscription_tier: tier,
                            models,
                            quota_groups: summary,
                        };
                        
                        // Send incremental update back to TUI right away
                        let _ = event_tx.send(AppEvent::NetworkSuccess(NetworkResult::QuotaRefreshed {
                            email: email.clone(),
                            quota: q,
                            project_id: resolved_proj_id,
                        }));
                        total_refreshed += 1;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                
                let _ = event_tx.send(AppEvent::Progress(format!("✓ Quota reload complete. Refreshed {}/{} accounts.", total_refreshed, count_accs)));
            }
            "oauth_login" => {
                let db_path = new_acc_details.unwrap().2;
                let _ = event_tx.send(AppEvent::Progress("Starting local OAuth listener on loopback...".to_string()));
                
                match listen_for_oauth_code(OAUTH_PORT).await {
                    Ok(auth_code) => {
                        let _ = event_tx.send(AppEvent::Progress("Exchanging code for tokens...".to_string()));
                        match exchange_oauth_code(&auth_code, OAUTH_PORT).await {
                            Ok((access_token, refresh_token, expiry)) => {
                                let _ = event_tx.send(AppEvent::Progress("Fetching user email profile...".to_string()));
                                match fetch_user_email(&access_token).await {
                                    Ok(email) => {
                                        let _ = event_tx.send(AppEvent::Progress(format!("Verifying project subscription for {}...", email)));
                                        let (proj_id, tier) = async_fetch_project_and_tier(&access_token).await;
                                        
                                        let _ = event_tx.send(AppEvent::Progress("Adding account to database...".to_string()));
                                        match add_account_to_db(&db_path, &email, &refresh_token) {
                                            Ok(new_acc) => {
                                                cli_cache.tokens.insert(email.clone(), TokenCache {
                                                    access_token,
                                                    expiry_timestamp: expiry,
                                                    project_id: proj_id,
                                                    subscription_tier: tier,
                                                });
                                                save_cli_cache(&cli_cache);
                                                
                                                let _ = event_tx.send(AppEvent::NetworkSuccess(NetworkResult::AddAccountComplete {
                                                    new_account: new_acc,
                                                }));
                                            }
                                            Err(e) => {
                                                let _ = event_tx.send(AppEvent::NetworkError(format!("Add account failed: {}", e)));
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        let _ = event_tx.send(AppEvent::NetworkError(format!("Email fetch failed: {}", e)));
                                    }
                                }
                            }
                            Err(e) => {
                                let _ = event_tx.send(AppEvent::NetworkError(format!("OAuth exchange failed: {}", e)));
                            }
                        }
                    }
                    Err(e) => {
                        let _ = event_tx.send(AppEvent::NetworkError(format!("OAuth listener error: {}", e)));
                    }
                }
            }
            "add_account" => {
                let (email, rt, db_path) = new_acc_details.unwrap();
                let _ = event_tx.send(AppEvent::Progress(format!("Validating refresh token for {}...", email)));
                
                if let Some((access_token, expiry)) = async_refresh_token(rt.clone()).await {
                    let _ = event_tx.send(AppEvent::Progress(format!("Verifying project settings for {}...", email)));
                    let (proj_id, tier) = async_fetch_project_and_tier(&access_token).await;
                    
                    let _ = event_tx.send(AppEvent::Progress("Writing credentials to database...".to_string()));
                    match add_account_to_db(&db_path, &email, &rt) {
                        Ok(new_acc) => {
                            cli_cache.tokens.insert(email.clone(), TokenCache {
                                access_token,
                                expiry_timestamp: expiry,
                                project_id: proj_id,
                                subscription_tier: tier,
                            });
                            save_cli_cache(&cli_cache);
                            
                            let _ = event_tx.send(AppEvent::NetworkSuccess(NetworkResult::AddAccountComplete {
                                new_account: new_acc,
                            }));
                        }
                        Err(e) => {
                            let _ = event_tx.send(AppEvent::NetworkError(format!("Add account failed: {}", e)));
                        }
                    }
                } else {
                    let _ = event_tx.send(AppEvent::NetworkError("Validation failed: Invalid refresh token.".to_string()));
                }
            }
            "switch" => {
                let account = account.unwrap();
                let email = account.email.clone();
                let _ = event_tx.send(AppEvent::Progress(format!("Connecting session for {}...", email)));
                
                let token_info = ensure_valid_token(&email, &account.refresh_token, &mut cli_cache).await;
                if token_info.is_none() {
                    let _ = event_tx.send(AppEvent::NetworkError(format!("Failed to refresh credentials for {}", email)));
                    return;
                }
                let (access_token, _proj_id) = token_info.unwrap();
                let expiry = cli_cache.tokens.get(&email).map(|d| d.expiry_timestamp).unwrap_or(now + 3600);
                
                let keyring_success = write_to_system_keyring(&email, &access_token, &account.refresh_token, expiry);
                write_oauth_token_file(&access_token, &account.refresh_token, expiry);
                
                let _ = event_tx.send(AppEvent::NetworkSuccess(NetworkResult::SwitchComplete {
                    email: email.clone(),
                    keyring_success,
                }));
                
                let details = cli_cache.tokens.get(&email).cloned();
                if let Some(fd) = details {
                    let _ = event_tx.send(AppEvent::NetworkSuccess(NetworkResult::QuotaRefreshed {
                        email: account.email,
                        quota: QuotaData { subscription_tier: fd.subscription_tier, models: Vec::new(), quota_groups: None },
                        project_id: fd.project_id,
                    }));
                }
            }
            "quota" => {
                let account = account.unwrap();
                let email = account.email.clone();
                
                let token_info = ensure_valid_token(&email, &account.refresh_token, &mut cli_cache).await;
                if token_info.is_none() {
                    let _ = event_tx.send(AppEvent::NetworkError(format!("Failed to refresh credentials for {}", email)));
                    return;
                }
                let (access_token, resolved_proj_id) = token_info.unwrap();
                
                let summary = async_fetch_quota_summary(&access_token, resolved_proj_id.as_deref()).await;
                match async_fetch_quota(&access_token, resolved_proj_id.as_deref()).await {
                    Ok(models) => {
                        let tier = cli_cache.tokens.get(&email).and_then(|d| d.subscription_tier.clone());
                        let q = QuotaData {
                            subscription_tier: tier,
                            models,
                            quota_groups: summary,
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
                let account = account.unwrap();
                let email = account.email.clone();
                
                let token_info = ensure_valid_token(&email, &account.refresh_token, &mut cli_cache).await;
                if token_info.is_none() {
                    let _ = event_tx.send(AppEvent::NetworkError(format!("Failed to refresh credentials for {}", email)));
                    return;
                }
                let (access_token, resolved_proj_id) = token_info.unwrap();
                
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
                    
                    let token_info = ensure_valid_token(email, &acc.refresh_token, &mut cli_cache).await;
                    if token_info.is_none() {
                        total_logs.push(format!("Skipped {}: Token refresh failed.", email));
                        total_skipped += 1;
                        continue;
                    }
                    let (access_token, resolved_proj_id) = token_info.unwrap();
                    
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
    println!("=============================================================");
    println!("{:<3} | {:<6} | {:<32} | {:<20}", "#", "Active", "Email", "Name");
    println!("-------------------------------------------------------------");
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
    
    if let Some((access_token, _project_id)) = ensure_valid_token(email, &acc.refresh_token, &mut cache).await {
        let expiry = cache.tokens.get(email).map(|t| t.expiry_timestamp).unwrap_or(0);
        let keyring_success = write_to_system_keyring(email, &access_token, &acc.refresh_token, expiry);
        write_oauth_token_file(&access_token, &acc.refresh_token, expiry);
        
        cache.active_email = Some(email.clone());
        save_cli_cache(&cache);
        
        let data_dir = get_data_dir();
        let index_path = data_dir.join("accounts.json");
        if index_path.exists() {
            if let Some(ref acc_id) = acc.id {
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
    let mut cache = load_cli_cache();

    if identifier == Some("all") {
        if refresh {
            println!("Refreshing quotas for ALL configured accounts sequentially...");
            let count_accs = accounts.len();
            for (idx, acc) in accounts.iter().enumerate() {
                let email = &acc.email;
                println!("[{}/{}] Fetching quota for {}...", idx + 1, count_accs, email);
                
                let (access_token, mut project_id) = match ensure_valid_token(email, &acc.refresh_token, &mut cache).await {
                    Some(t) => t,
                    None => {
                        eprintln!("✗ Error: Failed to validate credentials for {}. Skipping.", email);
                        continue;
                    }
                };
                
                let (api_proj, tier) = async_fetch_project_and_tier(&access_token).await;
                if api_proj.is_some() {
                    project_id = api_proj.clone();
                    if let Some(tc) = cache.tokens.get_mut(email) {
                        tc.project_id = api_proj;
                        tc.subscription_tier = tier.clone();
                    }
                }
                
                let summary = async_fetch_quota_summary(&access_token, project_id.as_deref()).await;
                match async_fetch_quota(&access_token, project_id.as_deref()).await {
                    Ok(models) => {
                        cache.quotas.insert(email.clone(), QuotaData {
                            subscription_tier: tier.or_else(|| cache.tokens.get(email).and_then(|t| t.subscription_tier.clone())),
                            models,
                            quota_groups: summary,
                        });
                        save_cli_cache(&cache);
                        println!("✓ Quota updated.");
                    }
                    Err(e) => {
                        eprintln!("✗ Error: {}", e);
                    }
                }
            }
            println!("Quotas refresh complete.");
        }
        
        for acc in accounts {
            let email = &acc.email;
            if let Some(q) = cache.quotas.get(email) {
                println!("\nQuota for {}:", email);
                let proj = cache.tokens.get(email).and_then(|t| t.project_id.as_deref()).unwrap_or("N/A");
                println!("Subscription Tier: {} | Project: {}", q.subscription_tier.as_deref().unwrap_or("N/A"), proj);
                println!("--------------------------------------------------");
                for m in &q.models {
                    let display = m.display_name.as_deref().unwrap_or(&m.name);
                    println!("  {:<35} : {}%", display, m.percentage);
                }
            } else {
                println!("\nQuota for {}: No cached metrics. Run with '--refresh'.", email);
            }
        }
        return;
    }

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
        
        let summary = async_fetch_quota_summary(&access_token, project_id.as_deref()).await;
        match async_fetch_quota(&access_token, project_id.as_deref()).await {
            Ok(models) => {
                cache.quotas.insert(target_email.to_string(), QuotaData {
                    subscription_tier: tier.or_else(|| cache.tokens.get(target_email).and_then(|t| t.subscription_tier.clone())),
                    models,
                    quota_groups: summary,
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
    if identifier == Some("all") {
        println!("Running Warm Up cycle for ALL configured accounts sequentially...");
        let mut cache = load_cli_cache();
        let mut history = load_warmup_history();
        let now = chrono::Utc::now().timestamp();
        let count_accs = accounts.len();
        
        for (idx, acc) in accounts.iter().enumerate() {
            let email = &acc.email;
            println!("\n[{}/{}] Processing account: {}...", idx + 1, count_accs, email);
            
            let (access_token, mut project_id) = match ensure_valid_token(email, &acc.refresh_token, &mut cache).await {
                Some(t) => t,
                None => {
                    eprintln!("✗ Error: Failed to validate credentials for {}. Skipping.", email);
                    continue;
                }
            };
            
            let mut models = cache.quotas.get(email).map(|q| q.models.clone()).unwrap_or_default();
            if models.is_empty() || force {
                let (api_proj, tier) = async_fetch_project_and_tier(&access_token).await;
                if api_proj.is_some() {
                    project_id = api_proj.clone();
                    if let Some(tc) = cache.tokens.get_mut(email) {
                        tc.project_id = api_proj;
                        tc.subscription_tier = tier;
                    }
                }
                if let Ok(m) = async_fetch_quota(&access_token, project_id.as_deref()).await {
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
                println!("✓ All models have remaining usage. No warmup needed.");
                continue;
            }
            
            for m in to_warm {
                let display = m.display_name.as_deref().unwrap_or(&m.name);
                
                if m.name.contains("2.5-") || m.name.contains("2-5-") {
                    continue;
                }
                
                if !force {
                    let key = format!("{}:{}:100", email, m.name);
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
                match async_trigger_warmup(&access_token, &m.name, project_id.as_deref(), email).await {
                    Ok(_) => {
                        println!("✓ Successfully warmed up {}!", display);
                        let key = format!("{}:{}:100", email, m.name);
                        history.insert(key, chrono::Utc::now().timestamp());
                        save_warmup_history(&history);
                    }
                    Err(e) => {
                        println!("✗ Warmup failed for {}: {}", display, e);
                    }
                }
                tokio::time::sleep(Duration::from_millis(800)).await;
            }
        }
        println!("\nWarmup cycle for all accounts completed.");
        return;
    }
    
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
                name: m_name.to_string(),
                percentage: 100,
                display_name: Some(m_name.to_string()),
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

// Centered rect generator helper for rendering popups
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

// ---------------------------------------------------------
// MAIN RUNTIME ORCHESTRATOR
// ---------------------------------------------------------

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (accounts, db_path, db_desc) = load_accounts_list();
    let active_email = get_active_email(&accounts);
    
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        let subcommand = &args[1];
        match subcommand.as_str() {
            "list" => {
                cli_list(&accounts, active_email.as_deref(), &db_desc);
            }
            "switch" => {
                if args.len() < 3 {
                    eprintln!("Usage: agm switch <index/email>");
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
                println!("  agm                   Launch interactive terminal user interface (TUI)");
                println!("  agm list              List configured accounts");
                println!("  agm switch <id>       Switch the active account");
                println!("  agm quota [id] [-r]   Display quotas (use --refresh to update)");
                println!("  agm quota all [-r]    Display/Refresh quotas for ALL accounts");
                println!("  agm warmup [id] [flg] Run warmup cycles (use --model <name> or --force)");
                println!("  agm warmup all        Sequentially warm up ALL configured accounts");
                println!("\nExamples:");
                println!("  agm switch 3");
                println!("  agm quota all --refresh");
                println!("  agm warmup all");
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
            if event::poll(Duration::from_millis(200)).unwrap() {
                if let CEvent::Key(key) = event::read().unwrap() {
                    let _ = tx.send(AppEvent::Key(key));
                }
            }
            let _ = tx.send(AppEvent::Tick);
        }
    });

    let mut app = App::new(accounts, db_path, db_desc, active_email, cache, history);

    if let Some(ref email) = app.active_email {
        if !app.cli_cache.quotas.contains_key(email) && !app.accounts.is_empty() {
            if let Some(acc) = app.accounts.iter().find(|a| a.email == *email).cloned() {
                app.is_loading = true;
                app.set_status(&format!("Auto-fetching initial quota for {}...", email));
                spawn_network_task(event_tx.clone(), Some(acc), Vec::new(), app.cli_cache.clone(), app.warmup_history.clone(), "quota", None, false, None);
            }
        }
    }

    loop {
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

            let active_str = app.active_email.as_deref().unwrap_or("None");
            let title = Paragraph::new(format!(
                " Antigravity Manager TUI | Source: {} | Active Account: {}",
                app.db_desc, active_str
            ))
            .block(Block::default().borders(Borders::ALL).title(" System Header ").style(Style::default().fg(Color::Cyan)))
            .style(Style::default().add_modifier(Modifier::BOLD));
            f.render_widget(title, chunks[0]);

            let content_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(50), // Left panel: Account list & Quota summary
                    Constraint::Percentage(50), // Right panel: Details
                ])
                .split(chunks[1]);

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

                    let gemini_pct_str = match gemini_pct {
                        Some(pct) => format!("G:{}%", pct),
                        None => "G:--".to_string(),
                    };
                    let claude_pct_str = match claude_pct {
                        Some(pct) => format!("C:{}%", pct),
                        None => "C:--".to_string(),
                    };

                    let gemini_style = match gemini_pct {
                        Some(pct) if pct >= 80 => Style::default().fg(Color::Rgb(50, 200, 50)),
                        Some(pct) if pct >= 30 => Style::default().fg(Color::Rgb(240, 170, 30)),
                        Some(_) => Style::default().fg(Color::Rgb(220, 50, 50)),
                        None => Style::default().fg(Color::DarkGray),
                    };

                    let claude_style = match claude_pct {
                        Some(pct) if pct >= 80 => Style::default().fg(Color::Rgb(50, 200, 50)),
                        Some(pct) if pct >= 30 => Style::default().fg(Color::Rgb(240, 170, 30)),
                        Some(_) => Style::default().fg(Color::Rgb(220, 50, 50)),
                        None => Style::default().fg(Color::DarkGray),
                    };

                    let mut weekly_reset = "--".to_string();
                    let mut five_h_reset = "--".to_string();
                    
                    if let Some(groups) = quota_cache.and_then(|q| q.quota_groups.as_ref()) {
                        for group in groups {
                            for bucket in &group.buckets {
                                if bucket.window == "weekly" || bucket.bucket_id.contains("weekly") {
                                    if !bucket.reset_time.is_empty() {
                                        weekly_reset = format_countdown(&bucket.reset_time).unwrap_or_else(|| "--".to_string());
                                    }
                                } else if bucket.window == "5h" || bucket.bucket_id.contains("5h") {
                                    if !bucket.reset_time.is_empty() {
                                        five_h_reset = format_countdown(&bucket.reset_time).unwrap_or_else(|| "--".to_string());
                                    }
                                }
                            }
                        }
                    }

                    let mut spans = vec![
                        Span::styled(prefix, style),
                        Span::styled(format!("{:<20}", acc.email), style),
                        Span::styled(" (", Style::default().fg(Color::DarkGray)),
                        Span::styled(gemini_pct_str, gemini_style),
                        Span::styled(" | ", Style::default().fg(Color::DarkGray)),
                        Span::styled(claude_pct_str, claude_style),
                        Span::styled(")", Style::default().fg(Color::DarkGray)),
                        Span::styled(" [5h:", Style::default().fg(Color::DarkGray)),
                        Span::styled(five_h_reset, Style::default().fg(Color::Rgb(120, 180, 240))),
                        Span::styled(" | Wk:", Style::default().fg(Color::DarkGray)),
                        Span::styled(weekly_reset, Style::default().fg(Color::Rgb(240, 120, 240))),
                        Span::styled("]", Style::default().fg(Color::DarkGray)),
                    ];
                    if is_active {
                        spans.push(Span::styled(" ★", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)));
                    }
                    
                    ListItem::new(Line::from(spans))
                })
                .collect();

            let account_list = List::new(items)
                .block(Block::default().borders(Borders::ALL).title(" Accounts Summary ").style(Style::default().fg(Color::Cyan)))
                .highlight_style(Style::default().bg(Color::Rgb(50, 50, 70)).add_modifier(Modifier::BOLD));
            f.render_stateful_widget(account_list, content_chunks[0], &mut app.list_state);

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
                    Span::styled(" ★ ACTIVE SESSION ", Style::default().bg(Color::Rgb(50, 150, 50)).fg(Color::White).add_modifier(Modifier::BOLD))
                } else {
                    Span::styled(" ○ INACTIVE ", Style::default().fg(Color::DarkGray))
                };
                
                let header_text = vec![
                    Line::from(vec![Span::raw(" Email: "), Span::styled(email, Style::default().add_modifier(Modifier::BOLD))]),
                    Line::from(vec![Span::raw(" Subscription Tier: "), Span::styled(tier, Style::default().fg(Color::Cyan))]),
                    Line::from(vec![Span::raw(" Project ID: "), Span::styled(project_id, Style::default().fg(Color::Yellow))]),
                    Line::from(vec![Span::raw(" Status: "), status_span]),
                ];
                
                let details_header = Paragraph::new(header_text)
                    .block(Block::default().borders(Borders::ALL).title(" Account Profile ").style(Style::default().fg(Color::Yellow)));
                f.render_widget(details_header, details_chunks[0]);

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
                                Color::Rgb(50, 200, 50)  // Green
                            } else if pct >= 30 {
                                Color::Rgb(240, 170, 30) // Orange/Yellow
                            } else {
                                Color::Rgb(220, 50, 50)  // Red
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
                                Span::styled(format!("{:<28}", display), Style::default().fg(Color::White)),
                                Span::styled(bar_str, Style::default().fg(bar_color)),
                                Span::styled(cooldown_str, Style::default().fg(Color::DarkGray)),
                                Span::styled(reset_str, Style::default().fg(Color::Rgb(150, 150, 200))),
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

            let loader_prefix = if app.is_loading { "⏳ " } else { "" };
            let status_block = Paragraph::new(format!("{}{}", loader_prefix, app.status_message))
                .block(Block::default().borders(Borders::ALL).title(" Logger Console ").style(Style::default().fg(Color::Green)))
                .wrap(Wrap { trim: true });
            f.render_widget(status_block, chunks[2]);

            let footer = Paragraph::new(" [Enter] Switch | [r] Refresh Quota | [R] Refresh All | [w] Warm Up | [W] Warm All | [a] Custom | [l] Login | [q] Quit")
                .style(Style::default().fg(Color::DarkGray));
            f.render_widget(footer, chunks[3]);

            if let InputMode::AddAccount { email, refresh_token, active_field, error_message } = &app.input_mode {
                let block = Block::default()
                    .title(" Add Custom Account ")
                    .borders(Borders::ALL)
                    .style(Style::default().bg(Color::Rgb(20, 20, 30)).fg(Color::Cyan));
                
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
                    .style(if *active_field == 0 { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::DarkGray) });
                let email_para = Paragraph::new(email.as_str()).block(email_block);
                f.render_widget(email_para, modal_chunks[0]);

                let token_block = Block::default()
                    .title(" 2. OAuth Refresh Token ")
                    .borders(Borders::ALL)
                    .style(if *active_field == 1 { Style::default().fg(Color::Yellow) } else { Style::default().fg(Color::DarkGray) });
                let token_para = Paragraph::new(refresh_token.as_str()).block(token_block);
                f.render_widget(token_para, modal_chunks[1]);

                if let Some(err) = error_message {
                    let err_para = Paragraph::new(format!("Error: {}", err))
                        .style(Style::default().fg(Color::Rgb(220, 50, 50)).add_modifier(Modifier::BOLD));
                    f.render_widget(err_para, modal_chunks[2]);
                }

                let help_text = Paragraph::new(
                    " [Tab] Switch Fields  |  [Enter] Verify & Add Account  |  [Esc] Cancel Modal\n (The refresh token will be validated with Google prior to saving.)"
                )
                .style(Style::default().fg(Color::DarkGray));
                f.render_widget(help_text, modal_chunks[3]);
            }

            if let InputMode::OAuthLogin { auth_url } = &app.input_mode {
                let block = Block::default()
                    .title(" Google OAuth Authentication ")
                    .borders(Borders::ALL)
                    .style(Style::default().bg(Color::Rgb(20, 20, 30)).fg(Color::Cyan));
                
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
                    .title(" Copy & Paste URL ")
                    .style(Style::default().fg(Color::Yellow));
                let url_para = Paragraph::new(auth_url.as_str())
                    .block(url_block)
                    .wrap(Wrap { trim: false });
                f.render_widget(url_para, modal_chunks[1]);

                let status_desc = Paragraph::new("Status: Awaiting authorization callback from Google loopback listener...")
                    .style(Style::default().fg(Color::Rgb(50, 180, 240)).add_modifier(Modifier::BOLD));
                f.render_widget(status_desc, modal_chunks[2]);

                let footer_help = Paragraph::new(" [Esc] Cancel OAuth Login Session\n Listening on local loopback TCP port 14210.")
                    .style(Style::default().fg(Color::DarkGray));
                f.render_widget(footer_help, modal_chunks[3]);
            }
        })?;

        while let Ok(event) = event_rx.try_recv() {
            match event {
                AppEvent::Key(key) => {
                    if let InputMode::OAuthLogin { .. } = &app.input_mode {
                        if key.code == KeyCode::Esc {
                            app.input_mode = InputMode::Normal;
                            app.set_status("OAuth login session cancelled.");
                            app.is_loading = false;
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

                    match key.code {
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
                                    CLIENT_ID, OAUTH_PORT
                                );
                                
                                let url_clone = auth_url.clone();
                                tokio::spawn(async move {
                                    let _ = open_browser(&url_clone);
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
                        _ => {}
                    }
                }
                AppEvent::Progress(msg) => {
                    app.set_status(&msg);
                }
                AppEvent::NetworkSuccess(result) => {
                    app.is_loading = false;
                    match result {
                        NetworkResult::AddAccountComplete { new_account } => {
                            app.input_mode = InputMode::Normal;
                            
                            let (reload_accs, _, _) = load_accounts_list();
                            app.accounts = reload_accs;
                            
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
                    app.update_status_decay();
                }
            }
        }
    }
}
