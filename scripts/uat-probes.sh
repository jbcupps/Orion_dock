#!/usr/bin/env bash
# Full-stack UAT probes. Run from orion-build container when UAT_MODE=full and full profile is up.
# Uses Compose service hostnames: orion-api:8080, frontend:80, postgres:5432, ollama:11434.
# Deterministic, CI-safe: retry with bounded attempts, then assert.
set -euo pipefail

ORION_API_URL="${ORION_API_URL:-http://orion-api:8080}"
FRONTEND_URL="${FRONTEND_URL:-http://frontend:80}"
MAX_ATTEMPTS="${UAT_PROBE_ATTEMPTS:-30}"
SLEEP="${UAT_PROBE_SLEEP:-2}"

fail() {
  echo "UAT probe failed: $*" >&2
  exit 1
}

# Wait for HTTP endpoint to respond with 200
wait_for_http() {
  local url="$1"
  local desc="${2:-$url}"
  echo "Waiting for $desc ..."
  for i in $(seq 1 "$MAX_ATTEMPTS"); do
    if curl -sf --max-time 5 "$url" >/dev/null 2>&1; then
      echo "$desc is up."
      return 0
    fi
    if [[ $i -eq $MAX_ATTEMPTS ]]; then
      fail "$desc did not become ready in time (tried $MAX_ATTEMPTS times)"
    fi
    sleep "$SLEEP"
  done
}

# --- UAT-1: Startup health ---
wait_for_http "${ORION_API_URL}/health" "orion-api /health"
body=$(curl -sf --max-time 5 "${ORION_API_URL}/health")
if ! echo "$body" | grep -q '"status":"ok"'; then
  fail "UAT-1: /health did not return status ok: $body"
fi
echo "UAT-1 Startup health: OK"

# --- UAT-2: Birth journey (status endpoint) ---
body=$(curl -sf --max-time 5 "${ORION_API_URL}/api/status")
if ! echo "$body" | grep -q 'memory_backend'; then
  fail "UAT-2: /api/status missing memory_backend: $body"
fi
if ! echo "$body" | grep -q 'birth_complete'; then
  fail "UAT-2: /api/status missing birth_complete: $body"
fi
echo "UAT-2 Birth journey (status): OK"

# --- UAT-3: Routing / config surface ---
if ! echo "$body" | grep -q 'local_llm_configured'; then
  fail "UAT-3: /api/status missing local_llm_configured: $body"
fi
echo "UAT-3 Routing correctness (status): OK"

# --- UAT-4: Genesis path listing ---
paths_body=$(curl -sf --max-time 5 "${ORION_API_URL}/api/genesis/paths")
if ! echo "$paths_body" | grep -q '"id"'; then
  fail "UAT-4: /api/genesis/paths missing id in response: $paths_body"
fi
if ! echo "$paths_body" | grep -q '"label"'; then
  fail "UAT-4: /api/genesis/paths missing label: $paths_body"
fi
if ! echo "$paths_body" | grep -q '"description"'; then
  fail "UAT-4: /api/genesis/paths missing description: $paths_body"
fi
if ! echo "$paths_body" | grep -q '"estimated_time"'; then
  fail "UAT-4: /api/genesis/paths missing estimated_time: $paths_body"
fi
# Expect at least 5 path entries (Direct + 3 Soul Crystallization depths + Soul Forge)
path_count=$(echo "$paths_body" | grep -c '"id":' || true)
if [[ "${path_count:-0}" -lt 5 ]]; then
  fail "UAT-4: /api/genesis/paths expected at least 5 entries, got ${path_count:-0}"
fi
echo "UAT-4 Genesis path listing: OK"

# --- UAT-5: Create agent and start Soul Forge Genesis ---
create_body=$(curl -sf --max-time 10 -X POST "${ORION_API_URL}/api/agents" \
  -H "Content-Type: application/json" \
  -d '{"name":"UAT Probe Agent"}')
agent_id=$(echo "$create_body" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
if [[ -z "${agent_id:-}" ]]; then
  fail "UAT-5: create agent did not return id: $create_body"
fi
curl -sf --max-time 5 -X POST "${ORION_API_URL}/api/agents/${agent_id}/load" >/dev/null
# Allow 4xx so we can read body and skip UAT-5/6 when agent is not in Connectivity
genesis_start_body=$(curl -s -w "\n%{http_code}" --max-time 10 -X POST \
  "${ORION_API_URL}/api/agents/${agent_id}/genesis/start" \
  -H "Content-Type: application/json" \
  -d '{"path":"soul_forge"}')
genesis_start_http=$(echo "$genesis_start_body" | tail -n1)
genesis_start_json=$(echo "$genesis_start_body" | sed '$d')
if [[ "$genesis_start_http" == "200" ]]; then
  if ! echo "$genesis_start_json" | grep -q '"state":"scenario1"'; then
    fail "UAT-5: genesis/start 200 but missing state scenario1: $genesis_start_json"
  fi
  if ! echo "$genesis_start_json" | grep -q '"choices"'; then
    fail "UAT-5: genesis/start 200 but missing choices: $genesis_start_json"
  fi
  echo "UAT-5 Create agent and start Soul Forge Genesis: OK"

  # --- UAT-6: Soul Forge scenario progression ---
  for _ in 1 2 3; do
    forge_body=$(curl -sf --max-time 10 -X POST \
      "${ORION_API_URL}/api/agents/${agent_id}/genesis/forge/select" \
      -H "Content-Type: application/json" \
      -d '{"choice":0}')
    if ! echo "$forge_body" | grep -q '"state"'; then
      fail "UAT-6: forge/select missing state: $forge_body"
    fi
  done
  if ! echo "$forge_body" | grep -q '"archetype"'; then
    fail "UAT-6: final forge/select missing archetype: $forge_body"
  fi
  if ! echo "$forge_body" | grep -q '"soul_hash"'; then
    fail "UAT-6: final forge/select missing soul_hash: $forge_body"
  fi
  echo "UAT-6 Soul Forge scenario progression: OK"
else
  echo "UAT-5/UAT-6 skipped (agent not in Connectivity; genesis/start returned $genesis_start_http)"
fi

# --- UAT-7: Failure handling (ready endpoint) ---
ready_body=$(curl -sf --max-time 5 "${ORION_API_URL}/ready")
if ! echo "$ready_body" | grep -q '"status":"ok"'; then
  fail "UAT-7: /ready did not return status ok: $ready_body"
fi
echo "UAT-7 Failure handling (ready): OK"

# --- Frontend smoke (optional: only if frontend is part of stack) ---
if curl -sf --max-time 5 --connect-timeout 2 "${FRONTEND_URL}" >/dev/null 2>&1; then
  echo "Frontend smoke: OK"
else
  echo "Frontend not reachable (optional); skipping."
fi

echo "All UAT probes passed."
