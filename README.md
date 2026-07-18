# 🚀 Antigravity Manager Tool (`agm-tui`)

A unified, 100% Rust-based tool for managing accounts, tracking quotas, and warming up models for the Antigravity system. 

It runs in two modes:
1. **Interactive TUI Mode (Default)**: A terminal user interface built with `ratatui` for mobile tablet environments (Termux) and general command lines.
2. **Command Line Mode (CLI)**: Runs specific subcommands directly from the command line for fast scripting or quick queries.

---

## ⚡ Features

- **Keyring & Active Sync**: Switch active accounts instantly. Writes credentials directly to your system keyring (keychain / Linux secret service) so Cursor/IDE integrations pick it up automatically.
- **Visual Quota Tracking**: Displays remaining percentages and reset times. The TUI features hue-shifting visual progress bars (Green 🟢, Yellow 🟡, Red 🔴).
- **Smart Model Warm Up**: Scans for cooled down models (100% quota) and fires warmup requests, respecting a **4-hour cooldown** stored inside `~/.antigravity_tools/warmup_history.json`.
- **Asynchronous Execution**: In TUI mode, network operations run in background worker threads so the interface remains completely responsive.

---

## 📦 Accounts Database Setup

By default, the tool reads accounts from a backup JSON file containing emails and refresh tokens:
- **Linux Default Path**: `/home/fhrrrzy/Downloads/antigravity_accounts_2026-07-17.json`
- **Termux Default Path**: `~/.antigravity_tools/antigravity_accounts_2026-07-17.json`

If the backup file is not found, the tool automatically falls back to loading configured accounts from the Tauri desktop directory (`~/.antigravity_tools/accounts.json` and `accounts/{id}.json`).

---

## 🔧 Installation & Build Instructions

### 1. Prerequisites
Ensure you have Rust, cargo, and git installed:
```bash
# On Linux (Ubuntu/Debian)
sudo apt update && sudo apt install git build-essential -y
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# On Android (Termux)
pkg update && pkg install git rust clang -y
```

### 2. Compile from Source
```bash
# Clone the repository
git clone git@github.com:fhrrrzy/antigravity-manager-cli.git ~/antigravity-manager-cli

# Navigate to the TUI project folder
cd ~/antigravity-manager-cli/tui

# Build the optimized release binary
cargo build --release
```

### 3. Create a Global Shortcut
Create a symlink to run the tool easily as `agm` from any directory:

```bash
# On Desktop Linux
sudo ln -sf ~/antigravity-manager-cli/tui/target/release/antigravity-tui /usr/local/bin/agm

# On Android Termux
ln -sf ~/antigravity-manager-cli/tui/target/release/antigravity-tui /data/data/com.termux/files/usr/bin/agm
```

---

## 💻 CLI Mode Usage

Run `agm <command>` to execute actions directly:

```bash
# 1. List configured accounts
agm list

# 2. Switch the active account (by index number or email address)
agm switch 3
agm switch fahrurrozy4220@gmail.com

# 3. View quotas (cached)
agm quota

# 4. Refresh and view quotas from Google APIs
agm quota --refresh

# 5. Smart warmup cycle
agm warmup

# 6. Force-warm up a specific model
agm warmup --model gemini-3-flash --force
```

---

## 📱 Interactive TUI Mode Usage

Launch the GUI by running the tool with no arguments:
```bash
agm
```

### Keybindings Guide
- **`↑` / `↓` (or `k` / `j`)**: Scroll through the account list.
- **`Enter`**: Activate the highlighted account (switches active state and writes credentials).
- **`r`**: Refresh quota metrics from Google companion APIs.
- **`w`**: Run a smart Warm Up cycle for the highlighted account.
- **`f`**: Force warm up all models (bypasses cooldowns and percentages).
- **`q` (or `Esc`)**: Exit TUI cleanly.
