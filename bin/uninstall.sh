#!/usr/bin/env bash
# Removes the installed app. Your music, settings (~/.config/mixtape) and download log (~/.local/share/mixtape) stay.
set -euo pipefail
rm -f "$HOME/.local/bin/mixtape" "$HOME/.local/share/applications/mixtape.desktop"
echo "Mixtape removed."
