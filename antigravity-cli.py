#!/usr/bin/env python3
import os
import sys
import json
import time
import uuid
import datetime
import subprocess
import base64
import argparse
import requests

# Constants matching Rust backend
CLIENT_ID = "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com"
CLIENT_SECRET = "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"
TOKEN_URL = "https://oauth2.googleapis.com/token"

# Cooldown duration: 4 hours (14400 seconds)
COOLDOWN_SECONDS = 14400

# Color logging helpers
def print_success(msg):
    print(f"\033[92m✓ {msg}\033[0m")

def print_warning(msg):
    print(f"\033[93m⚠️  {msg}\033[0m")

def print_error(msg):
    print(f"\033[91m❌ {msg}\033[0m", file=sys.stderr)

def print_info(msg):
    print(f"\033[94mℹ {msg}\033[0m")

def print_bold(msg):
    print(f"\033[1m{msg}\033[0m")

# Get path to data directory
def get_data_dir():
    env_path = os.environ.get("ABV_DATA_DIR")
    if env_path and env_path.strip():
        data_dir = os.path.expanduser(env_path.strip())
    else:
        data_dir = os.path.expanduser("~/.antigravity_tools")
    
    if not os.path.exists(data_dir):
        os.makedirs(data_dir, exist_ok=True)
    return data_dir

# CLI Cache persistence (cli_cache.json)
def get_cli_cache_path():
    return os.path.join(get_data_dir(), "cli_cache.json")

def load_cli_cache():
    path = get_cli_cache_path()
    if os.path.exists(path):
        try:
            with open(path, 'r', encoding='utf-8') as f:
                return json.load(f)
        except Exception:
            return {"active_email": None, "tokens": {}, "quotas": {}}
    return {"active_email": None, "tokens": {}, "quotas": {}}

def save_cli_cache(cache):
    path = get_cli_cache_path()
    try:
        with open(path, 'w', encoding='utf-8') as f:
            json.dump(cache, f, indent=2, ensure_ascii=False)
    except Exception as e:
        print_warning(f"Failed to save cli_cache.json: {e}")

# Load accounts from Backup JSON or official accounts.json fallback
def load_accounts(backup_path):
    # Try loading from specified backup path first
    if backup_path and os.path.exists(backup_path):
        try:
            with open(backup_path, 'r', encoding='utf-8') as f:
                raw = json.load(f)
                # Normalize format: ensure it's a list of dicts with email and refresh_token
                accounts = []
                for item in raw:
                    if 'email' in item and 'refresh_token' in item:
                        accounts.append({
                            "email": item["email"],
                            "refresh_token": item["refresh_token"],
                            "name": item.get("name", item["email"].split('@')[0].capitalize()),
                            "source": f"backup ({os.path.basename(backup_path)})"
                        })
                if accounts:
                    return accounts, backup_path
        except Exception as e:
            print_warning(f"Failed to load backup file {backup_path}: {e}. Falling back to Tauri configuration.")
            
    # Fallback: Official accounts.json
    data_dir = get_data_dir()
    index_path = os.path.join(data_dir, "accounts.json")
    if os.path.exists(index_path):
        try:
            with open(index_path, 'r', encoding='utf-8') as f:
                content = f.read().replace('\xef\xbb\xbf', '').strip().replace('\x00', '')
                index_data = json.loads(content)
                accounts = []
                for acc in index_data.get("accounts", []):
                    # Load details to get refresh token
                    acc_path = os.path.join(data_dir, "accounts", f"{acc['id']}.json")
                    if os.path.exists(acc_path):
                        with open(acc_path, 'r', encoding='utf-8') as af:
                            details = json.load(af)
                            rt = details.get('token', {}).get('refresh_token')
                            if rt:
                                accounts.append({
                                    "email": acc["email"],
                                    "refresh_token": rt,
                                    "name": acc.get("name"),
                                    "id": acc["id"],
                                    "source": "tauri index"
                                })
                if accounts:
                    return accounts, index_path
        except Exception as e:
            print_warning(f"Failed to load official accounts.json: {e}")
            
    return [], None

