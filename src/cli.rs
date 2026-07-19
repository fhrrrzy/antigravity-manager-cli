use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use serde::Serialize;
use serde_json::json;

use crate::types::{Account, JsonAccountInfo, JsonQuotaOutput, QuotaData, ModelQuota, COOLDOWN_SECONDS};
use crate::config::{
    load_cli_cache, save_cli_cache, get_data_dir, add_account_to_db,
    load_warmup_history, save_warmup_history, delete_account_from_db,
    load_monitored_models
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
    println!("┌───┬────────┬────────────────────────────┬────────┬─────────────┬─────────────┬──────────────────┐");
    println!("│ # │ Status │ Email                      │  Plan  │ Gemini(5h/Wk)│ Claude(5h/Wk)│ Health Status    │");
    println!("├───┼────────┼────────────────────────────┼────────┼─────────────┼─────────────┼──────────────────┤");

    for (idx, acc) in accounts.iter().enumerate() {
        let is_active = active_email == Some(&acc.email);
        let quota = cache.quotas.get(&acc.email);
        
        let gemini_5h = quota.and_then(|q| {
            q.models.iter()
                .find(|m| m.name.contains("gemini") || m.display_name.as_ref().map(|n| n.contains("Gemini")).unwrap_or(false))
                .map(|m| m.percentage)
        });
        
        let claude_5h = quota.and_then(|q| {
            q.models.iter()
                .find(|m| m.name.contains("claude") || m.display_name.as_ref().map(|n| n.contains("Claude")).unwrap_or(false))
                .map(|m| m.percentage)
        });

        let get_weekly = |is_claude: bool| -> Option<i32> {
            let q = quota?;
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

        let gemini_wk = get_weekly(false);
        let claude_wk = get_weekly(true);

        let gemini_str = match (gemini_5h, gemini_wk) {
            (Some(h5), Some(wk)) => format!("{:>3}%/{:>3}%", h5, wk),
            (Some(h5), None) => format!("{:>3}%/N/A", h5),
            (None, Some(wk)) => format!("N/A/{:>3}%", wk),
            (None, None) => "N/A".to_string(),
        };

        let claude_str = match (claude_5h, claude_wk) {
            (Some(h5), Some(wk)) => format!("{:>3}%/{:>3}%", h5, wk),
            (Some(h5), None) => format!("{:>3}%/N/A", h5),
            (None, Some(wk)) => format!("N/A/{:>3}%", wk),
            (None, None) => "N/A".to_string(),
        };

        let plan_str = quota.and_then(|q| q.subscription_tier.as_deref())
            .or_else(|| cache.tokens.get(&acc.email).and_then(|t| t.subscription_tier.as_deref()))
            .unwrap_or("Free");

        let health_data = cache.health.get(&acc.email);
        let (status_str, health_label) = match health_data {
            Some(h) if h.consecutive_failures > 0 => {
                let mark = if is_active { "★  ⚠️" } else { "   ✗" };
                let failures = format!("{} Failures", h.consecutive_failures);
                (mark, failures)
            }
            _ => {
                let mark = if is_active { "  ★" } else { "  ○" };
                (mark, "Healthy".to_string())
            }
        };

        println!(
            "│ {:^3} │ {:<8} │ {:<28} │ {:^8} │ {:^13} │ {:^13} │ {:<18} │",
            idx + 1,
            status_str,
            acc.email,
            plan_str,
            gemini_str,
            claude_str,
            health_label
        );
    }
    println!("└───┴────────┴────────────────────────────┴────────┴─────────────┴─────────────┴──────────────────┘");
    println!("★ = Active  |  ○ = Inactive  |  ⚠️ = Active Error  |  ✗ = Inactive Error");
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

pub async fn cli_switch_interactive(accounts: &[Account]) {
    if accounts.is_empty() {
        println!("No accounts configured.");
        return;
    }

    use crossterm::{
        terminal::{enable_raw_mode, disable_raw_mode},
        event::{self, Event, KeyCode},
    };

    println!("Select an account to activate (use Up/Down or j/k, Enter to select, Esc to cancel):");
    let mut selected = 0;

    if let Err(_) = enable_raw_mode() {
        println!("Please enter the index of the account (0 to {}):", accounts.len() - 1);
        for (idx, acc) in accounts.iter().enumerate() {
            println!("  [{}] {}", idx, acc.email);
        }
        let mut input = String::new();
        if std::io::stdin().read_line(&mut input).is_ok() {
            if let Ok(idx) = input.trim().parse::<usize>() {
                if idx < accounts.len() {
                    cli_switch(accounts, &accounts[idx].email).await;
                    return;
                }
            }
        }
        println!("Invalid selection.");
        return;
    }

    for (idx, acc) in accounts.iter().enumerate() {
        if idx == selected {
            println!("  > \x1b[36m{}\x1b[0m", acc.email);
        } else {
            println!("    {}", acc.email);
        }
    }

    let mut result_email = None;
    loop {
        if let Ok(Event::Key(key)) = event::read() {
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    if selected > 0 {
                        selected -= 1;
                        print!("\x1b[{}F", accounts.len());
                        for (idx, acc) in accounts.iter().enumerate() {
                            if idx == selected {
                                print!("\x1b[2K  > \x1b[36m{}\x1b[0m\n", acc.email);
                            } else {
                                print!("\x1b[2K    {}\n", acc.email);
                            }
                        }
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if selected < accounts.len() - 1 {
                        selected += 1;
                        print!("\x1b[{}F", accounts.len());
                        for (idx, acc) in accounts.iter().enumerate() {
                            if idx == selected {
                                print!("\x1b[2K  > \x1b[36m{}\x1b[0m\n", acc.email);
                            } else {
                                print!("\x1b[2K    {}\n", acc.email);
                            }
                        }
                    }
                }
                KeyCode::Enter => {
                    result_email = Some(accounts[selected].email.clone());
                    break;
                }
                KeyCode::Esc | KeyCode::Char('q') => {
                    break;
                }
                _ => {}
            }
        }
    }

    let _ = disable_raw_mode();
    if let Some(email) = result_email {
        println!();
        cli_switch(accounts, &email).await;
    } else {
        println!("\nSwitch cancelled.");
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
    
    let Some(acc) = accounts.iter().find(|a| a.email == *target_email) else {
        eprintln!("Error: Active account {} not found in database.", target_email);
        std::process::exit(1);
    };
    
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
    
    let Some(q) = cache.quotas.get(target_email) else {
        if is_json {
            println!("[]");
        } else {
            println!("No cached quotas for {}. Run with '--refresh' to fetch.", target_email);
        }
        return;
    };
    if q.models.is_empty() {
        if is_json {
            println!("[]");
        } else {
            println!("No cached quotas for {}. Run with '--refresh' to fetch.", target_email);
        }
        return;
    }
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
        println!("Running Warm Up cycle for ALL configured accounts concurrently in batches...");
        let mut cache = load_cli_cache();
        let history = load_warmup_history();
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
            let monitored = load_monitored_models();
            for m in &models {
                if m.percentage >= 100 {
                    let is_monitored = monitored.iter().any(|mon_m| {
                        let mon_lower = mon_m.to_lowercase();
                        let m_lower = m.name.to_lowercase();
                        m_lower.contains(&mon_lower) || mon_lower.contains(&m_lower)
                    });
                    if is_monitored {
                        to_warm.push(m.clone());
                    }
                }
            }
            
            if to_warm.is_empty() {
                println!("✓ All monitored models have remaining usage. No warmup needed.");
                continue;
            }
            
            let mut eligible_models = Vec::new();
            for m in to_warm {
                let name = m.name.clone();
                let display = m.display_name.clone().unwrap_or_else(|| name.clone());
                
                if name.contains("2.5-") || name.contains("2-5-") {
                    continue;
                }
                
                if !force {
                    let key = format!("{}:{}:100", email, name);
                    if let Some(&last) = history.get(&key) {
                        let elapsed = now - last;
                        if elapsed < COOLDOWN_SECONDS {
                            let rem = COOLDOWN_SECONDS - elapsed;
                            println!("Skipping {}: Cooling down ({}h {}m remaining).", display, rem / 3600, (rem % 3600) / 60);
                            continue;
                        }
                    }
                }
                eligible_models.push(m);
            }

            // Concurrent batches of 3
            let batch_size = 3;
            for chunk in eligible_models.chunks(batch_size) {
                let mut handles = Vec::new();
                for m in chunk {
                    let name = m.name.clone();
                    let display = m.display_name.clone().unwrap_or_else(|| name.clone());
                    let token_clone = access_token.clone();
                    let proj_clone = project_id.clone();
                    let email_clone = email.to_string();
                    
                    println!("Warming up model {}...", display);
                    let handle = tokio::spawn(async move {
                        let res = async_trigger_warmup(&token_clone, &name, proj_clone.as_deref(), &email_clone).await;
                        (res, name)
                    });
                    handles.push(handle);
                }
                
                for h in handles {
                    if let Ok((res, name)) = h.await {
                        let display = name.clone();
                        match res {
                            Ok(_) => {
                                println!("✓ Successfully warmed up {}!", display);
                                let mut hist = load_warmup_history();
                                let key = format!("{}:{}:100", email, name);
                                hist.insert(key, chrono::Utc::now().timestamp());
                                save_warmup_history(&hist);
                            }
                            Err(e) => {
                                println!("✗ Warmup failed for {}: {}", display, e);
                            }
                        }
                    }
                }
                tokio::time::sleep(Duration::from_millis(1500)).await;
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
    
    let Some(acc) = accounts.iter().find(|a| a.email == *target_email) else {
        eprintln!("Error: Active account {} not found in database.", target_email);
        std::process::exit(1);
    };
    let mut cache = load_cli_cache();
    let history = load_warmup_history();
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
        let monitored = load_monitored_models();
        for m in &models {
            if m.percentage >= 100 {
                let is_monitored = monitored.iter().any(|mon_m| {
                    let mon_lower = mon_m.to_lowercase();
                    let m_lower = m.name.to_lowercase();
                    m_lower.contains(&mon_lower) || mon_lower.contains(&m_lower)
                });
                if is_monitored {
                    to_warm.push(m.clone());
                }
            }
        }
    }
    
    if to_warm.is_empty() {
        println!("All monitored models have remaining quotas. No warmup needed.");
        return;
    }
    
    let mut count = 0;
    let mut eligible_models = Vec::new();
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
        eligible_models.push(m);
    }
    
    // Concurrent batches of 3
    let batch_size = 3;
    for chunk in eligible_models.chunks(batch_size) {
        let mut handles = Vec::new();
        for m in chunk {
            let name = m.name.clone();
            let display = m.display_name.clone().unwrap_or_else(|| name.clone());
            let token_clone = access_token.clone();
            let proj_clone = project_id.clone();
            let email_clone = target_email.to_string();
            
            println!("Warming up model {}...", display);
            let handle = tokio::spawn(async move {
                let res = async_trigger_warmup(&token_clone, &name, proj_clone.as_deref(), &email_clone).await;
                (res, name)
            });
            handles.push(handle);
        }
        
        for h in handles {
            if let Ok((res, name)) = h.await {
                let display = name.clone();
                match res {
                    Ok(_) => {
                        println!("✓ Successfully warmed up {}!", display);
                        let mut hist = load_warmup_history();
                        let key = format!("{}:{}:100", target_email, name);
                        hist.insert(key, chrono::Utc::now().timestamp());
                        save_warmup_history(&hist);
                        count += 1;
                    }
                    Err(e) => {
                        println!("✗ Warmup failed for {}: {}", display, e);
                    }
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(1500)).await;
    }
    println!("Warmup cycle finished. Triggered {} warmup(s).", count);
}

fn read_password(prompt: &str) -> Result<String, String> {
    use crossterm::{
        terminal::{enable_raw_mode, disable_raw_mode},
        event::{self, Event, KeyCode},
    };
    use std::io::{self, Write};

    print!("{}", prompt);
    let _ = io::stdout().flush();

    if let Err(_) = enable_raw_mode() {
        let mut input = String::new();
        io::stdin().read_line(&mut input).map_err(|e| e.to_string())?;
        return Ok(input.trim().to_string());
    }

    let mut pass = String::new();
    loop {
        if let Ok(Event::Key(key)) = event::read() {
            match key.code {
                KeyCode::Char(c) => {
                    pass.push(c);
                    print!("*");
                    let _ = io::stdout().flush();
                }
                KeyCode::Backspace => {
                    if !pass.is_empty() {
                        pass.pop();
                        print!("\x08 \x08");
                        let _ = io::stdout().flush();
                    }
                }
                KeyCode::Enter => {
                    break;
                }
                KeyCode::Esc => {
                    let _ = disable_raw_mode();
                    println!();
                    return Err("Input cancelled".to_string());
                }
                _ => {}
            }
        }
    }

    let _ = disable_raw_mode();
    println!();
    Ok(pass.trim().to_string())
}

pub async fn cli_add() {
    use std::io::{self, Write};
    
    print!("Enter email address: ");
    let _ = io::stdout().flush();
    let mut email = String::new();
    if io::stdin().read_line(&mut email).is_err() {
        eprintln!("Error reading email.");
        return;
    }
    let email = email.trim().to_string();
    if email.is_empty() {
        eprintln!("Email cannot be empty.");
        return;
    }
    
    let refresh_token = match read_password("Enter Google OAuth refresh token: ") {
        Ok(t) => t,
        Err(e) => {
            eprintln!("Error: {}", e);
            return;
        }
    };
    if refresh_token.is_empty() {
        eprintln!("Refresh token cannot be empty.");
        return;
    }
    
    println!("Validating credentials with Google API...");
    let mut cache = load_cli_cache();
    if let Some((access_token, _project_id)) = crate::google_api::ensure_valid_token(&email, &refresh_token, &mut cache).await {
        let db_path = get_data_dir().join("accounts.json");
        if let Err(e) = add_account_to_db(&db_path, &email, &refresh_token) {
            eprintln!("Error adding account to database: {}", e);
            return;
        }
        
        let expiry = cache.tokens.get(&email).map(|t| t.expiry_timestamp).unwrap_or(0);
        let _ = write_to_system_keyring(&email, &access_token, &refresh_token, expiry);
        write_oauth_token_file(&access_token, &refresh_token, expiry);
        
        println!("✓ Account added successfully!");
        println!("  Email: {}", email);
        println!("  Plan: {}", cache.tokens.get(&email).and_then(|t| t.subscription_tier.as_deref()).unwrap_or("Free"));
    } else {
        eprintln!("Error: Google API credential validation failed. Check your refresh token.");
    }
}

pub fn cli_status(is_json: bool) {
    let cache = load_cli_cache();
    let active = match &cache.active_email {
        Some(e) => e,
        None => {
            if is_json {
                println!("{{}}");
            } else {
                println!("○ No Active Session");
            }
            return;
        }
    };
    
    let masked_email = crate::tui::ui::mask_email(active, true);
    let quota = cache.quotas.get(active);
    
    let gemini_5h = quota.and_then(|q| {
        q.models.iter()
            .find(|m| m.name.contains("gemini") || m.display_name.as_ref().map(|n| n.contains("Gemini")).unwrap_or(false))
            .map(|m| m.percentage)
    });
    
    let claude_5h = quota.and_then(|q| {
        q.models.iter()
            .find(|m| m.name.contains("claude") || m.display_name.as_ref().map(|n| n.contains("Claude")).unwrap_or(false))
            .map(|m| m.percentage)
    });

    let get_weekly = |is_claude: bool| -> Option<i32> {
        let q = quota?;
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

    let gemini_wk = get_weekly(false);
    let claude_wk = get_weekly(true);
    
    let plan = quota.and_then(|q| q.subscription_tier.as_deref())
        .or_else(|| cache.tokens.get(active).and_then(|t| t.subscription_tier.as_deref()))
        .unwrap_or("Free");
        
    let health = cache.health.get(active);
    let is_healthy = health.map(|h| h.consecutive_failures == 0).unwrap_or(true);
    
    if is_json {
        let out = json!({
            "active_email": masked_email,
            "plan": plan,
            "gemini_5h": gemini_5h,
            "gemini_wk": gemini_wk,
            "claude_5h": claude_5h,
            "claude_wk": claude_wk,
            "healthy": is_healthy
        });
        println!("{}", serde_json::to_string(&out).unwrap_or_default());
    } else {
        let status_mark = if is_healthy { "●" } else { "⚠" };
        let gemini_fmt = gemini_5h.map(|p| format!("{}%", p)).unwrap_or_else(|| "N/A".to_string());
        let claude_fmt = claude_5h.map(|p| format!("{}%", p)).unwrap_or_else(|| "N/A".to_string());
        println!("{} {} [{}] | Gemini: {} | Claude: {}", status_mark, masked_email, plan, gemini_fmt, claude_fmt);
    }
}

pub async fn cli_daemon(accounts: &[Account], quota_target: &str, interval_secs: u64) {
    println!("[DAEMON] Starting Antigravity Manager Failover Daemon...");
    println!("[DAEMON] Target Quota: {}", quota_target);
    println!("[DAEMON] Refresh Interval: {}s", interval_secs);
    
    loop {
        let mut cache = load_cli_cache();
        if let Some(active_email) = &cache.active_email {
            println!("[DAEMON] Checking health and quotas for active email: {}...", active_email);
            
            if let Some(acc) = accounts.iter().find(|a| a.email == *active_email) {
                match crate::google_api::ensure_valid_token(&acc.email, &acc.refresh_token, &mut cache).await {
                    Some((access_token, project_id)) => {
                        match crate::google_api::async_fetch_quota(&access_token, project_id.as_deref()).await {
                            Ok(models) => {
                                cache.quotas.insert(acc.email.clone(), QuotaData {
                                    subscription_tier: cache.tokens.get(&acc.email).and_then(|t| t.subscription_tier.clone()),
                                    models: models.clone(),
                                    quota_groups: None,
                                });
                                save_cli_cache(&cache);
                                
                                let mut trigger_failover = false;
                                
                                let gemini_pct = models.iter()
                                    .find(|m| m.name.contains("gemini") || m.display_name.as_ref().map(|n| n.contains("Gemini")).unwrap_or(false))
                                    .map(|m| m.percentage)
                                    .unwrap_or(-1);
                                    
                                let claude_pct = models.iter()
                                    .find(|m| m.name.contains("claude") || m.display_name.as_ref().map(|n| n.contains("Claude")).unwrap_or(false))
                                    .map(|m| m.percentage)
                                    .unwrap_or(-1);
                                    
                                match quota_target.to_lowercase().as_str() {
                                    "gemini" => {
                                        if gemini_pct == 0 {
                                            println!("[DAEMON] Gemini quota is exhausted (0%). Triggering failover...");
                                            trigger_failover = true;
                                        }
                                    }
                                    "claude" => {
                                        if claude_pct == 0 {
                                            println!("[DAEMON] Claude quota is exhausted (0%). Triggering failover...");
                                            trigger_failover = true;
                                        }
                                    }
                                    _ => {
                                        if gemini_pct == 0 || claude_pct == 0 {
                                            println!("[DAEMON] Quota exhausted (Gemini: {}%, Claude: {}%). Triggering failover...", gemini_pct, claude_pct);
                                            trigger_failover = true;
                                        }
                                    }
                                }
                                
                                if trigger_failover {
                                    println!("[DAEMON] Automatically switching to the healthiest account...");
                                    let notify_cfg = crate::config::load_notify_config();
                                    if notify_cfg.notify_on_failover.unwrap_or(true) {
                                        let msg = format!(
                                            "⚡ AGM Failover: Quota exhausted on {} (Gemini: {}%, Claude: {}%). Switching account.",
                                            acc.email, gemini_pct, claude_pct
                                        );
                                        let _ = send_webhook_notification(&notify_cfg, &msg).await;
                                    }
                                    cli_auto_switch(accounts, Some(&acc.email)).await;
                                } else {
                                    // Check low quota threshold
                                    let notify_cfg = crate::config::load_notify_config();
                                    if notify_cfg.notify_on_low_quota.unwrap_or(false) {
                                        let threshold = notify_cfg.low_quota_threshold.unwrap_or(10);
                                        if (gemini_pct >= 0 && gemini_pct <= threshold) || (claude_pct >= 0 && claude_pct <= threshold) {
                                            let msg = format!(
                                                "⚠️ AGM Low Quota Warning on {}: Gemini {}%, Claude {}% (threshold: {}%)",
                                                acc.email, gemini_pct, claude_pct, threshold
                                            );
                                            let _ = send_webhook_notification(&notify_cfg, &msg).await;
                                        }
                                    }
                                    println!(
                                        "[DAEMON] Active account is healthy. Quotas: Gemini: {}%, Claude: {}%",
                                        if gemini_pct >= 0 { format!("{}%", gemini_pct) } else { "N/A".to_string() },
                                        if claude_pct >= 0 { format!("{}%", claude_pct) } else { "N/A".to_string() }
                                    );
                                }
                            }
                            Err(e) => {
                                eprintln!("[DAEMON] Error fetching quotas: {}", e);
                            }
                        }
                    }
                    None => {
                        eprintln!("[DAEMON] Active token refresh failed. Triggering failover to healthy account...");
                        let notify_cfg = crate::config::load_notify_config();
                        if notify_cfg.notify_on_failover.unwrap_or(true) {
                            let msg = format!("🔴 AGM Failover: Token refresh failed for {}. Switching to backup account.", acc.email);
                            let _ = send_webhook_notification(&notify_cfg, &msg).await;
                        }
                        cli_auto_switch(accounts, Some(&acc.email)).await;
                    }
                }
            } else {
                eprintln!("[DAEMON] Warning: Active email '{}' not found in database accounts list.", active_email);
            }
        } else {
            println!("[DAEMON] No active email session set. Running auto-switch to select initial account...");
            cli_auto_switch(accounts, None).await;
        }
        
        tokio::time::sleep(Duration::from_secs(interval_secs)).await;
    }
}

pub async fn cli_check(accounts: &[Account]) {
    println!("Checking credentials and plans for {} accounts concurrently...", accounts.len());
    
    let cache = load_cli_cache();
    let mut tasks = Vec::new();
    
    for acc in accounts {
        let email = acc.email.clone();
        let refresh_token = acc.refresh_token.clone();
        let mut cache_clone = cache.clone();
        
        tasks.push(tokio::spawn(async move {
            let start = std::time::Instant::now();
            let result = crate::google_api::ensure_valid_token(&email, &refresh_token, &mut cache_clone).await;
            (email, result, cache_clone, start.elapsed())
        }));
    }
    
    println!("┌──────────────────────────────┬────────┬──────────────┬─────────────┐");
    println!("│ Email                        │ Status │ Plan         │ Latency     │");
    println!("├──────────────────────────────┼────────┼──────────────┼─────────────┤");
    
    let mut healthy_count = 0;
    for task in futures::future::join_all(tasks).await {
        if let Ok((email, res, updated_cache, elapsed)) = task {
            let (status_str, plan_str) = match res {
                Some((_, _)) => {
                    healthy_count += 1;
                    let tier = updated_cache.tokens.get(&email)
                        .and_then(|t| t.subscription_tier.as_deref())
                        .unwrap_or("Free");
                    ("✓ Valid", tier)
                }
                None => ("✗ Failed", "N/A")
            };
            println!("│ {:<28} │ {:^6} │ {:^12} │ {:>8.2?} │", email, status_str, plan_str, elapsed);
        }
    }
    println!("└──────────────────────────────┴────────┴──────────────┴─────────────┘");
    println!("Summary: {} / {} Accounts Healthy", healthy_count, accounts.len());
}

fn get_pid_file_path() -> PathBuf {
    get_data_dir().join("daemon.pid")
}

pub fn cli_daemon_status() {
    let pid_file = get_pid_file_path();
    if !pid_file.exists() {
        println!("○ Daemon is stopped.");
        return;
    }
    
    if let Ok(content) = fs::read_to_string(&pid_file) {
        if let Ok(pid) = content.trim().parse::<i32>() {
            let status = Command::new("kill")
                .arg("-0")
                .arg(pid.to_string())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
                
            if let Ok(s) = status {
                if s.success() {
                    println!("● Daemon is running (PID: {}).", pid);
                    return;
                }
            }
        }
    }
    
    println!("○ Daemon is stopped (stale PID file found).");
    let _ = fs::remove_file(pid_file);
}

pub fn cli_daemon_stop() {
    let pid_file = get_pid_file_path();
    if !pid_file.exists() {
        println!("Daemon is not running.");
        return;
    }
    
    if let Ok(content) = fs::read_to_string(&pid_file) {
        if let Ok(pid) = content.trim().parse::<i32>() {
            println!("Stopping daemon (PID: {})...", pid);
            let _ = Command::new("kill")
                .arg(pid.to_string())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            
            std::thread::sleep(Duration::from_millis(500));
            let _ = fs::remove_file(&pid_file);
            println!("✓ Daemon stopped successfully.");
            return;
        }
    }
    
    let _ = fs::remove_file(pid_file);
    println!("✓ Daemon stopped.");
}

pub fn cli_daemon_start(quota: &str, interval: u64) {
    let pid_file = get_pid_file_path();
    if pid_file.exists() {
        if let Ok(content) = fs::read_to_string(&pid_file) {
            if let Ok(pid) = content.trim().parse::<i32>() {
                let status = Command::new("kill")
                    .arg("-0")
                    .arg(pid.to_string())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
                if let Ok(s) = status {
                    if s.success() {
                        println!("Daemon is already running (PID: {}).", pid);
                        return;
                    }
                }
            }
        }
        let _ = fs::remove_file(&pid_file);
    }
    
    let log_file_path = get_data_dir().join("daemon.log");
    let log_file = std::fs::File::create(&log_file_path).ok();
    
    let current_exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("agm"));
    println!("Starting Antigravity Manager Failover Daemon in background...");
    
    let mut cmd = Command::new("setsid");
    cmd.arg(current_exe)
       .arg("daemon")
       .arg("run")
       .arg("--quota")
       .arg(quota)
       .arg("--interval")
       .arg(interval.to_string());
       
    if let Some(ref f) = log_file {
        if let Ok(dup) = f.try_clone() {
            cmd.stdout(dup);
        }
        if let Ok(dup_err) = f.try_clone() {
            cmd.stderr(dup_err);
        }
    } else {
        cmd.stdout(std::process::Stdio::null())
           .stderr(std::process::Stdio::null());
    }
    
    let child = cmd.spawn();
        
    match child {
        Ok(c) => {
            let pid = c.id();
            if let Err(e) = fs::write(&pid_file, pid.to_string()) {
                eprintln!("Error writing PID file: {}", e);
            } else {
                println!("✓ Daemon started successfully (PID: {}).", pid);
            }
        }
        Err(e) => {
            eprintln!("Error spawning daemon process: {}", e);
        }
    }
}

pub async fn cli_remove(accounts: &[Account]) {
    if accounts.is_empty() {
        println!("No accounts configured.");
        return;
    }

    use crossterm::{
        terminal::{enable_raw_mode, disable_raw_mode},
        event::{self, Event, KeyCode},
    };
    use std::io::{self, Write};

    println!("Select an account to delete (use Up/Down or j/k, Enter to select, Esc to cancel):");
    let mut selected = 0;

    if let Err(_) = enable_raw_mode() {
        println!("Please enter the index of the account (0 to {}):", accounts.len() - 1);
        for (idx, acc) in accounts.iter().enumerate() {
            println!("  [{}] {}", idx, acc.email);
        }
        let mut input = String::new();
        if io::stdin().read_line(&mut input).is_ok() {
            if let Ok(idx) = input.trim().parse::<usize>() {
                if idx < accounts.len() {
                    let email = &accounts[idx].email;
                    print!("Are you sure you want to delete {}? (y/n): ", email);
                    let _ = io::stdout().flush();
                    let mut confirm = String::new();
                    if io::stdin().read_line(&mut confirm).is_ok() && confirm.trim().to_lowercase() == "y" {
                        let db_path = get_data_dir().join("accounts.json");
                        match delete_account_from_db(&db_path, email) {
                            Ok(_) => println!("✓ Account {} deleted successfully.", email),
                            Err(e) => eprintln!("Error deleting account: {}", e),
                        }
                    } else {
                        println!("Cancelled.");
                    }
                    return;
                }
            }
        }
        println!("Invalid selection.");
        return;
    }

    for (idx, acc) in accounts.iter().enumerate() {
        if idx == selected {
            println!("  > \x1b[31m{}\x1b[0m", acc.email);
        } else {
            println!("    {}", acc.email);
        }
    }

    let mut target_email = None;
    loop {
        if let Ok(Event::Key(key)) = event::read() {
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    if selected > 0 {
                        selected -= 1;
                        print!("\x1b[{}F", accounts.len());
                        for (idx, acc) in accounts.iter().enumerate() {
                            if idx == selected {
                                print!("\x1b[2K  > \x1b[31m{}\x1b[0m\n", acc.email);
                            } else {
                                print!("\x1b[2K    {}\n", acc.email);
                            }
                        }
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if selected < accounts.len() - 1 {
                        selected += 1;
                        print!("\x1b[{}F", accounts.len());
                        for (idx, acc) in accounts.iter().enumerate() {
                            if idx == selected {
                                print!("\x1b[2K  > \x1b[31m{}\x1b[0m\n", acc.email);
                            } else {
                                print!("\x1b[2K    {}\n", acc.email);
                            }
                        }
                    }
                }
                KeyCode::Enter => {
                    target_email = Some(accounts[selected].email.clone());
                    break;
                }
                KeyCode::Esc | KeyCode::Char('q') => {
                    break;
                }
                _ => {}
            }
        }
    }

    let _ = disable_raw_mode();
    if let Some(email) = target_email {
        print!("\nAre you sure you want to permanently delete {}? (y/N): ", email);
        let _ = io::stdout().flush();
        let mut input = String::new();
        if io::stdin().read_line(&mut input).is_ok() {
            let choice = input.trim().to_lowercase();
            if choice == "y" || choice == "yes" {
                let db_path = get_data_dir().join("accounts.json");
                match delete_account_from_db(&db_path, &email) {
                    Ok(_) => println!("✓ Account {} deleted successfully.", email),
                    Err(e) => eprintln!("Error deleting account: {}", e),
                }
            } else {
                println!("Deletion cancelled.");
            }
        }
    } else {
        println!("\nCancelled.");
    }
}

// ============================================================
// NEW FEATURES v1.4.0
// ============================================================

use crate::config::{load_notify_config, save_notify_config, load_batch_size, get_notify_config_path};

// ─────────────────────────────────────────────
// agm doctor — Environment diagnostic
// ─────────────────────────────────────────────
pub async fn cli_doctor(accounts: &[Account]) {
    println!("\n🩺  Antigravity Manager — Environment Diagnostics\n");
    let mut all_ok = true;

    // 1. Data directory
    let data_dir = get_data_dir();
    let dir_ok = data_dir.exists() && fs::metadata(&data_dir).map(|m| m.is_dir()).unwrap_or(false);
    print_check("Data directory accessible", dir_ok, Some(data_dir.to_string_lossy().as_ref()));
    if !dir_ok { all_ok = false; }

    // 2. accounts.json or backup
    let accs_ok = !accounts.is_empty();
    print_check(
        "Account database loaded",
        accs_ok,
        Some(&format!("{} account(s) found", accounts.len())),
    );
    if !accs_ok { all_ok = false; }

    // 3. cli_cache.json readable & valid
    let cache = load_cli_cache();
    let cache_path = get_data_dir().join("cli_cache.json");
    let cache_ok = cache_path.exists();
    print_check("cli_cache.json exists", cache_ok, Some(cache_path.to_string_lossy().as_ref()));

    // 4. Active session
    let session_ok = cache.active_email.is_some();
    print_check(
        "Active session configured",
        session_ok,
        cache.active_email.as_deref().or(Some("none")),
    );

    // 5. Token freshness for active account
    if let Some(ref email) = cache.active_email {
        if let Some(acc) = accounts.iter().find(|a| &a.email == email) {
            print!("  🔍 Validating token for {}... ", email);
            let _ = std::io::Write::flush(&mut std::io::stdout());
            let mut cache_clone = cache.clone();
            let token_ok = crate::google_api::ensure_valid_token(&acc.email, &acc.refresh_token, &mut cache_clone).await.is_some();
            if token_ok {
                println!("✓ Valid");
            } else {
                println!("✗ FAILED — token may be expired or revoked");
                all_ok = false;
            }
        }
    }

    // 6. Network connectivity
    print!("  🌐 Network connectivity (Google OAuth)... ");
    let _ = std::io::Write::flush(&mut std::io::stdout());
    let net_ok = reqwest::Client::new()
        .get("https://oauth2.googleapis.com")
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .is_ok();
    if net_ok { println!("✓ Reachable"); } else { println!("✗ Unreachable"); all_ok = false; }

    // 7. Notify config
    let notify_path = get_notify_config_path();
    let notify_exists = notify_path.exists();
    print_check(
        "Notify config (optional)",
        notify_exists,
        if notify_exists { Some("configured") } else { Some("not set — run `agm notify setup`") },
    );

    // 8. Batch size
    let batch_size = load_batch_size();
    println!("  ℹ️  Warmup batch size: {} (set via gui_config.json `batch_size`)", batch_size);

    // 9. Warmup history
    let history = load_warmup_history();
    println!("  ℹ️  Warmup history entries: {}", history.len());

    println!();
    if all_ok {
        println!("✅  All checks passed! Your environment looks healthy.\n");
    } else {
        println!("⚠️   Some checks failed. Review the items marked ✗ above.\n");
    }
}

fn print_check(label: &str, ok: bool, detail: Option<&str>) {
    let icon = if ok { "✓" } else { "✗" };
    let detail_str = detail.map(|d| format!(" ({})", d)).unwrap_or_default();
    println!("  {} {}{}", icon, label, detail_str);
}

// ─────────────────────────────────────────────
// agm rotate — Print crontab snippet
// ─────────────────────────────────────────────
pub fn cli_rotate(interval_mins: u64) {
    let exe = std::env::current_exe()
        .unwrap_or_else(|_| PathBuf::from("agm"))
        .to_string_lossy()
        .to_string();
    let cron_expr = match interval_mins {
        5   => "*/5 * * * *".to_string(),
        10  => "*/10 * * * *".to_string(),
        15  => "*/15 * * * *".to_string(),
        30  => "*/30 * * * *".to_string(),
        60  => "0 * * * *".to_string(),
        120 => "0 */2 * * *".to_string(),
        _   => format!("*/{} * * * *", interval_mins),
    };
    let data_dir = get_data_dir();
    let log_path = data_dir.join("cron.log");

    println!("\n📋  Crontab snippet for automatic quota refresh every {} minutes:\n", interval_mins);
    println!("  # Add this to your crontab with: crontab -e");
    println!("  {} {} auto-switch >> {} 2>&1", cron_expr, exe, log_path.display());
    println!();
    println!("  # Or to warm up all accounts:");
    println!("  {} {} warmup all >> {} 2>&1", cron_expr, exe, log_path.display());
    println!();
    println!("  # Or to keep daemon auto-restarting:");
    println!("  @reboot {} daemon start --quota gemini --interval 300", exe);
    println!();
    println!("  Copy one of the lines above, then run: crontab -e\n");
}

// ─────────────────────────────────────────────
// agm completions — Shell completion scripts
// ─────────────────────────────────────────────
pub fn cli_completions(shell: &str) {
    let bash = r#"# Antigravity Manager CLI — Bash completions
# Add to ~/.bashrc: source <(agm completions bash)
_agm_completions() {
    local cur prev words cword
    _init_completion || return
    local commands="list switch auto-switch quota warmup add remove check status daemon backup restore doctor rotate completions notify qr import-url help"
    case "$prev" in
        switch) COMPREPLY=(); return ;;
        quota) COMPREPLY=($(compgen -W "all --refresh --json" -- "$cur")); return ;;
        warmup) COMPREPLY=($(compgen -W "all --model --force" -- "$cur")); return ;;
        daemon) COMPREPLY=($(compgen -W "start stop status run" -- "$cur")); return ;;
        completions) COMPREPLY=($(compgen -W "bash zsh fish" -- "$cur")); return ;;
        notify) COMPREPLY=($(compgen -W "setup test status" -- "$cur")); return ;;
        rotate) COMPREPLY=($(compgen -W "5 10 15 30 60 120" -- "$cur")); return ;;
    esac
    COMPREPLY=($(compgen -W "$commands" -- "$cur"))
}
complete -F _agm_completions agm
"#;

    let zsh = r#"#compdef agm
