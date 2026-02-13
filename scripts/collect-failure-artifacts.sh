#!/usr/bin/env bash
# Write triage artifacts after a failed run. Call from CI or locally after a failure.
# Usage: ./scripts/collect-failure-artifacts.sh [output_dir]
# Writes: output_dir/uat-failure.log (last test/build output if available). No env dump (would leak secrets in CI).
set -euo pipefail
OUT_DIR="${1:-./artifacts}"
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
mkdir -p "$OUT_DIR"
cd "$REPO_ROOT"

echo "Collecting failure artifacts into $OUT_DIR"
if [[ -n "${UAT_LOG_CAPTURE:-}" && -f "$UAT_LOG_CAPTURE" ]]; then
  cp "$UAT_LOG_CAPTURE" "$OUT_DIR/uat-failure.log" 2>/dev/null || true
fi
echo "Done. Inspect $OUT_DIR for triage."
