# 🚀 Antigravity Manager Tools (`agm` & `agm-tui`)

A unified suite of lightweight, standalone command-line tools for managing accounts, tracking quotas, and warming up models for the Antigravity system.

This repository contains two tools:
1. **Python CLI (`agm`)**: Designed for quick terminal commands on desktop Linux environments.
2. **Rust TUI (`agm-tui`)**: Designed as a pretty, interactive terminal user interface built with `ratatui` for mobile tablet environments (Termux) and general command lines.

---

## ⚡ Key Features

- **Double-Sided Integration**: Both tools automatically update the active account configuration and inject tokens into your system keyring (keychain / Linux secret service) so Cursor/IDE integrations pick it up instantly.
- **Visual Quota Tracking**: Shows remaining percentages and reset times for all models. The TUI features hue-shifting visual progress bars (Green 🟢, Yellow 🟡, Red 🔴).
- **Asynchronous Loop**: The TUI runs API tasks in background worker threads to keep interface navigation completely fluid.
- **Model Warm Up**: Respects the standard **4-hour cooldown** for warmups to optimize quota usage, storing history inside `~/.antigravity_tools/warmup_history.json`.

---

## 📦 Accounts Database Setup

By default, both tools read accounts directly from a backup JSON file containing emails and refresh tokens.
- **Linux Default Path**: `/home/fhrrrzy/Downloads/antigravity_accounts_2026-07-17.json`
- **Termux Default Path**: `~/.antigravity_tools/antigravity_accounts_2026-07-17.json`

If the backup file is not found, the tools automatically fallback to loading configured accounts from the Tauri desktop directory (`~/.antigravity_tools/accounts.json` and `accounts/{id}.json`).

---

## 💻 1. Linux Setup (Python CLI)

The Python CLI is located at the root as `antigravity-cli.py`.

### Installation

1. Make the script executable:
   ```bash
   chmod +x antigravity-cli.py
   ```

2. Create a global symlink or shell alias:
   ```bash
   # Symlink (Recommended)
   sudo ln -sf $(pwd)/antigravity-cli.py /usr/local/bin/agm

   # Or add to ~/.bashrc or ~/.zshrc
   alias agm="$(pwd)/antigravity-cli.py"
   ```

### Command Usage

```bash
# List all accounts
agm list

# Switch active account (by index or email)
agm switch 3

# View cached quotas (add --refresh to pull fresh data from APIs)
agm quota --refresh

# Smart warmup (pings models at 100% quota that are out of cooldown)
agm warmup
```

---

## 📱 2. Termux Tablet Setup (Rust TUI)

The Rust TUI project is located inside the `tui/` subdirectory.

### Pre-requisites (Termux)

Ensure you have Rust, cargo, and git installed in Termux:
```bash
pkg update && pkg install git rust clang -y
```

### Installation & Compilation

1. Clone this repository on the tablet:
   ```bash
   git clone <your-repo-ssh-url> ~/antigravity-manager-cli
   ```

2. Navigate into the TUI subdirectory and compile:
   ```bash
   cd ~/antigravity-manager-cli/tui
   # Build optimized release binary
   cargo build --release
   ```

3. Symlink the compiled binary to your Termux path:
   ```bash
   ln -sf ~/antigravity-manager-cli/tui/target/release/antigravity-tui /data/data/com.termux/files/usr/bin/agm-tui
   ```

4. Make sure your account backup is placed on the tablet at:
   `~/.antigravity_tools/antigravity_accounts_2026-07-17.json`

### Running the TUI

Simply type:
```bash
agm-tui
```

### Keybindings Guide
- **`↑` / `↓` (or `k` / `j`)**: Scroll through the account list.
- **`Enter`**: Activate the highlighted account (updates session and keyring credentials).
- **`r`**: Refresh quota metrics from Google companion APIs.
- **`w`**: Run a smart Warm Up cycle for the highlighted account.
- **`f`**: Force warm up all models (bypasses cooldowns and percentages).
- **`q` (or `Esc`)**: Exit TUI.
