use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use serde::Serialize;
use serde_json::json;

use crate::types::{Account, JsonAccountInfo, JsonQuotaOutput, QuotaData, ModelQuota, COOLDOWN_SECONDS};
use crate::config::{
    load_cli_cache, save_cli_cache, get_data_dir, add_account_to_db,
    load_warmup_history, save_warmup_history
};
use crate::google_api::{ensure_valid_token, async_fetch_project_and_tier, async_fetch_quota_summary, async_fetch_quota, async_trigger_warmup};
use crate::keyring::{write_to_system_keyring, write_oauth_token_file};

pub fn find_account_by_identifier<'a>(accounts: &'a [Account], id: &str) -> Option<&'a Account> {
    if let Ok(idx) = id.parse::<usize>() {
        if idx > 0 && idx <= accounts.len() {
            return Some(&accounts[idx - 1]);
        }
    }
    accounts.iter().find(|a| a.email.to_lowercase() == id.to_lowercase())
}

pub fn cli_backup(accounts: &[Account], filepath: Option<&str>) {
    let default_path = get_data_dir().join(format!("backup_antigravity_accounts_{}.json", chrono::Local::now().format("%Y-%m-%d")));
    let target_path = match filepath {
        Some(fp) => PathBuf::from(fp),
        None => default_path,
    };
    
    if let Some(parent) = target_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    
    #[derive(Serialize)]
    struct BackupAcc {
        email: String,
        refresh_token: String,
        name: String,
    }
    
    let backup_data: Vec<BackupAcc> = accounts.iter().map(|a| BackupAcc {
        email: a.email.clone(),
        refresh_token: a.refresh_token.clone(),
        name: a.name.clone(),
    }).collect();
    
    match serde_json::to_string_pretty(&backup_data) {
        Ok(json_str) => {
            match fs::write(&target_path, json_str) {
                Ok(_) => {
                    println!("✓ Successfully backed up {} accounts to: {}", backup_data.len(), target_path.to_string_lossy());
                }
                Err(e) => {
                    eprintln!("✗ Failed to write backup file: {}", e);
                }
            }
        }
        Err(e) => {
            eprintln!("✗ Failed to serialize backup data: {}", e);
        }
    }
}

pub fn cli_restore(db_path: &Path, filepath: &str) {
    let source_path = PathBuf::from(filepath);
    if !source_path.exists() {
        eprintln!("Error: Backup file does not exist at: {}", filepath);
        std::process::exit(1);
    }
    
    let content = match fs::read_to_string(&source_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: Failed to read backup file: {}", e);
            std::process::exit(1);
        }
    };
    
    #[allow(dead_code)]
    #[derive(serde::Deserialize)]
    struct RawBackupAcc {
        email: String,
        refresh_token: String,
        name: Option<String>,
    }
    
    let raw_accs: Vec<RawBackupAcc> = match serde_json::from_str(&content) {
        Ok(accs) => accs,
        Err(e) => {
            eprintln!("Error: Failed to parse backup file (invalid format): {}", e);
            std::process::exit(1);
        }
    };
    
    if raw_accs.is_empty() {
        println!("No accounts found in backup file. Nothing to restore.");
        return;
    }
    
    println!("Restoring {} accounts into local database...", raw_accs.len());
    let mut restored_count = 0;
    let mut skipped_count = 0;
    
    for acc in raw_accs {
        match add_account_to_db(db_path, &acc.email, &acc.refresh_token) {
            Ok(_) => {
                println!("  ✓ Restored: {}", acc.email);
                restored_count += 1;
            }
            Err(e) => {
                println!("  ○ Skipped {}: {}", acc.email, e);
                skipped_count += 1;
            }
        }
    }
    
    println!("\nRestore complete! Restored: {} accounts, Skipped: {} (duplicates/errors).", restored_count, skipped_count);
}

