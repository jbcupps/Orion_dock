#!/usr/bin/env bash
# dev-stack.sh — Build and launch the full Orion stack for manual testing.
#
# Usage:
#   ./scripts/dev-stack.sh          # Build images, start stack
#   ./scripts/dev-stack.sh down     # Tear down the stack
#   ./scripts/dev-stack.sh rebuild  # Force rebuild images before starting
#
# Once running:
#   Web UI:  http://localhost:3000
#   Postgres: 127.0.0.1:5432  (orion / orion_dev)  — loopback only
#   Ollama:   127.0.0.1:11434                      — loopback only

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

COMPOSE="docker compose -f docker/docker-compose.yml"
ACTION="${1:-up}"

# --- Tear down ---
if [[ "$ACTION" == "down" ]]; then
  echo ""
  echo "  Tearing down Orion stack..."
  echo ""
  $COMPOSE --profile full down
  echo ""
  echo "  Stack is down."
  echo ""
  exit 0
fi

# --- Ensure ORION_MASTER_KEY (generate on first run, persist for copy/backup) ---
ORION_KEY_FILE="$REPO_ROOT/.orion-master-key"
if [[ -z "${ORION_MASTER_KEY:-}" ]]; then
  if [[ -f "$ORION_KEY_FILE" ]]; then
    ORION_MASTER_KEY=$(cat "$ORION_KEY_FILE")
    export ORION_MASTER_KEY
  else
    ORION_MASTER_KEY=$(openssl rand -base64 32)
    printf '%s' "$ORION_MASTER_KEY" > "$ORION_KEY_FILE"
    export ORION_MASTER_KEY
    echo ""
    echo "  ================================================================"
    echo "   ORION_MASTER_KEY generated (first run). Copy and store securely."
    echo "  ================================================================"
    echo ""
    echo "   $ORION_MASTER_KEY"
    echo ""
    echo "   Saved to: $ORION_KEY_FILE"
    echo "   You need this key to decrypt secrets. Back it up safely."
    echo ""
  fi
fi

# --- Build ---
echo ""
echo "  Building Orion Docker images..."
echo ""
if [[ "$ACTION" == "rebuild" ]]; then
  $COMPOSE --profile full build --no-cache
else
  $COMPOSE --profile full build
fi

# --- Start ---
echo ""
echo "  Starting full stack (postgres, ollama, orion-api, frontend)..."
echo ""
$COMPOSE --profile full up -d

# --- Wait for API health (via docker inspect, port not published) ---
echo "  Waiting for API to become healthy..."
MAX_WAIT=60
API_READY=false
API_CID=$($COMPOSE ps -q orion-api 2>/dev/null)
for i in $(seq 1 "$MAX_WAIT"); do
  STATUS=$(docker inspect --format='{{.State.Health.Status}}' "$API_CID" 2>/dev/null || true)
  if [[ "$STATUS" == "healthy" ]]; then
    API_READY=true
    break
  fi
  sleep 1
done
if [[ "$API_READY" != "true" ]]; then
  echo ""
  echo "  WARNING: API did not become healthy within ${MAX_WAIT}s. Check 'docker compose logs orion-api'."
  echo ""
fi

# --- Ensure birth model is available in Ollama ---
BIRTH_MODEL="${BIRTH_MODEL:-qwen2.5:3b-instruct}"
echo "  Ensuring Ollama has birth model ($BIRTH_MODEL)..."
if ! $COMPOSE exec -T ollama ollama list 2>/dev/null | grep -qF "$BIRTH_MODEL"; then
  echo "  Pulling $BIRTH_MODEL (first run — may take a few minutes)..."
  $COMPOSE exec -T ollama ollama pull "$BIRTH_MODEL"
  echo "  Model $BIRTH_MODEL ready."
else
  echo "  Model $BIRTH_MODEL already available."
fi

# --- Wait for frontend ---
echo "  Waiting for frontend..."
FE_READY=false
for i in $(seq 1 30); do
  if curl -sf --max-time 2 http://localhost:3000 >/dev/null 2>&1; then
    FE_READY=true
    break
  fi
  sleep 1
done

# --- Summary ---
echo ""
echo "  ============================================"
echo "   Orion stack is running!"
echo "  ============================================"
echo ""
echo "   Web UI:    http://localhost:3000"
echo "   Health:    http://localhost:3000/health (via nginx)"
echo "   Postgres:  127.0.0.1:5432  (orion / orion_dev)"
echo "   Ollama:    127.0.0.1:11434"
echo ""
echo "   Tear down: ./scripts/dev-stack.sh down"
echo "   Logs:      docker compose -f docker/docker-compose.yml logs -f orion-api"
echo ""

# Open browser (best-effort)
if [[ "$FE_READY" == "true" ]]; then
  if command -v xdg-open >/dev/null 2>&1; then
    xdg-open "http://localhost:3000" 2>/dev/null &
  elif command -v open >/dev/null 2>&1; then
    open "http://localhost:3000" 2>/dev/null &
  fi
fi