# Get active account email
def get_active_email(accounts):
    cache = load_cli_cache()
    active_email = cache.get("active_email")
    if active_email:
        # Verify it still exists in our accounts list
        if any(a["email"].lower() == active_email.lower() for a in accounts):
            return active_email
            
    # Fallback to official active account in accounts.json
    index_path = os.path.join(get_data_dir(), "accounts.json")
    if os.path.exists(index_path):
        try:
            with open(index_path, 'r', encoding='utf-8') as f:
                index_data = json.loads(f.read().replace('\xef\xbb\xbf', '').strip())
                curr_id = index_data.get("current_account_id")
                if curr_id:
                    for acc in index_data.get("accounts", []):
                        if acc["id"] == curr_id:
                            return acc["email"]
        except Exception:
            pass
            
    # Fallback to first account in list
    if accounts:
        return accounts[0]["email"]
    return None

# Keyring integration
def store_token_keyring(email, access_token, refresh_token, expiry_timestamp):
    expiry_datetime = datetime.datetime.fromtimestamp(expiry_timestamp, tz=datetime.timezone.utc)
    expiry_str = expiry_datetime.strftime('%Y-%m-%dT%H:%M:%S.%f')[:-3] + 'Z'
    
    payload = {
        "token": {
            "access_token": access_token,
            "token_type": "Bearer",
            "refresh_token": refresh_token,
            "expiry": expiry_str
        },
        "auth_method": "consumer"
    }
    payload_json = json.dumps(payload, separators=(',', ':'))
    
    system = sys.platform
    if system == "linux":
        try:
            proc = subprocess.Popen(
                ['secret-tool', 'store', '--label=gemini', 'service', 'gemini', 'username', 'antigravity'],
                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True
            )
            stdout, stderr = proc.communicate(input=payload_json)
            return proc.returncode == 0
        except Exception as e:
            print_error(f"Failed to run secret-tool: {e}")
            return False
            
    elif system == "darwin":
        try:
            subprocess.run(['security', 'delete-generic-password', '-s', 'gemini', '-a', 'antigravity'], capture_output=True)
            encoded = base64.b64encode(payload_json.encode('utf-8')).decode('utf-8')
            full_val = f"go-keyring-base64:{encoded}"
            proc = subprocess.run([
                'security', 'add-generic-password',
                '-s', 'gemini', '-a', 'antigravity',
                '-w', full_val, '-A'
            ], capture_output=True)
            return proc.returncode == 0
        except Exception as e:
            print_error(f"Failed to write to macOS keychain: {e}")
            return False
            
    elif system == "win32":
        try:
            import ctypes
            from ctypes import wintypes
            
            class FILETIME(ctypes.Structure):
                _fields_ = [("dwLowDateTime", wintypes.DWORD), ("dwHighDateTime", wintypes.DWORD)]
                
            class CREDENTIALW(ctypes.Structure):
                _fields_ = [
                    ("Flags", wintypes.DWORD), ("Type", wintypes.DWORD),
                    ("TargetName", wintypes.LPWSTR), ("Comment", wintypes.LPWSTR),
                    ("LastWritten", FILETIME), ("CredentialBlobSize", wintypes.DWORD),
                    ("CredentialBlob", ctypes.c_void_p), ("Persist", wintypes.DWORD),
                    ("AttributeCount", wintypes.DWORD), ("Attributes", ctypes.c_void_p),
                    ("TargetAlias", wintypes.LPWSTR), ("UserName", wintypes.LPWSTR)
                ]
                
            advapi32 = ctypes.windll.advapi32
            target = "gemini:antigravity"
            user = "antigravity"
            secret_bytes = payload_json.encode('utf-8')
            
            advapi32.CredDeleteW(target, 1, 0)
            
            cred = CREDENTIALW()
            cred.Flags = 0
            cred.Type = 1
            cred.TargetName = target
            cred.Comment = None
            cred.LastWritten = FILETIME(0, 0)
            cred.CredentialBlobSize = len(secret_bytes)
            cred.CredentialBlob = ctypes.cast(ctypes.create_string_buffer(secret_bytes), ctypes.c_void_p)
            cred.Persist = 2
            cred.AttributeCount = 0
            cred.Attributes = None
            cred.TargetAlias = None
            cred.UserName = user
            
            return advapi32.CredWriteW(ctypes.byref(cred), 0) != 0
        except Exception as e:
            print_error(f"Failed to write Windows Credential: {e}")
            return False
            
    return False

