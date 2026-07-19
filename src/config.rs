use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use serde_json::json;
use uuid::Uuid;

use crate::types::{Account, CliCache, AccountHealth};

// OS Config Helpers
pub fn get_data_dir() -> PathBuf {
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

pub fn get_cli_cache_path() -> PathBuf {
    get_data_dir().join("cli_cache.json")
}

pub fn load_cli_cache() -> CliCache {
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
        theme: None,
        health: HashMap::new(),
        layout_preset: None,
    }
}

pub fn save_cli_cache(cache: &CliCache) {
    let path = get_cli_cache_path();
    if let Ok(content) = serde_json::to_string_pretty(cache) {
        let _ = fs::write(&path, content);
    }
}

pub fn record_health_failure(email: &str, error_msg: &str, cli_cache: &mut CliCache) {
    let now = chrono::Utc::now().timestamp();
    let health_entry = cli_cache.health.entry(email.to_string()).or_insert_with(|| AccountHealth {
        consecutive_failures: 0,
        last_error: None,
        last_check_timestamp: Some(now),
    });
    health_entry.consecutive_failures += 1;
    health_entry.last_error = Some(error_msg.to_string());
    health_entry.last_check_timestamp = Some(now);
    save_cli_cache(cli_cache);
}

pub fn record_health_success(email: &str, cli_cache: &mut CliCache) {
    let now = chrono::Utc::now().timestamp();
    let health_entry = cli_cache.health.entry(email.to_string()).or_insert_with(|| AccountHealth {
        consecutive_failures: 0,
        last_error: None,
        last_check_timestamp: Some(now),
    });
    health_entry.consecutive_failures = 0;
    health_entry.last_error = None;
    health_entry.last_check_timestamp = Some(now);
    save_cli_cache(cli_cache);
}

pub fn get_warmup_history_path() -> PathBuf {
    get_data_dir().join("warmup_history.json")
}

pub fn load_warmup_history() -> HashMap<String, i64> {
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

pub fn save_warmup_history(history: &HashMap<String, i64>) {
    let path = get_warmup_history_path();
    if let Ok(content) = serde_json::to_string_pretty(history) {
        let _ = fs::write(&path, content);
    }
}

// Helper to find the newest backup file matching antigravity_accounts_*.json
fn find_newest_backup() -> Option<PathBuf> {
    let mut candidates = Vec::new();

    // 1. Check in ~/.antigravity_tools
    let data_dir = get_data_dir();
    if let Ok(entries) = fs::read_dir(&data_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(filename) = path.file_name().and_then(|f| f.to_str()) {
                if filename.starts_with("antigravity_accounts_") && filename.ends_with(".json") {
                    candidates.push(path);
                }
            }
        }
    }

    // 2. Check in ~/Downloads
    if let Some(home) = dirs::home_dir() {
        let downloads = home.join("Downloads");
        if let Ok(entries) = fs::read_dir(downloads) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(filename) = path.file_name().and_then(|f| f.to_str()) {
                    if filename.starts_with("antigravity_accounts_") && filename.ends_with(".json") {
                        candidates.push(path);
                    }
                }
            }
        }
    }

    // 3. Also check the hardcoded termux path prefix
    let termux_tools = PathBuf::from("/data/data/com.termux/files/home/.antigravity_tools");
    if termux_tools.exists() {
        if let Ok(entries) = fs::read_dir(termux_tools) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(filename) = path.file_name().and_then(|f| f.to_str()) {
                    if filename.starts_with("antigravity_accounts_") && filename.ends_with(".json") {
                        candidates.push(path);
                    }
                }
            }
        }
    }

    // Sort alphabetically (which sorts YYYY-MM-DD correctly)
    candidates.sort_by(|a, b| {
        let a_name = a.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let b_name = b.file_name().and_then(|n| n.to_str()).unwrap_or("");
        a_name.cmp(b_name)
    });

    candidates.pop() // Get the last one (newest)
}

