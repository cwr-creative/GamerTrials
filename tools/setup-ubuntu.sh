#!/usr/bin/env bash
# Everything an Ubuntu machine needs to build and run GamerTrials: the system
# libraries the client links (WebKitGTK for Tauri, ALSA for sound, udev for
# gamepads, OpenSSL for wss://) and a Rust toolchain from rustup.
#
#   bash tools/setup-ubuntu.sh
#
# Asks for your password once, for apt. Safe to run again.
set -euo pipefail

sudo apt-get update
sudo apt-get install -y \
    build-essential pkg-config curl file \
    libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev libayatana-appindicator3-dev libxdo-dev \
    libssl-dev libasound2-dev libudev-dev \
    libx11-dev libxtst6 \
    fonts-noto-color-emoji

if ! command -v cargo >/dev/null 2>&1 && [ ! -x "$HOME/.cargo/bin/cargo" ]; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
fi

echo
echo "Done. Open a new terminal (or run: source \$HOME/.cargo/env), then:"
echo "    cargo run -p app-client"