# Token validation and refresh
def refresh_access_token(refresh_token, email=None):
    if email:
        print_info(f"Refreshing access token for {email}...")
    else:
        print_info("Refreshing access token...")
        
    payload = {
        'client_id': CLIENT_ID,
        'client_secret': CLIENT_SECRET,
        'refresh_token': refresh_token,
        'grant_type': 'refresh_token'
    }
    headers = {'User-Agent': 'vscode/1.X.X (Antigravity/4.3.0)'}
    
    try:
        resp = requests.post(TOKEN_URL, data=payload, headers=headers, timeout=30)
        if resp.status_code == 200:
            data = resp.json()
            return {
                'access_token': data['access_token'],
                'expires_in': data['expires_in'],
                'expiry_timestamp': int(time.time()) + data['expires_in']
            }
        else:
            print_error(f"Google Token endpoint returned {resp.status_code}: {resp.text}")
            return None
    except Exception as e:
        print_error(f"Network error refreshing token: {e}")
        return None

def ensure_valid_token(email, refresh_token):
    cache = load_cli_cache()
    token_info = cache.get("tokens", {}).get(email)
    
    now = int(time.time())
    if token_info and token_info.get("expiry_timestamp", 0) > now + 300:
        return token_info["access_token"], token_info.get("project_id")
        
    # Expired or missing, refresh it
    new_tokens = refresh_access_token(refresh_token, email)
    if not new_tokens:
        return None, None
        
    # Fetch project ID and tier as well
    project_id, tier = fetch_project_id_and_tier(new_tokens['access_token'], email)
    
    # Update cache
    if "tokens" not in cache:
        cache["tokens"] = {}
    cache["tokens"][email] = {
        "access_token": new_tokens["access_token"],
        "expiry_timestamp": new_tokens["expiry_timestamp"],
        "project_id": project_id,
        "subscription_tier": tier
    }
    save_cli_cache(cache)
    
    # If active account, write new token to system keyring
    active_email = cache.get("active_email")
    if active_email and active_email.lower() == email.lower():
        store_token_keyring(email, new_tokens["access_token"], refresh_token, new_tokens["expiry_timestamp"])
        
    return new_tokens["access_token"], project_id

# Quota fetching and tier checks
def fetch_project_id_and_tier(access_token, email):
    url = "https://daily-cloudcode-pa.sandbox.googleapis.com/v1internal:loadCodeAssist"
    payload = {"metadata": {"ideType": "ANTIGRAVITY"}}
    headers = {
        "Authorization": f"Bearer {access_token}",
        "Content-Type": "application/json",
        "User-Agent": "vscode/1.X.X (Antigravity/4.3.0)"
    }
    try:
        resp = requests.post(url, json=payload, headers=headers, timeout=15)
        if resp.status_code == 200:
            data = resp.json()
            project_id = data.get("cloudaicompanionProject")
            
            paid_tier = data.get("paidTier") or data.get("paid_tier")
            current_tier = data.get("currentTier") or data.get("current_tier")
            allowed_tiers = data.get("allowedTiers") or data.get("allowed_tiers") or []
            
            tier_name = None
            if paid_tier:
                tier_name = paid_tier.get("name") or paid_tier.get("id")
            if not tier_name:
                ineligible = data.get("ineligibleTiers") or data.get("ineligible_tiers")
                is_ineligible = ineligible and len(ineligible) > 0
                if not is_ineligible and current_tier:
                    tier_name = current_tier.get("name") or current_tier.get("id")
                elif allowed_tiers:
                    default_tier = next((t for t in allowed_tiers if t.get("is_default") or t.get("isDefault")), None)
                    if default_tier:
                        tier_name = default_tier.get("name") or default_tier.get("id")
                        if tier_name:
                            tier_name = f"{tier_name} (Restricted)"
            return project_id, tier_name
    except Exception:
        pass
    return None, None