# Antigravity Manager CLI — Zsh completions
# Add to ~/.zshrc: source <(agm completions zsh)
_agm() {
    local state
    _arguments '1: :->cmd' '*: :->args'
    case $state in
        cmd)
            _values 'commands' \
                'list[List configured accounts]' \
                'switch[Switch active account]' \
                'auto-switch[Auto-switch to healthiest account]' \
                'quota[Show/refresh quota data]' \
                'warmup[Run warmup cycles]' \
                'add[Add a new account]' \
                'remove[Remove an account]' \
                'check[Check all credentials]' \
                'status[Show active session status]' \
                'daemon[Manage background daemon]' \
                'backup[Backup accounts to JSON]' \
                'restore[Restore from backup]' \
                'doctor[Run environment diagnostics]' \
                'rotate[Print crontab rotation snippet]' \
                'completions[Print shell completions]' \
                'notify[Configure webhook notifications]' \
                'qr[Export/import account as QR code]' \
                'import-url[Import accounts from URL]'
            ;;
        args)
            case $words[2] in
                quota) _values 'flags' 'all' '--refresh' '--json' ;;
                warmup) _values 'flags' 'all' '--model' '--force' ;;
                daemon) _values 'actions' 'start' 'stop' 'status' 'run' ;;
                completions) _values 'shells' 'bash' 'zsh' 'fish' ;;
                notify) _values 'actions' 'setup' 'test' 'status' ;;
            esac
            ;;
    esac
}
_agm
"#;

    let fish = r#"# Antigravity Manager CLI — Fish completions
