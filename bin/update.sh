#!/usr/bin/env bash
# Pulls the latest Mixtape from git and reinstalls it. Restart Mixtape afterwards.
set -euo pipefail
cd "$(dirname "$0")/.."
if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
  echo "You have local changes in $(pwd). Commit or stash them, then run this again." >&2
  exit 1
fi
git pull --ff-only
bin/install.sh