def fetch_quota_from_api(access_token, project_id=None):
    urls = [
        "https://daily-cloudcode-pa.sandbox.googleapis.com/v1internal:fetchAvailableModels",
        "https://daily-cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels",
        "https://cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels"
    ]
    payload = {"project": project_id} if project_id else {}
    headers = {
        "Authorization": f"Bearer {access_token}",
        "Content-Type": "application/json",
        "User-Agent": "vscode/1.X.X (Antigravity/4.3.0)"
    }
    
    last_err = None
    for url in urls:
        try:
            resp = requests.post(url, json=payload, headers=headers, timeout=15)
            if resp.status_code == 200:
                return resp.json(), None
            elif resp.status_code == 403:
                if project_id:
                    resp_no_proj = requests.post(url, json={}, headers=headers, timeout=15)
                    if resp_no_proj.status_code == 200:
                        return resp_no_proj.json(), None
                return {"is_forbidden": True}, f"403 Forbidden: {resp.text}"
            else:
                last_err = f"HTTP {resp.status_code}: {resp.text}"
        except Exception as e:
            last_err = str(e)
            
    return None, last_err

# Warmup execution
def trigger_warmup_api(access_token, model_name, project_id, email):
    timestamp_ms = int(time.time() * 1000)
    random_hex = uuid.uuid4().hex[:8]
    request_id = f"agent/{timestamp_ms}/{random_hex}"
    
    is_enterprise = not (email.endswith("@gmail.com") or email.endswith("@googlemail.com"))
    user_agent = "jetski" if is_enterprise else "antigravity"
    
    body = {
        "project": project_id,
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
    }
    
    headers = {
        "Authorization": f"Bearer {access_token}",
        "Content-Type": "application/json",
        "User-Agent": "vscode/1.X.X (Antigravity/4.3.0)"
    }
    
    urls = [
        "https://daily-cloudcode-pa.sandbox.googleapis.com/v1internal:generateContent",
        "https://daily-cloudcode-pa.googleapis.com/v1internal:generateContent",
        "https://cloudcode-pa.googleapis.com/v1internal:generateContent"
    ]
    
    last_err = None
    for url in urls:
        try:
            resp = requests.post(url, json=body, headers=headers, timeout=30)
            if resp.status_code == 200:
                return True, resp.text
            else:
                last_err = f"HTTP {resp.status_code}: {resp.text}"
        except Exception as e:
            last_err = str(e)
            
    return False, last_err

# Warmup Cooldown check helpers
def get_warmup_history_path():
    return os.path.join(get_data_dir(), "warmup_history.json")

def load_warmup_history():
    path = get_warmup_history_path()
    if os.path.exists(path):
        try:
            with open(path, 'r', encoding='utf-8') as f:
                return json.load(f)
        except Exception:
            return {}
    return {}

def save_warmup_history(history):
    path = get_warmup_history_path()
    try:
        with open(path, 'w', encoding='utf-8') as f:
            json.dump(history, f, indent=2, ensure_ascii=False)
    except Exception as e:
        print_warning(f"Failed to save warmup history: {e}")

def check_cooldown(email, model_name):
    history = load_warmup_history()
    key = f"{email}:{model_name}:100"
    last_ts = history.get(key)
    if last_ts is not None:
        elapsed = int(time.time()) - last_ts
        if elapsed < COOLDOWN_SECONDS:
            return True, COOLDOWN_SECONDS - elapsed
    return False, 0

def record_warmup_success(email, model_name):
    history = load_warmup_history()
    key = f"{email}:{model_name}:100"
    history[key] = int(time.time())
    save_warmup_history(history)

# Helper to find account by identifier
def find_account_by_identifier(accounts, identifier):
    try:
        idx = int(identifier) - 1
        if 0 <= idx < len(accounts):
            return accounts[idx]
    except ValueError:
        pass
        
    for acc in accounts:
        if acc["email"].lower() == identifier.lower():
            return acc
    return None

# Visual outputs format helper
def print_table(headers, rows):
    widths = [len(h) for h in headers]
    for row in rows:
        for i, val in enumerate(row):
            widths[i] = max(widths[i], len(str(val)))
            
    header_str = " | ".join(f"{str(h):<{widths[i]}}" for i, h in enumerate(headers))
    print_bold(header_str)
    print("-" * len(header_str))
    for row in rows:
        print(" | ".join(f"{str(val):<{widths[i]}}" for i, val in enumerate(row)))