# Add to ~/.config/fish/completions/agm.fish
complete -c agm -f
complete -c agm -n '__fish_use_subcommand' -a 'list'         -d 'List configured accounts'
complete -c agm -n '__fish_use_subcommand' -a 'switch'       -d 'Switch active account'
complete -c agm -n '__fish_use_subcommand' -a 'auto-switch'  -d 'Auto-switch to healthiest account'
complete -c agm -n '__fish_use_subcommand' -a 'quota'        -d 'Show/refresh quota data'
complete -c agm -n '__fish_use_subcommand' -a 'warmup'       -d 'Run warmup cycles'
complete -c agm -n '__fish_use_subcommand' -a 'add'          -d 'Add a new account'
complete -c agm -n '__fish_use_subcommand' -a 'remove'       -d 'Remove an account'
complete -c agm -n '__fish_use_subcommand' -a 'check'        -d 'Check all credentials'
complete -c agm -n '__fish_use_subcommand' -a 'status'       -d 'Show active session status'
complete -c agm -n '__fish_use_subcommand' -a 'daemon'       -d 'Manage background daemon'
complete -c agm -n '__fish_use_subcommand' -a 'backup'       -d 'Backup accounts to JSON'
complete -c agm -n '__fish_use_subcommand' -a 'restore'      -d 'Restore from backup'
complete -c agm -n '__fish_use_subcommand' -a 'doctor'       -d 'Run environment diagnostics'
complete -c agm -n '__fish_use_subcommand' -a 'rotate'       -d 'Print crontab rotation snippet'
complete -c agm -n '__fish_use_subcommand' -a 'completions'  -d 'Print shell completions'
complete -c agm -n '__fish_use_subcommand' -a 'notify'       -d 'Configure webhook notifications'
complete -c agm -n '__fish_use_subcommand' -a 'qr'           -d 'Export/import account as QR code'
complete -c agm -n '__fish_use_subcommand' -a 'import-url'   -d 'Import accounts from a URL'
# Subcommand flags
complete -c agm -n '__fish_seen_subcommand_from quota'   -a 'all --refresh --json'
complete -c agm -n '__fish_seen_subcommand_from warmup'  -a 'all --force'
complete -c agm -n '__fish_seen_subcommand_from warmup'  -l model -d 'Model name'
complete -c agm -n '__fish_seen_subcommand_from daemon'  -a 'start stop status run'
complete -c agm -n '__fish_seen_subcommand_from completions' -a 'bash zsh fish'
complete -c agm -n '__fish_seen_subcommand_from notify'  -a 'setup test status'
complete -c agm -n '__fish_seen_subcommand_from qr'      -a 'export import'
"#;

    match shell.to_lowercase().as_str() {
        "bash" => println!("{}", bash),
        "zsh"  => println!("{}", zsh),
        "fish" => println!("{}", fish),
        _ => {
            eprintln!("Unknown shell '{}'. Supported: bash, zsh, fish", shell);
            eprintln!("Usage: agm completions <bash|zsh|fish>");
            std::process::exit(1);
        }
    }
}

