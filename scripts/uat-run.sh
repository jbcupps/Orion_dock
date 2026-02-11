#!/usr/bin/env bash
# Canonical UAT entrypoint: run quality gates and optional full-stack checks.
# Usage:
#   ./scripts/uat-run.sh           # Fast: Docker build+test + docs check
#   ./scripts/uat-run.sh --full     # Start full stack, then run tests + UAT probes (requires Docker)
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

RUN_FULL="${1:-}"

echo "=== UAT: Docker build and test ==="
docker compose -f docker/docker-compose.yml config -q
docker compose -f docker/docker-compose.yml build orion-build

if [[ "$RUN_FULL" == "--full" ]]; then
  echo "=== UAT (full): starting full stack ==="
  docker compose -f docker/docker-compose.yml --profile full build orion-api frontend 2>/dev/null || true
  docker compose -f docker/docker-compose.yml --profile full up -d postgres ollama orion-api frontend 2>/dev/null || true
  export DATABASE_URL="${DATABASE_URL:-postgres://orion:orion_dev@localhost:5432/orion}"
  export LOCAL_LLM_BASE_URL="${LOCAL_LLM_BASE_URL:-http://localhost:11434}"
  if [[ -x "$REPO_ROOT/scripts/ready-full-stack.sh" ]]; then
    "$REPO_ROOT/scripts/ready-full-stack.sh" || true
  fi
  echo "=== UAT (full): running build+test+probes in container ==="
  # Container must use service hostname "postgres", not localhost
  if ! docker compose -f docker/docker-compose.yml --profile full run --rm \
    -e UAT_MODE=full \
    -e DATABASE_URL=postgres://orion:orion_dev@postgres:5432/orion \
    orion-build; then
    echo "UAT failed. Run ./scripts/collect-failure-artifacts.sh for triage artifacts."
    exit 1
  fi
else
  if ! docker compose -f docker/docker-compose.yml run --rm -e UAT_MODE=fast orion-build; then
    echo "UAT failed. Run ./scripts/collect-failure-artifacts.sh for triage artifacts."
    exit 1
  fi
fi

echo "=== UAT: docs/index.html check ==="
if [[ -x "$REPO_ROOT/scripts/check-docs-html.sh" ]]; then
  "$REPO_ROOT/scripts/check-docs-html.sh" "$REPO_ROOT"
else
  bash "$REPO_ROOT/scripts/check-docs-html.sh" "$REPO_ROOT"
fi

echo "UAT completed."