# CLI Commands Implementation
def cmd_list(accounts, active_email, source_desc):
    if not accounts:
        print_info("No accounts resolved. Check backup JSON file path.")
        return
        
    print_bold(f"\nAccounts List (Source: {source_desc}):")
    print("=" * 60)
    
    headers = ["#", "Active", "Email", "Name"]
    rows = []
    
    for idx, acc in enumerate(accounts):
        num = idx + 1
        is_active = "*" if acc["email"].lower() == active_email.lower() else ""
        rows.append([
            num,
            is_active,
            acc["email"],
            acc["name"]
        ])
        
    print_table(headers, rows)
    print("\n* = Current active account used by Antigravity.")

def cmd_switch(accounts, identifier):
    acc = find_account_by_identifier(accounts, identifier)
    if not acc:
        print_error(f"Could not find account matching '{identifier}'.")
        sys.exit(1)
        
    email = acc["email"]
    refresh_token = acc["refresh_token"]
    
    print_info(f"Switching active account to: {email}...")
    
    # Ensure fresh tokens before switching
    access_token, project_id = ensure_valid_token(email, refresh_token)
    if not access_token:
        print_error(f"Failed to refresh credentials for {email}. Switch aborted.")
        sys.exit(1)
        
    # Update active email in cache
    cache = load_cli_cache()
    cache["active_email"] = email
    save_cli_cache(cache)
    
    # Write to system keyring/keychain
    token_info = cache.get("tokens", {}).get(email, {})
    expiry_ts = token_info.get("expiry_timestamp", int(time.time() + 3600))
    keyring_success = store_token_keyring(email, access_token, refresh_token, expiry_ts)
    
    # Update official accounts.json if it exists to maintain sync
    data_dir = get_data_dir()
    index_path = os.path.join(data_dir, "accounts.json")
    if os.path.exists(index_path) and "id" in acc:
        try:
            with open(index_path, 'r', encoding='utf-8') as f:
                index_data = json.loads(f.read().replace('\xef\xbb\xbf', '').strip())
            index_data["current_account_id"] = acc["id"]
            with open(index_path, 'w', encoding='utf-8') as f:
                json.dump(index_data, f, indent=2, ensure_ascii=False)
        except Exception:
            pass
            
    print_success(f"Active account changed to {email} ({acc['name']}).")
    if keyring_success:
        print_success("Credentials successfully written to system keyring. Cursor/IDE will pick up the token instantly.")
    else:
        print_warning("Failed to update system keyring.")