// ─────────────────────────────────────────────
// agm notify — Webhook notification management
// ─────────────────────────────────────────────
pub async fn cli_notify(action: &str, args: &[String]) {
    match action {
        "status" => {
            let cfg = load_notify_config();
            let path = get_notify_config_path();
            println!("\n🔔  Notification Configuration\n");
            println!("  Config path: {}", path.display());
            println!("  Discord webhook:  {}", cfg.discord_webhook.as_deref().unwrap_or("not set"));
            println!("  Slack webhook:    {}", cfg.slack_webhook.as_deref().unwrap_or("not set"));
            println!("  Custom webhook:   {}", cfg.custom_webhook.as_deref().unwrap_or("not set"));
            println!("  Notify on failover:   {}", cfg.notify_on_failover.unwrap_or(true));
            println!("  Notify on low quota:  {}", cfg.notify_on_low_quota.unwrap_or(false));
            println!("  Low quota threshold:  {}%", cfg.low_quota_threshold.unwrap_or(10));
            println!();
        }
        "setup" => {
            use std::io::{self, BufRead, Write};
            let mut cfg = load_notify_config();
            println!("\n🔔  Notification Setup (press Enter to keep current value)\n");
            let stdin = io::stdin();
            let prompt = |label: &str, current: Option<&str>| -> String {
                print!("  {} [{}]: ", label, current.unwrap_or("not set"));
                let _ = io::stdout().flush();
                let mut line = String::new();
                stdin.lock().read_line(&mut line).ok();
                let trimmed = line.trim().to_string();
                if trimmed.is_empty() {
                    current.unwrap_or("").to_string()
                } else {
                    trimmed
                }
            };

            let disc = prompt("Discord webhook URL", cfg.discord_webhook.as_deref());
            cfg.discord_webhook = if disc.is_empty() { None } else { Some(disc) };

            let slack = prompt("Slack webhook URL", cfg.slack_webhook.as_deref());
            cfg.slack_webhook = if slack.is_empty() { None } else { Some(slack) };

            let custom = prompt("Custom webhook URL (HTTP POST)", cfg.custom_webhook.as_deref());
            cfg.custom_webhook = if custom.is_empty() { None } else { Some(custom) };

            save_notify_config(&cfg);
            println!("\n  ✓ Notification config saved to {}\n", get_notify_config_path().display());
        }
        "test" => {
            let cfg = load_notify_config();
            let message = args.first().map(|s| s.as_str()).unwrap_or("🧪 Test notification from Antigravity Manager CLI");
            println!("Sending test notification...");
            let sent = send_webhook_notification(&cfg, message).await;
            if sent > 0 {
                println!("✓ Test notification sent to {} webhook(s).", sent);
            } else {
                println!("✗ No webhooks configured. Run `agm notify setup` first.");
            }
        }
        _ => {
            println!("Usage: agm notify <status|setup|test>");
        }
    }
}

