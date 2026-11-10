#!/usr/bin/env bash
# Builds Mixtape and installs it for the current user (no root needed).
set -euo pipefail
cd "$(dirname "$0")/.."
for tool in cargo uv yt-dlp ffmpeg; do
  command -v "$tool" >/dev/null || { echo "Missing $tool: see README.md" >&2; exit 1; }
done
cargo build --release
install -Dm755 target/release/mixtape "$HOME/.local/bin/mixtape"
install -Dm644 assets/mixtape.desktop "$HOME/.local/share/applications/mixtape.desktop"
install -Dm644 assets/mixtape.svg "$HOME/.local/share/icons/hicolor/scalable/apps/mixtape.svg"
# Lets the Update button in Mixtape find this folder.
data="${XDG_DATA_HOME:-$HOME/.local/share}/mixtape"
mkdir -p "$data"
pwd > "$data/source-dir"
echo "Installed. Start Mixtape from the app launcher or run: mixtape"
