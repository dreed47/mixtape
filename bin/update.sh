#!/usr/bin/env bash
# Updates Mixtape to the newest tagged release and reinstalls it. Restart Mixtape afterwards.
set -euo pipefail
cd "$(dirname "$0")/.."
if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
  echo "You have local changes in $(pwd). Commit or stash them, then run this again." >&2
  exit 1
fi
git fetch --quiet --tags origin
latest=$(git tag --list 'v*' --sort=-v:refname | head -n1)
if [ -z "$latest" ]; then
  echo "No releases found." >&2
  exit 1
fi
if [ "$(git rev-parse HEAD)" != "$(git rev-parse "$latest^{commit}")" ]; then
  git -c advice.detachedHead=false checkout --quiet "$latest"
fi
echo "Mixtape $latest"
bin/install.sh
