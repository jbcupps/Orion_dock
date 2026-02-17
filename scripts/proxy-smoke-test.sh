#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

COMPOSE="docker compose -f docker/docker-compose.yml --profile full"

run_nettest() {
  local cmd="$1"
  $COMPOSE exec -T nettest sh -lc "$cmd"
}

pass() {
  echo "[PASS] $1"
}

fail() {
  echo "[FAIL] $1" >&2
  exit 1
}

echo "Bringing up proxy test dependencies (proxy_external, proxy_internal, ollama, nettest)..."
$COMPOSE up -d proxy_external proxy_internal ollama nettest

echo "Waiting for Ollama internal endpoint..."
ollama_ready=false
for i in $(seq 1 45); do
  if run_nettest "curl -fsS --connect-timeout 2 --max-time 5 http://ollama:11434/api/tags -o /dev/null" >/dev/null 2>&1; then
    ollama_ready=true
    break
  fi
  sleep 2
done
[ "$ollama_ready" = "true" ] || fail "Ollama did not become reachable from nettest."

# 1) Direct egress should fail when bypassing proxy env.
if run_nettest "unset HTTP_PROXY HTTPS_PROXY http_proxy https_proxy ALL_PROXY all_proxy; curl -fsS --connect-timeout 8 --max-time 12 --noproxy '*' https://example.com -o /dev/null" >/dev/null 2>&1; then
  fail "Direct egress unexpectedly succeeded."
else
  pass "Direct egress is blocked."
fi

# 2) Proxied egress should work with container proxy env.
if run_nettest "curl -fsS --connect-timeout 8 --max-time 20 https://example.com -o /dev/null" >/dev/null 2>&1; then
  pass "Proxied egress works."
else
  fail "Proxied egress failed."
fi

# 3) SSRF/metadata target must be blocked by external proxy policy.
if run_nettest "curl -fsS --connect-timeout 8 --max-time 12 http://169.254.169.254 -o /dev/null" >/dev/null 2>&1; then
  fail "Metadata endpoint unexpectedly reachable."
else
  pass "Metadata endpoint is blocked."
fi

# 4) Internal service call should bypass proxy via NO_PROXY and succeed.
if run_nettest "curl -fsS --connect-timeout 8 --max-time 12 http://ollama:11434/api/tags -o /dev/null" >/dev/null 2>&1; then
  pass "Internal NO_PROXY bypass works (ollama reachable)."
else
  fail "Internal NO_PROXY bypass failed for ollama."
fi

echo "All proxy smoke tests passed."