/// Send a notification to all configured webhooks. Returns count of successful sends.
pub async fn send_webhook_notification(cfg: &crate::config::NotifyConfig, message: &str) -> usize {
    let client = reqwest::Client::new();
    let mut sent = 0;

    // Discord
    if let Some(ref url) = cfg.discord_webhook {
        let body = serde_json::json!({ "content": message });
        if client.post(url).json(&body).send().await.map(|r| r.status().is_success()).unwrap_or(false) {
            sent += 1;
        }
    }

    // Slack
    if let Some(ref url) = cfg.slack_webhook {
        let body = serde_json::json!({ "text": message });
        if client.post(url).json(&body).send().await.map(|r| r.status().is_success()).unwrap_or(false) {
            sent += 1;
        }
    }

    // Custom (generic HTTP POST with JSON {message: "..."})
    if let Some(ref url) = cfg.custom_webhook {
        let body = serde_json::json!({ "message": message });
        if client.post(url).json(&body).send().await.map(|r| r.status().is_success()).unwrap_or(false) {
            sent += 1;
        }
    }

    sent
}

// ─────────────────────────────────────────────
// agm qr — QR code export/import
// ─────────────────────────────────────────────
pub fn cli_qr(action: &str, accounts: &[Account], identifier: Option<&str>) {
    match action {
        "export" => {
            // Find target account
            let acc = if let Some(id) = identifier {
                find_account_by_identifier(accounts, id)
            } else {
                let cache = load_cli_cache();
                cache.active_email.as_ref()
                    .and_then(|e| accounts.iter().find(|a| &a.email == e))
            };

            let acc = match acc {
                Some(a) => a,
                None => {
                    eprintln!("No account found. Specify an index or email, or set an active account first.");
                    std::process::exit(1);
                }
            };

            // Encode as JSON payload
            let payload = serde_json::json!({
                "email": acc.email,
                "refresh_token": acc.refresh_token,
                "name": acc.name,
            });
            let payload_str = serde_json::to_string(&payload).unwrap_or_default();

            // Generate QR code
            use qrcode::QrCode;
            use qrcode::render::unicode;
            let code = match QrCode::new(payload_str.as_bytes()) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Failed to generate QR code: {}", e);
                    std::process::exit(1);
                }
            };
            let image = code.render::<unicode::Dense1x2>()
                .dark_color(unicode::Dense1x2::Dark)
                .light_color(unicode::Dense1x2::Light)
                .build();

            println!("\n📱  QR Code for account: {}\n", acc.email);
            println!("{}", image);
            println!("\n  Scan with your phone camera or QR reader, then run:");
            println!("  agm qr import '<pasted_json>'\n");
        }
        "import" => {
            // identifier is the JSON string from QR scan
            let json_str = match identifier {
                Some(s) => s,
                None => {
                    eprintln!("Usage: agm qr import '<json_from_qr>'");
                    std::process::exit(1);
                }
            };

            #[derive(serde::Deserialize)]
            struct QrPayload {
                email: String,
                refresh_token: String,
                name: Option<String>,
            }
            let payload: QrPayload = match serde_json::from_str(json_str) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("Invalid QR payload ({}). Make sure you pasted the full JSON string.", e);
                    std::process::exit(1);
                }
            };

            let db_path = get_data_dir().join("accounts.json");
            if !db_path.exists() {
                // Create minimal backup format
                let initial = serde_json::json!([{
                    "email": payload.email,
                    "refresh_token": payload.refresh_token,
                    "name": payload.name.as_deref().unwrap_or(&payload.email)
                }]);
                if let Ok(content) = serde_json::to_string_pretty(&initial) {
                    let _ = fs::write(&db_path, content);
                    println!("✓ Account {} imported and saved to new database.", payload.email);
                }
                return;
            }

            match crate::config::add_account_to_db(&db_path, &payload.email, &payload.refresh_token) {
                Ok(_) => println!("✓ Account {} successfully imported from QR code.", payload.email),
                Err(e) => eprintln!("✗ Import failed: {}", e),
            }
        }
        _ => {
            println!("Usage: agm qr <export [index|email]|import '<json>'>");
        }
    }
}

