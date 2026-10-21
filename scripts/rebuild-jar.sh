#!/usr/bin/env bash
# rebuild-jar.sh
# Rebuild RustMode.jar and drop it into RustMode/mode/.
#
# Usage:
#   ./scripts/rebuild-jar.sh
#   PROCESSING4_CORE=~/processing-4/core/library/core.jar ./scripts/rebuild-jar.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RUSTMODE_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
JAR_DEST="$RUSTMODE_DIR/mode/RustMode.jar"

# ── Sanity checks ─────────────────────────────────────────────────────────────

if ! command -v gradle &>/dev/null; then
  echo "ERROR: gradle not found on PATH."
  exit 1
fi

# ── Locate core.jar ───────────────────────────────────────────────────────────

CORE_FLAG=""
if [ -n "${PROCESSING4_CORE:-}" ]; then
  CORE_FLAG="-Pcore=$PROCESSING4_CORE"
else
  candidates=(
    "$HOME/processing-4/core/library/core.jar"
    "$HOME/processing4/core/library/core.jar"
    "/opt/processing/core/library/core.jar"
    "/opt/processing4/core/library/core.jar"
    "/usr/local/lib/processing/core/library/core.jar"
    "/Applications/Processing.app/Contents/Java/core/library/core.jar"
  )
  for c in "${candidates[@]}"; do
    if [ -f "$c" ]; then
      CORE_FLAG="-Pcore=$c"
      echo "core.jar: $c"
      break
    fi
  done
  if [ -z "$CORE_FLAG" ]; then
    echo "WARNING: core.jar not found. Set PROCESSING4_CORE=<path> or build may fail."
    echo
  fi
fi

# ── Build ─────────────────────────────────────────────────────────────────────

cd "$RUSTMODE_DIR"
# shellcheck disable=SC2086
gradle jar --console=plain $CORE_FLAG "$@"

# ── Verify ────────────────────────────────────────────────────────────────────

if [ ! -f "$JAR_DEST" ]; then
  echo "ERROR: mode/RustMode.jar not found after build."
  exit 1
fi

SIZE=$(du -sh "$JAR_DEST" | cut -f1)
echo
echo "ok   mode/RustMode.jar  ($SIZE)"
echo "Restart Processing to load the updated mode."