// Load accounts index or backup
pub fn load_accounts_list() -> (Vec<Account>, PathBuf, String) {
    if let Some(path) = find_newest_backup() {
        if let Ok(content) = fs::read_to_string(&path) {
            #[derive(serde::Deserialize)]
            struct RawBackupAcc {
                email: String,
                refresh_token: String,
                name: Option<String>,
            }
            if let Ok(raw_accs) = serde_json::from_str::<Vec<RawBackupAcc>>(&content) {
                let mut accounts = Vec::new();
                for item in raw_accs {
                    let default_name = item.email.split('@').next().unwrap_or("").to_string();
                    let file_name = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_else(|| "backup".to_string());
                    accounts.push(Account {
                        name: item.name.unwrap_or(default_name),
                        email: item.email,
                        refresh_token: item.refresh_token,
                        source: format!("backup ({})", file_name),
                        id: None,
                    });
                }
                if !accounts.is_empty() {
                    let file_name = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_else(|| "backup".to_string());
                    return (accounts, path.clone(), format!("Backup file '{}'", file_name));
                }
            }
        }
    }


    let data_dir = get_data_dir();
    let index_path = data_dir.join("accounts.json");
    if index_path.exists() {
        if let Ok(content) = fs::read_to_string(&index_path) {
            let cleaned = content.replace("\u{feff}", "").replace('\x00', "");
            
            #[derive(serde::Deserialize)]
            struct AccountSummary {
                id: String,
                email: String,
                name: Option<String>,
            }
            #[derive(serde::Deserialize)]
            struct AccountIndex {
                accounts: Vec<AccountSummary>,
            }

            if let Ok(index_data) = serde_json::from_str::<AccountIndex>(&cleaned) {
                let mut accounts = Vec::new();
                for acc in index_data.accounts {
                    let acc_path = data_dir.join("accounts").join(format!("{}.json", acc.id));
                    if acc_path.exists() {
                        if let Ok(af_content) = fs::read_to_string(&acc_path) {
                            #[derive(serde::Deserialize)]
                            struct TokenDetails {
                                refresh_token: String,
                            }
                            #[derive(serde::Deserialize)]
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

pub fn get_active_email(accounts: &[Account]) -> Option<String> {
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
            #[derive(serde::Deserialize)]
            struct AccountSummary {
                id: String,
                email: String,
            }
            #[derive(serde::Deserialize)]
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

// Write a new account directly to the database file
pub fn add_account_to_db(path: &Path, email: &str, refresh_token: &str) -> Result<Account, String> {
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
            let file_name = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_else(|| "backup".to_string());
            return Ok(Account {
                email: email.to_string(),
                refresh_token: refresh_token.to_string(),
                name: name.clone(),
                source: format!("backup ({})", file_name),
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
                
                let Some(data_dir) = path.parent() else {
                    return Err("Invalid database path (no parent directory).".to_string());
                };
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

pub fn delete_account_from_db(path: &Path, email: &str) -> Result<(), String> {
    if !path.exists() {
        return Err("Database file does not exist.".to_string());
    }
    
    let content = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let cleaned = content.replace("\u{feff}", "").replace('\x00', "");
    
    if let Ok(mut val) = serde_json::from_str::<serde_json::Value>(&cleaned) {
        if let Some(arr) = val.as_array_mut() {
            let len_before = arr.len();
            arr.retain(|x| x.get("email").and_then(|e| e.as_str()) != Some(email));
            if arr.len() == len_before {
                return Err("Account not found in database.".to_string());
            }
            let new_content = serde_json::to_string_pretty(&val).map_err(|e| e.to_string())?;
            fs::write(path, new_content).map_err(|e| e.to_string())?;
            return Ok(());
        } else if let Some(obj) = val.as_object_mut() {
            let mut deleted_id = None;
            let accounts_arr = obj.get_mut("accounts").and_then(|a| a.as_array_mut());
            if let Some(arr) = accounts_arr {
                let mut kept_arr = Vec::new();
                for x in arr.iter() {
                    if x.get("email").and_then(|e| e.as_str()) == Some(email) {
                        deleted_id = x.get("id").and_then(|i| i.as_str()).map(|s| s.to_string());
                    } else {
                        kept_arr.push(x.clone());
                    }
                }
                if deleted_id.is_none() {
                    return Err("Account not found in database.".to_string());
                }
                *arr = kept_arr;
            }
            
            if let Some(ref d_id) = deleted_id {
                let Some(data_dir) = path.parent() else {
                    return Err("Invalid database path (no parent directory).".to_string());
                };
                let acc_file = data_dir.join("accounts").join(format!("{}.json", d_id));
                if acc_file.exists() {
                    let _ = fs::remove_file(acc_file);
                }
                
                if obj.get("current_account_id").and_then(|c| c.as_str()) == Some(d_id) {
                    let accounts_arr = obj.get("accounts").and_then(|a| a.as_array());
                    if let Some(arr) = accounts_arr {
                        if !arr.is_empty() {
                            let first_id = arr[0].get("id").cloned().unwrap_or_else(|| json!(""));
                            obj.insert("current_account_id".to_string(), first_id);
                        } else {
                            obj.remove("current_account_id");
                        }
                    } else {
                        obj.remove("current_account_id");
                    }
                }
                
                let new_content = serde_json::to_string_pretty(&val).map_err(|e| e.to_string())?;
                fs::write(path, new_content).map_err(|e| e.to_string())?;
                return Ok(());
            }
        }
    }
    
    Err("Unknown/Unsupported database format.".to_string())
}

// Load monitored models list from gui_config.json
pub fn load_monitored_models() -> Vec<String> {
    let path = get_data_dir().join("gui_config.json");
    if path.exists() {
        if let Ok(content) = fs::read_to_string(&path) {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(monitored) = val.get("scheduled_warmup")
                    .and_then(|sw| sw.get("monitored_models"))
                    .and_then(|mm| mm.as_array())
                {
                    return monitored.iter()
                        .filter_map(|m| m.as_str().map(|s| s.to_string()))
                        .collect();
                }
            }
        }
    }
    // Fallback default list
    vec![
        "gemini-3-flash".to_string(),
        "claude".to_string(),
        "gemini-3-pro-high".to_string(),
        "gemini-3-pro-image".to_string(),
    ]
}

