#!/usr/bin/env bash
# Quick install: curl -fsSL https://raw.githubusercontent.com/fhrrrzy/antigravity-manager-tui/main/install-quick.sh | bash

set -e

echo "🚀 Installing Antigravity Manager (agm)..."


# Detect OS and architecture
OS="$(uname -s)"
ARCH="$(uname -m)"

if [ "$OS" = "Linux" ]; then
    if [ "$ARCH" = "x86_64" ]; then
        BINARY="agm-ubuntu-x86_64"
        FALLBACK="agm-debian-bullseye-x86_64"
    elif [ "$ARCH" = "aarch64" ]; then
        if [ -n "$PREFIX" ] && [[ "$PREFIX" == *"/termux"* ]]; then
            BINARY="agm-termux-aarch64"
        else
            echo "❌ Linux aarch64 (non-Termux) is not currently supported by pre-built binaries."
            exit 1
        fi
    else
        echo "❌ Architecture $ARCH is not supported."
        exit 1
    fi
elif [ "$OS" = "Darwin" ]; then
    if [ "$ARCH" = "x86_64" ]; then
        BINARY="agm-macos-x86_64"
    elif [ "$ARCH" = "arm64" ]; then
        BINARY="agm-macos-aarch64"
    else
        echo "❌ Architecture $ARCH on macOS is not supported."
        exit 1
    fi
else
    echo "❌ OS $OS is not supported."
    exit 1
fi

# Define download URL
LATEST_URL="https://github.com/fhrrrzy/antigravity-manager-tui/releases/latest/download"

# Define destination directory
if [ -n "$PREFIX" ] && [[ "$PREFIX" == *"/termux"* ]]; then
    DEST_DIR="$PREFIX/bin"
else
    DEST_DIR="$HOME/.local/bin"
    mkdir -p "$DEST_DIR"
fi

DEST_FILE="$DEST_DIR/agm"

echo "📥 Downloading $BINARY..."

if curl -fsSL "$LATEST_URL/$BINARY" -o "$DEST_FILE"; then
    chmod +x "$DEST_FILE"
    echo "✅ Successfully installed agm to $DEST_FILE"
elif [ -n "$FALLBACK" ]; then
    echo "⚠️ Failed to download $BINARY. Trying fallback $FALLBACK..."
    if curl -fsSL "$LATEST_URL/$FALLBACK" -o "$DEST_FILE"; then
        chmod +x "$DEST_FILE"
        echo "✅ Successfully installed agm to $DEST_FILE"
    else
        echo "❌ Failed to download fallback $FALLBACK."
        exit 1
    fi
else
    echo "❌ Failed to download $BINARY."
    exit 1
fi

# Check PATH
if [[ ":$PATH:" != *":$DEST_DIR:"* ]]; then
    echo "⚠️  Note: $DEST_DIR is not in your PATH."
    echo "You might need to add it to your shell configuration (e.g., ~/.bashrc or ~/.zshrc):"
    echo "  export PATH=\"\$PATH:$DEST_DIR\""
fi

echo "🎉 Installation complete! Run 'agm' to get started."
