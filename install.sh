#!/bin/bash
set -e

echo "🚀 Starting Antigravity Manager installer..."

# 1. Check for cargo presence
if ! command -v cargo &> /dev/null; then
    echo "✗ Error: Rust/Cargo is not installed. Please install Rust first (see README.md)."
    exit 1
fi

# 2. Build the release binary
echo "📦 Building optimized release binary..."
cargo build --release

# 2. Determine installation location
INSTALL_DIR=""
if [ -n "$PREFIX" ] && [ -d "$PREFIX/bin" ]; then
    # Termux environment
    INSTALL_DIR="$PREFIX/bin"
    cp target/release/antigravity-tui "$INSTALL_DIR/agm"
    echo "✓ Binary installed to $INSTALL_DIR/agm"
else
    # Standard Linux/macOS
    INSTALL_DIR="$HOME/.local/bin"
    mkdir -p "$INSTALL_DIR"
    cp target/release/antigravity-tui "$INSTALL_DIR/agm"
    echo "✓ Binary installed to $INSTALL_DIR/agm"

    # Append path to shell configs
    SHELL_CONFIGS=("$HOME/.bashrc" "$HOME/.zshrc" "$HOME/.profile" "$HOME/.bash_profile")
    PATH_LINE='export PATH="$HOME/.local/bin:$PATH"'
    
    echo "Updating shell profile configurations..."
    UPDATED=0
    for config in "${SHELL_CONFIGS[@]}"; do
        if [ -f "$config" ]; then
            if ! grep -Fq "$PATH_LINE" "$config" && ! grep -q '\.local/bin' "$config"; then
                echo -e "\n# Antigravity Manager Bin Path\n$PATH_LINE" >> "$config"
                echo "  Added to $config"
                UPDATED=1
            fi
        fi
    done
    if [ $UPDATED -eq 1 ]; then
        echo "✓ Shell configs updated. Please restart your terminal or run: source ~/.bashrc (or your corresponding shell config)"
    else
        echo "✓ Path already configured in shell profiles."
    fi
fi

echo "🎉 Installation successful! You can now run the tool using 'agm'."