def cmd_quota(accounts, active_email, identifier=None, refresh=False):
    target_email = active_email
    if identifier:
        acc = find_account_by_identifier(accounts, identifier)
        if not acc:
            print_error(f"Could not find account matching '{identifier}'")
            sys.exit(1)
        target_email = acc["email"]
        
    if not target_email:
        print_error("No target account email resolved.")
        sys.exit(1)
        
    acc = next((a for a in accounts if a["email"].lower() == target_email.lower()), None)
    if not acc:
        print_error(f"Account {target_email} not found in database.")
        sys.exit(1)
        
    refresh_token = acc["refresh_token"]
    
    # Ensure token is valid
    access_token, project_id = ensure_valid_token(target_email, refresh_token)
    if not access_token:
        print_error(f"Refresh failed for {target_email}.")
        sys.exit(1)
        
    cache = load_cli_cache()
    
    if refresh:
        print_info(f"Fetching latest quota from Google APIs for {target_email}...")
        api_proj, tier = fetch_project_id_and_tier(access_token, target_email)
        if api_proj:
            project_id = api_proj
            cache["tokens"][target_email]["project_id"] = project_id
            
        quota_resp, err = fetch_quota_from_api(access_token, project_id)
        if err:
            print_error(f"Failed to fetch quota from API: {err}")
            sys.exit(1)
            
        # Parse quota
        models_list = []
        for name, info in quota_resp.get("models", {}).items():
            quota_info = info.get("quotaInfo")
            if quota_info:
                fraction = quota_info.get("remainingFraction", 0.0)
                percentage = int(fraction * 100.0)
                models_list.append({
                    "name": name,
                    "percentage": percentage,
                    "reset_time": quota_info.get("resetTime", ""),
                    "display_name": info.get("displayName", name)
                })
                
        # Save quota to cache
        if "quotas" not in cache:
            cache["quotas"] = {}
        cache["quotas"][target_email] = {
            "subscription_tier": tier or cache.get("tokens", {}).get(target_email, {}).get("subscription_tier") or "N/A",
            "models": models_list
        }
        save_cli_cache(cache)
        print_success("Quota cache updated successfully.")
        
    quota_data = cache.get("quotas", {}).get(target_email)
    if not quota_data or not quota_data.get("models"):
        print_warning(f"No quota data available in cache for {target_email}. Run with '--refresh' flag.")
        return
        
    print_bold(f"\nQuota for {target_email}:")
    print(f"Subscription Tier: {quota_data.get('subscription_tier', 'N/A')}")
    print(f"Project ID: {project_id or 'N/A'}")
    print("=" * 70)
    
    headers = ["Model Display Name", "Model ID", "Remaining %", "Reset Time (UTC)"]
    rows = []
    
    for m in quota_data['models']:
        rt = m.get('reset_time', '')
        if rt:
            try:
                dt = datetime.datetime.fromisoformat(rt.replace('Z', '+00:00'))
                rt = dt.strftime('%Y-%m-%d %H:%M:%S')
            except Exception:
                pass
                
        pct = m.get('percentage', 0)
        pct_str = f"{pct}%"
        if pct == 100:
            pct_str = f"\033[92m{pct_str} [READY]\033[0m"
        elif pct == 0:
            pct_str = f"\033[91m{pct_str} [LIMIT]\033[0m"
            
        rows.append([
            m.get('display_name') or m['name'],
            m['name'],
            pct_str,
            rt or "N/A"
        ])
        
    print_table(headers, rows)

def cmd_warmup(accounts, active_email, identifier=None, model_name=None, force=False):
    target_accounts = []
    if identifier:
        acc = find_account_by_identifier(accounts, identifier)
        if not acc:
            print_error(f"Could not find account matching '{identifier}'")
            sys.exit(1)
        target_accounts.append(acc)
    else:
        if not active_email:
            print_error("No active account selected.")
            sys.exit(1)
        acc = next((a for a in accounts if a["email"].lower() == active_email.lower()), None)
        if not acc:
            print_error(f"Active account {active_email} not found in database.")
            sys.exit(1)
        target_accounts.append(acc)
        
    for acc in target_accounts:
        email = acc["email"]
        refresh_token = acc["refresh_token"]
        
        print_info(f"\nStarting Warm Up sequence for account: {email}...")
        
        # Ensure fresh token
        access_token, project_id = ensure_valid_token(email, refresh_token)
        if not access_token:
            print_error(f"Skip warmup: Credentials expired and refresh failed for {email}.")
            continue
            
        cache = load_cli_cache()
        
        # If no cached quota or force is specified, pull quota first
        quota_data = cache.get("quotas", {}).get(email)
        if not quota_data or not quota_data.get("models") or force:
            print_info("Refreshing quota from Google API first...")
            if not project_id:
                project_id, _ = fetch_project_id_and_tier(access_token, email)
                if project_id:
                    cache["tokens"][email]["project_id"] = project_id
            
            quota_resp, err = fetch_quota_from_api(access_token, project_id)
            if err:
                print_error(f"Quota fetch failed for {email}: {err}")
                continue
                
            models_list = []
            for name, info in quota_resp.get("models", {}).items():
                quota_info = info.get("quotaInfo")
                if quota_info:
                    fraction = quota_info.get("remainingFraction", 0.0)
                    percentage = int(fraction * 100.0)
                    models_list.append({
                        "name": name,
                        "percentage": percentage,
                        "reset_time": quota_info.get("resetTime", ""),
                        "display_name": info.get("displayName", name)
                    })
            if "quotas" not in cache:
                cache["quotas"] = {}
            cache["quotas"][email] = {
                "subscription_tier": cache.get("tokens", {}).get(email, {}).get("subscription_tier") or "N/A",
                "models": models_list
            }
            save_cli_cache(cache)
            quota_data = cache["quotas"][email]
            
        # Extract models to warm
        models_to_warm = []
        if model_name:
            m = next((x for x in quota_data["models"] if x["name"] == model_name or x.get("display_name") == model_name), None)
            if m:
                models_to_warm.append(m)
            else:
                models_to_warm.append({"name": model_name, "percentage": 100, "display_name": model_name})
        else:
            for m in quota_data.get("models", []):
                if m.get("percentage", 0) >= 100:
                    models_to_warm.append(m)
                    
        if not models_to_warm:
            print_success(f"All models for {email} have remaining usage, no warmup needed.")
            continue
            
        warmed_count = 0
        for m in models_to_warm:
            m_name = m["name"]
            m_display = m.get("display_name") or m_name
            
            if "2.5-" in m_name or "2-5-" in m_name:
                print_info(f"Skipping warmup for {m_display} (2.5 models not supported).")
                continue
                
            if not force:
                is_cooldown, remaining = check_cooldown(email, m_name)
                if is_cooldown:
                    h = remaining // 3600
                    m_units = (remaining % 3600) // 60
                    print_info(f"Skipping warmup for {m_display}: Cooling down (cooldown expires in {h}h {m_units}m).")
                    continue
                    
            print_info(f"Warming up model {m_display}...")
            success, resp_text = trigger_warmup_api(access_token, m_name, project_id, email)
            if success:
                print_success(f"Successfully warmed up {m_display}!")
                record_warmup_success(email, m_name)
                warmed_count += 1
            else:
                print_error(f"Warmup failed for {m_display}: {resp_text}")
                
            time.sleep(1)
            
        if warmed_count > 0:
            print_success(f"Warmup complete. Triggered {warmed_count} model warmup(s).")
        else:
            print_info("No models were warmed up.")