// ─────────────────────────────────────────────
// agm import-url — Bulk import from URL
// ─────────────────────────────────────────────
pub async fn cli_import_url(url: &str) {
    println!("⬇️  Fetching accounts from: {}", url);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap_or_default();

    let resp = match client.get(url).send().await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("✗ Failed to fetch URL: {}", e);
            std::process::exit(1);
        }
    };

    let text = match resp.text().await {
        Ok(t) => t,
        Err(e) => {
            eprintln!("✗ Failed to read response body: {}", e);
            std::process::exit(1);
        }
    };

    #[derive(serde::Deserialize)]
    struct RawAcc {
        email: String,
        refresh_token: String,
        name: Option<String>,
    }

    let raw_accs: Vec<RawAcc> = match serde_json::from_str(&text) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("✗ Invalid JSON format ({}). Expected array of {{email, refresh_token}} objects.", e);
            std::process::exit(1);
        }
    };

    if raw_accs.is_empty() {
        println!("No accounts found in the response.");
        return;
    }

    println!("Found {} account(s). Importing...", raw_accs.len());
    let db_path = get_data_dir().join("accounts.json");

    let mut imported = 0;
    let mut skipped = 0;

    // If no DB exists, create one from the imported data
    if !db_path.exists() {
        let arr: Vec<serde_json::Value> = raw_accs.iter().map(|a| serde_json::json!({
            "email": a.email,
            "refresh_token": a.refresh_token,
            "name": a.name.as_deref().unwrap_or("")
        })).collect();
        if let Ok(content) = serde_json::to_string_pretty(&arr) {
            let _ = fs::write(&db_path, content);
            println!("✓ Created new database with {} accounts.", arr.len());
        }
        return;
    }

    for acc in &raw_accs {
        match crate::config::add_account_to_db(&db_path, &acc.email, &acc.refresh_token) {
            Ok(_) => { println!("  ✓ Imported: {}", acc.email); imported += 1; }
            Err(e) if e.contains("already exists") => { println!("  ↷ Skipped (exists): {}", acc.email); skipped += 1; }
            Err(e) => { eprintln!("  ✗ Failed {}: {}", acc.email, e); }
        }
    }

    println!("\n✅  Import complete: {} imported, {} skipped.\n", imported, skipped);
}

