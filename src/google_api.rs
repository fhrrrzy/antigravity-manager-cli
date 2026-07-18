use std::collections::HashMap;
use std::time::Instant;
use std::time::Duration;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::types::{
    ModelQuota, QuotaBucket, QuotaGroup, CliCache, TokenCache,
    CLIENT_ID, CLIENT_SECRET, TOKEN_URL
};
use crate::config::{record_health_success, record_health_failure, save_cli_cache};
use crate::keyring::{write_to_system_keyring, write_oauth_token_file};

pub async fn async_refresh_token(refresh_token: String) -> Option<(String, i64)> {
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

pub async fn async_fetch_project_and_tier(access_token: &str) -> (Option<String>, Option<String>) {
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

pub async fn async_fetch_quota(access_token: &str, project_id: Option<&str>) -> Result<Vec<ModelQuota>, String> {
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

pub async fn async_fetch_quota_summary(access_token: &str, project_id: Option<&str>) -> Option<Vec<QuotaGroup>> {
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

pub async fn async_trigger_warmup(access_token: &str, model_name: &str, project_id: Option<&str>, email: &str) -> Result<(), String> {
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
pub async fn ensure_valid_token(email: &str, refresh_token: &str, cli_cache: &mut CliCache) -> Option<(String, Option<String>)> {
    let now = chrono::Utc::now().timestamp();
    if let Some(tc) = cli_cache.tokens.get(email) {
        if tc.expiry_timestamp > now + 900 {
            // Keep successes fresh
            return Some((tc.access_token.clone(), tc.project_id.clone()));
        }
    }
    
    if let Some((new_tok, new_exp)) = async_refresh_token(refresh_token.to_string()).await {
        let (proj_id, tier) = async_fetch_project_and_tier(&new_tok).await;
        
        record_health_success(email, cli_cache);

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
        record_health_failure(email, "Failed to refresh OAuth token", cli_cache);
        None
    }
}

// Listen for OAuth Code from Google redirect on local loopback TCP port
pub async fn listen_for_oauth_code(port: u16) -> Result<String, String> {
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
pub async fn exchange_oauth_code(code: &str, port: u16) -> Result<(String, String, i64), String> {
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
pub async fn fetch_user_email(access_token: &str) -> Result<String, String> {
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
