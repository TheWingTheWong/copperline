#!/usr/bin/env bash
# Screenshot a local HTML/SVG file with headless Chrome: scripts/shot.sh in.html out.png W H
set -euo pipefail
CHROME="${CHROME:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"
D="$(mktemp -d)"
trap 'rm -rf "$D"' EXIT
"$CHROME" --headless=new --user-data-dir="$D" --use-mock-keychain --password-store=basic --disable-gpu \
  --hide-scrollbars --window-size="$3,$4" --screenshot="$2" "file://$(cd "$(dirname "$1")" && pwd)/$(basename "$1")" >/dev/null 2>&1
