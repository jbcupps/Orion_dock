#!/usr/bin/env bash
# run-uat.sh — Full automated UAT: build, lint, test, start stack, run probes, tear down.
#
# Usage:
#   ./scripts/run-uat.sh            # Run everything (fast tests + full-stack probes)
#   ./scripts/run-uat.sh --fast     # Fast only: fmt, clippy, build, unit tests, frontend
#   ./scripts/run-uat.sh --keep     # Don't tear down after probes (leave stack for inspection)
#
# Exit code 0 = all tests passed.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

COMPOSE="docker compose -f docker/docker-compose.yml"
MODE="full"
KEEP_STACK=false
for arg in "$@"; do
  case "$arg" in
    --fast) MODE="fast" ;;
    --keep) KEEP_STACK=true ;;
  esac
done

START_TIME=$(date +%s)
PASS=0
FAIL=0
RESULTS=""

run_phase() {
  local name="$1"
  shift
  local phase_start
  phase_start=$(date +%s)
  echo ""
  echo "=== $name ==="
  if "$@"; then
    local elapsed=$(( $(date +%s) - phase_start ))
    RESULTS="${RESULTS}  [PASS] ${name}  (${elapsed}s)\n"
    PASS=$((PASS + 1))
    echo "  $name PASSED (${elapsed}s)"
  else
    local elapsed=$(( $(date +%s) - phase_start ))
    RESULTS="${RESULTS}  [FAIL] ${name}  (${elapsed}s)\n"
    FAIL=$((FAIL + 1))
    echo "  $name FAILED"
    return 1
  fi
}

cleanup() {
  if [[ "$KEEP_STACK" != "true" && "$MODE" == "full" ]]; then
    echo ""
    echo "=== Tearing down stack ==="
    $COMPOSE --profile full down 2>/dev/null || true
  elif [[ "$KEEP_STACK" == "true" ]]; then
    echo ""
    echo "  Stack left running (--keep). Tear down with: ./scripts/dev-stack.sh down"
  fi

  local total_elapsed=$(( $(date +%s) - START_TIME ))
  echo ""
  echo "  ============================================"
  echo "   UAT Results"
  echo "  ============================================"
  echo ""
  echo -e "$RESULTS"
  if [[ $FAIL -eq 0 && $PASS -gt 0 ]]; then
    echo "   All phases passed in ${total_elapsed}s"
  elif [[ $FAIL -gt 0 ]]; then
    echo "   $FAIL phase(s) failed"
  fi
  echo ""
}
trap cleanup EXIT

# ---- Phase 1: Fast CI (always runs) ----
run_phase "Build + Lint + Unit Tests + Frontend" \
  $COMPOSE run --rm -e UAT_MODE=fast orion-build

if [[ "$MODE" == "fast" ]]; then
  echo ""
  echo "  --fast flag set; skipping full-stack probes."
  exit $FAIL
fi

# ---- Phase 2: Build full-stack images ----
run_phase "Build Full-Stack Images" \
  $COMPOSE --profile full build

# ---- Phase 3: Start full stack ----
run_phase "Start Full Stack" \
  $COMPOSE --profile full up -d

# ---- Phase 4: Wait for services ----
wait_for_api() {
  echo "  Waiting for API..."
  for i in $(seq 1 90); do
    if curl -sf --max-time 2 http://localhost:8080/health >/dev/null 2>&1; then
      echo "  API is healthy."
      return 0
    fi
    sleep 1
  done
  echo "  API did not become healthy within 90s"
  return 1
}
run_phase "Service Readiness" wait_for_api

# ---- Phase 5: UAT Probes ----
run_phase "UAT Probes (14 endpoint tests)" \
  $COMPOSE exec orion-dev bash -c \
    "ORION_API_URL=http://orion-api:8080 FRONTEND_URL=http://frontend:80 bash /app/scripts/uat-probes.sh"

# ---- Phase 6: Postgres integration tests ----
run_phase "Postgres Integration Tests" \
  $COMPOSE exec orion-dev bash -c \
    "DATABASE_URL=postgres://orion:orion_dev@postgres:5432/orion cargo test -p orion-memory --no-fail-fast --features postgres"

exit $FAIL