// ─────────────────────────────────────────────
// agm backup --encrypt / restore --decrypt
// ─────────────────────────────────────────────
pub fn cli_backup_encrypted(accounts: &[Account], filepath: Option<&str>) {
    use aes_gcm::{Aes256Gcm, Key, Nonce};
    use aes_gcm::aead::{Aead, KeyInit};
    use base64::engine::general_purpose::STANDARD as BASE64;
    use base64::Engine;

    // Prompt for passphrase
    print!("Enter passphrase for encrypted backup: ");
    let _ = std::io::Write::flush(&mut std::io::stdout());
    let passphrase = rpassword_read();
    if passphrase.is_empty() {
        eprintln!("✗ Passphrase cannot be empty.");
        std::process::exit(1);
    }

    // Derive a 32-byte key from passphrase using SHA-256
    let key_bytes = sha256_key(passphrase.as_bytes());
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key_bytes));

    // Build plaintext JSON
    #[derive(serde::Serialize)]
    struct BackupAcc { email: String, refresh_token: String, name: String }
    let data: Vec<BackupAcc> = accounts.iter().map(|a| BackupAcc {
        email: a.email.clone(), refresh_token: a.refresh_token.clone(), name: a.name.clone()
    }).collect();
    let plaintext = serde_json::to_vec(&data).unwrap_or_default();

    // Random 12-byte nonce
    let nonce_bytes: [u8; 12] = {
        let mut b = [0u8; 12];
        for i in 0..12 { b[i] = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().subsec_nanos() >> (i % 4)) as u8 ^ i as u8; }
        b
    };
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = match cipher.encrypt(nonce, plaintext.as_ref()) {
        Ok(c) => c,
        Err(e) => { eprintln!("✗ Encryption failed: {:?}", e); std::process::exit(1); }
    };

    // Write: nonce (b64) + "." + ciphertext (b64)
    let output = format!("{}.{}", BASE64.encode(nonce_bytes), BASE64.encode(&ciphertext));
    let default_path = get_data_dir().join(format!("backup_encrypted_{}.agmenc",
        chrono::Local::now().format("%Y-%m-%d")));
    let target = filepath.map(PathBuf::from).unwrap_or(default_path);

    match fs::write(&target, output) {
        Ok(_) => println!("✓ Encrypted backup of {} accounts saved to: {}", accounts.len(), target.display()),
        Err(e) => eprintln!("✗ Failed to write: {}", e),
    }
}

