#!/usr/bin/env bash
# Agentic UAT: full automated test from birth to usable operation.
# Requires: TEST_LLM_KEY, TEST_SEARCH_KEY (optional: TEST_LLM_PROVIDER, TEST_SEARCH_PROVIDER, UAT_GENESIS_PATH).
# UAT_GENESIS_PATH: direct (default), soul_forge, soul_crystallization.
# Generates: UAT_REPORT_YYYY-MM-DD.md with sectional checkmarks.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

if [[ -z "${TEST_LLM_KEY:-}" ]] || [[ -z "${TEST_SEARCH_KEY:-}" ]]; then
  echo "Missing required env: TEST_LLM_KEY and TEST_SEARCH_KEY. Set them and re-run." >&2
  exit 1
fi

DATE="$(date +%Y-%m-%d)"
REPORT="$REPO_ROOT/UAT_REPORT_$DATE.md"
LOG="$REPO_ROOT/uat_run_$DATE.log"

echo "=== Agentic UAT: starting full stack ==="
docker compose -f docker/docker-compose.yml --profile full build orion-api frontend 2>/dev/null || true
docker compose -f docker/docker-compose.yml --profile full up -d postgres ollama orion-api frontend 2>/dev/null || true

export DATABASE_URL="${DATABASE_URL:-postgres://orion:orion_dev@localhost:5432/orion}"
export LOCAL_LLM_BASE_URL="${LOCAL_LLM_BASE_URL:-http://localhost:11434}"
if [[ -x "$REPO_ROOT/scripts/ready-full-stack.sh" ]]; then
  "$REPO_ROOT/scripts/ready-full-stack.sh" || true
fi

echo "=== Building and running orion-uat ==="
GENESIS_PATH="${UAT_GENESIS_PATH:-direct}"
RUN_CMD="cargo build -p orion-uat --release 2>&1 && /app/target/release/orion-uat 2>&1"
PASS=0
if docker compose -f docker/docker-compose.yml --profile full run --rm \
  -v orion-data:/var/lib/orion \
  -e ORION_DATA_DIR=/var/lib/orion \
  -e DATABASE_URL=postgres://orion:orion_dev@postgres:5432/orion \
  -e LOCAL_LLM_BASE_URL=http://ollama:11434 \
  -e MEMORY_BACKEND=postgres \
  -e BIRTH_MODEL="${BIRTH_MODEL:-qwen2.5:3b-instruct}" \
  -e TEST_LLM_KEY="$TEST_LLM_KEY" \
  -e TEST_SEARCH_KEY="$TEST_SEARCH_KEY" \
  -e TEST_LLM_PROVIDER="${TEST_LLM_PROVIDER:-auto}" \
  -e TEST_SEARCH_PROVIDER="${TEST_SEARCH_PROVIDER:-auto}" \
  -e UAT_GENESIS_PATH="$GENESIS_PATH" \
  orion-build bash -c "$RUN_CMD" >"$LOG" 2>&1; then
  PASS=1
fi

# API status check (optional: only if we have curl and API is up)
API_OK=""
if curl -sf --max-time 5 "${ORION_API_URL:-http://localhost:8080}/api/status" >/dev/null 2>&1; then
  API_OK=1
fi

# Generate report
{
  echo "# UAT Report - $DATE"
  echo ""
  echo "| Stage | Status |"
  echo "|-------|--------|"
  if [[ $PASS -eq 1 ]]; then
    echo "| 1. Darkness (identity generated) | [OK] |"
    echo "| 2. Ignition (local LLM) | [OK] |"
    echo "| 3. Connectivity (API keys stored) | [OK] |"
    echo "| 4. Genesis ($GENESIS_PATH) | [OK] |"
    echo "| 5. Emergence (birth complete) | [OK] |"
    echo "| 6. Early operation (router check) | [OK] |"
  else
    echo "| 1. Darkness | [X] |"
    echo "| 2. Ignition | [X] |"
    echo "| 3. Connectivity | [X] |"
    echo "| 4. Genesis ($GENESIS_PATH) | [X] |"
    echo "| 5. Emergence | [X] |"
    echo "| 6. Early operation | [X] |"
  fi
  echo "| 7. API status visible | $([ -n "$API_OK" ] && echo '[OK]' || echo '[X]') |"
  echo ""
  echo "## Log"
  printf '%s\n' '```'
  tail -n 80 "$LOG" 2>/dev/null || true
  printf '%s\n' '```'
} >"$REPORT"

echo "Report written to $REPORT"
if [[ $PASS -eq 0 ]]; then
  exit 1
fi
exit 0