pub fn cli_list(accounts: &[Account], active_email: Option<&str>, source: &str, is_json: bool) {
    let cache = load_cli_cache();
    if is_json {
        let json_accs: Vec<JsonAccountInfo> = accounts.iter().map(|acc| {
            let is_active = active_email == Some(&acc.email);
            let health_data = cache.health.get(&acc.email);
            JsonAccountInfo {
                email: acc.email.clone(),
                name: acc.name.clone(),
                active: is_active,
                source: acc.source.clone(),
                consecutive_failures: health_data.map(|h| h.consecutive_failures).unwrap_or(0),
                last_error: health_data.and_then(|h| h.last_error.clone()),
                last_check_timestamp: health_data.and_then(|h| h.last_check_timestamp),
            }
        }).collect();
        println!("{}", serde_json::to_string_pretty(&json_accs).unwrap_or_default());
        return;
    }

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

pub async fn cli_auto_switch(accounts: &[Account], active_email: Option<&str>) {
    if accounts.is_empty() {
        eprintln!("Error: No accounts configured.");
        std::process::exit(1);
    }

    let cache = load_cli_cache();
    let mut best_acc: Option<&Account> = None;
    let mut best_score = i32::MIN;

    let get_weekly_pct = |quota_cache: Option<&QuotaData>, is_claude: bool| -> i32 {
        let q = match quota_cache {
            Some(q) => q,
            None => return -1,
        };
        let groups = match &q.quota_groups {
            Some(g) => g,
            None => return -1,
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
        -1
    };

    println!("Evaluating account pool for auto-switching...");

    for acc in accounts {
        let email = &acc.email;
        let mut score = 1000;

        // 1. Health/Failure penalty
        if let Some(health) = cache.health.get(email) {
            if health.consecutive_failures >= 3 {
                score -= 10000;
            } else {
                score -= 300 * health.consecutive_failures as i32;
            }
        }

        // 2. 5-Hour model quota usage penalty
        if let Some(q) = cache.quotas.get(email) {
            let mut gemini_pct = -1;
            let mut claude_pct = -1;

            if let Some(gemini_m) = q.models.iter().find(|m| m.name.contains("gemini") || m.display_name.as_ref().map(|n| n.contains("Gemini")).unwrap_or(false)) {
                gemini_pct = gemini_m.percentage;
            }
            if let Some(claude_m) = q.models.iter().find(|m| m.name.contains("claude") || m.display_name.as_ref().map(|n| n.contains("Claude")).unwrap_or(false)) {
                claude_pct = claude_m.percentage;
            }

            let max_pct = std::cmp::max(gemini_pct, claude_pct);
            if max_pct >= 0 {
                score -= max_pct;
                if gemini_pct >= 100 || claude_pct >= 100 {
                    score -= 500;
                }
            } else {
                score -= 50;
            }
        } else {
            score -= 100;
        }

        // 3. Weekly remaining quota bonus
        let gemini_wk_pct = get_weekly_pct(cache.quotas.get(email), false);
        let claude_wk_pct = get_weekly_pct(cache.quotas.get(email), true);

        let min_wk_pct = if gemini_wk_pct >= 0 && claude_wk_pct >= 0 {
            std::cmp::min(gemini_wk_pct, claude_wk_pct)
        } else if gemini_wk_pct >= 0 {
            gemini_wk_pct
        } else if claude_wk_pct >= 0 {
            claude_wk_pct
        } else {
            -1
        };

        if min_wk_pct >= 0 {
            score += min_wk_pct;
            if min_wk_pct == 0 {
                score -= 500;
            }
        } else {
            score += 50;
        }

        println!("  - Account: {} | Score: {}", email, score);

        if score > best_score {
            best_score = score;
            best_acc = Some(acc);
        }
    }

    if let Some(best) = best_acc {
        let current_active = active_email.unwrap_or("");
        if best.email == current_active {
            println!("✓ Current active account {} is already the best choice (Score: {}).", best.email, best_score);
        } else {
            println!("✓ Auto-switched: Healthiest account is {} (Score: {})", best.email, best_score);
            cli_switch(accounts, &best.email).await;
        }
    } else {
        eprintln!("Error: No viable accounts found to auto-switch.");
    }
}

pub async fn cli_switch(accounts: &[Account], identifier: &str) {
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

pub async fn cli_quota(accounts: &[Account], active_email: Option<&str>, identifier: Option<&str>, refresh: bool, is_json: bool) {
    let mut cache = load_cli_cache();

    if identifier == Some("all") {
        if refresh {
            if !is_json {
                println!("Refreshing quotas for ALL configured accounts sequentially...");
            }
            let count_accs = accounts.len();
            for (idx, acc) in accounts.iter().enumerate() {
                let email = &acc.email;
                if !is_json {
                    println!("[{}/{}] Fetching quota for {}...", idx + 1, count_accs, email);
                }
                
                let (access_token, mut project_id) = match ensure_valid_token(email, &acc.refresh_token, &mut cache).await {
                    Some(t) => t,
                    None => {
                        if !is_json {
                            eprintln!("✗ Error: Failed to validate credentials for {}. Skipping.", email);
                        }
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
                        if !is_json {
                            println!("✓ Quota updated.");
                        }
                    }
                    Err(e) => {
                        if !is_json {
                            eprintln!("✗ Error: {}", e);
                        }
                    }
                }
            }
            if !is_json {
                println!("Quotas refresh complete.");
            }
        }
        
        if is_json {
            let mut list = Vec::new();
            for acc in accounts {
                let email = &acc.email;
                if let Some(q) = cache.quotas.get(email) {
                    let proj = cache.tokens.get(email).and_then(|t| t.project_id.clone());
                    list.push(JsonQuotaOutput {
                        email: email.clone(),
                        subscription_tier: q.subscription_tier.clone(),
                        project_id: proj,
                        models: q.models.clone(),
                        quota_groups: q.quota_groups.clone(),
                    });
                }
            }
            println!("{}", serde_json::to_string_pretty(&list).unwrap_or_default());
            return;
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
        if !is_json {
            println!("Fetching latest quota from Google APIs for {}...", target_email);
        }
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
                if !is_json {
                    println!("✓ Quota cache updated.");
                }
            }
            Err(e) => {
                eprintln!("Error fetching quota: {}", e);
                std::process::exit(1);
            }
        }
    }
    
    let quota_data = cache.quotas.get(target_email);
    if quota_data.is_none() || quota_data.unwrap().models.is_empty() {
        if is_json {
            println!("[]");
        } else {
            println!("No cached quotas for {}. Run with '--refresh' to fetch.", target_email);
        }
        return;
    }
    
    let q = quota_data.unwrap();
    if is_json {
        let proj = cache.tokens.get(target_email).and_then(|t| t.project_id.clone());
        let output = JsonQuotaOutput {
            email: target_email.to_string(),
            subscription_tier: q.subscription_tier.clone(),
            project_id: proj,
            models: q.models.clone(),
            quota_groups: q.quota_groups.clone(),
        };
        println!("{}", serde_json::to_string_pretty(&output).unwrap_or_default());
        return;
    }

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

pub async fn cli_warmup(accounts: &[Account], active_email: Option<&str>, identifier: Option<&str>, model_name: Option<&str>, force: bool) {
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