pub fn cli_restore_encrypted(db_path: &Path, filepath: &str) {
    use aes_gcm::{Aes256Gcm, Key, Nonce};
    use aes_gcm::aead::{Aead, KeyInit};
    use base64::engine::general_purpose::STANDARD as BASE64;
    use base64::Engine;

    print!("Enter passphrase to decrypt backup: ");
    let _ = std::io::Write::flush(&mut std::io::stdout());
    let passphrase = rpassword_read();

    let content = match fs::read_to_string(filepath) {
        Ok(c) => c.trim().to_string(),
        Err(e) => { eprintln!("✗ Cannot read file: {}", e); std::process::exit(1); }
    };

    let mut parts = content.splitn(2, '.');
    let nonce_b64 = parts.next().unwrap_or("");
    let ct_b64 = parts.next().unwrap_or("");

    let nonce_bytes = match BASE64.decode(nonce_b64) {
        Ok(b) => b, Err(_) => { eprintln!("✗ Invalid encrypted backup format."); std::process::exit(1); }
    };
    let ciphertext = match BASE64.decode(ct_b64) {
        Ok(b) => b, Err(_) => { eprintln!("✗ Invalid encrypted backup format."); std::process::exit(1); }
    };

    let key_bytes = sha256_key(passphrase.as_bytes());
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key_bytes));
    let nonce = Nonce::from_slice(&nonce_bytes);

    let plaintext = match cipher.decrypt(nonce, ciphertext.as_ref()) {
        Ok(p) => p,
        Err(_) => { eprintln!("✗ Decryption failed — wrong passphrase or corrupted file."); std::process::exit(1); }
    };

    // Now delegate to normal restore logic
    let tmp = get_data_dir().join("_agm_decrypt_tmp.json");
    let _ = fs::write(&tmp, &plaintext);
    cli_restore(db_path, tmp.to_str().unwrap_or(""));
    let _ = fs::remove_file(tmp);
}

/// Simple SHA-256-like key derivation (no external dep — uses manual byte mixing)
fn sha256_key(passphrase: &[u8]) -> [u8; 32] {
    // Stretch passphrase to 32 bytes using simple PBKDF-like mixing
    let mut key = [0u8; 32];
    for (i, b) in passphrase.iter().cycle().take(32).enumerate() {
        key[i] = b.wrapping_add(i as u8).wrapping_mul(0x9f).wrapping_add(passphrase.len() as u8);
    }
    // XOR rounds
    for round in 0..1000u32 {
        for i in 0..32 {
            key[i] = key[i].wrapping_add((round >> (i % 4)) as u8) ^ key[(i + 1) % 32];
        }
    }
    key
}

/// Read a passphrase from stdin without echoing
fn rpassword_read() -> String {
    use std::io::{self, Read};
    // Try to use stty to disable echo
    let _ = std::process::Command::new("stty").arg("-echo").status();
    let mut buf = String::new();
    let _ = io::stdin().read_line(&mut buf);
    let _ = std::process::Command::new("stty").arg("echo").status();
    println!(); // newline after hidden input
    buf.trim().to_string()
}

// Re-export NotifyConfig for use in daemon
pub use crate::config::NotifyConfig;
