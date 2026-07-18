use std::fs;
use std::path::PathBuf;
use serde_json::json;

pub fn write_to_system_keyring(_email: &str, access_token: &str, refresh_token: &str, expiry_timestamp: i64) -> bool {
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
pub fn write_oauth_token_file(access_token: &str, refresh_token: &str, expiry_timestamp: i64) {
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

pub fn subprocess_exists(cmd: &str) -> bool {
    std::process::Command::new("which")
        .arg(cmd)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn open_browser(url: &str) -> bool {
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