# Main Orchestrator
def main():
    default_backup = "/home/fhrrrzy/Downloads/antigravity_accounts_2026-07-17.json"
    
    parser = argparse.ArgumentParser(
        description="Antigravity Manager CLI - Standalone accounts and warmup manager"
    )
    parser.add_argument(
        "--backup", 
        default=default_backup,
        help="Path to account backup JSON file containing email and refresh_tokens"
    )
    
    subparsers = parser.add_subparsers(dest="command", help="Available subcommands")
    
    subparsers.add_parser("list", help="List all accounts in database")
    
    switch_parser = subparsers.add_parser("switch", help="Change active account")
    switch_parser.add_argument("identifier", help="Index or Email of the account to switch to")
    
    quota_parser = subparsers.add_parser("quota", help="Display quotas")
    quota_parser.add_argument("identifier", nargs="?", help="Index or Email of target account (defaults to active)")
    quota_parser.add_argument("--refresh", action="store_true", help="Fetch fresh quota from Google companion API")
    
    warmup_parser = subparsers.add_parser("warmup", help="Perform warmup for accounts")
    warmup_parser.add_argument("identifier", nargs="?", help="Index or Email of target account (defaults to active)")
    warmup_parser.add_argument("--model", help="Target a specific model name/ID")
    warmup_parser.add_argument("--force", action="store_true", help="Ignore 100% quota and 4h cooldown requirements")
    
    args = parser.parse_args()
    
    # Resolve accounts list and active email
    accounts, db_path = load_accounts(args.backup)
    if not accounts:
        print_error(f"Failed to load accounts. Neither backup file ({args.backup}) nor official database exist.")
        sys.exit(1)
        
    source_desc = f"Backup file '{os.path.basename(db_path)}'" if db_path == args.backup else "Tauri SQLite/JSON config"
    active_email = get_active_email(accounts)
    
    if args.command == "list":
        cmd_list(accounts, active_email, source_desc)
    elif args.command == "switch":
        cmd_switch(accounts, args.identifier)
    elif args.command == "quota":
        cmd_quota(accounts, active_email, args.identifier, args.refresh)
    elif args.command == "warmup":
        cmd_warmup(accounts, active_email, args.identifier, args.model, args.force)
    else:
        parser.print_help()

if __name__ == "__main__":
    main()
