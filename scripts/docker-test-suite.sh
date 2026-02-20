#!/usr/bin/env bash
# Canonical test runner for orion-build container. Supports fast (default) and full modes.
# Invoked by docker-compose orion-build via UAT_MODE env.
#   UAT_MODE=fast  - fmt, clippy, build, workspace tests (no postgres, no UAT probes)
#   UAT_MODE=full  - same as fast, then postgres-backed tests and full-stack UAT probes
set -euo pipefail

REPO_ROOT="${REPO_ROOT:-/app}"
cd "$REPO_ROOT"

MODE="${UAT_MODE:-fast}"
echo "=== Docker test suite: mode=$MODE ==="

# Ensure Rust tooling
rustup component add rustfmt clippy

# Fast path: format, lint, build, test (workspace default features)
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace
cargo test --workspace --no-fail-fast

# Frontend typecheck and build (Docker-only, no host Node required)
if [[ -f "$REPO_ROOT/frontend/package.json" ]]; then
  echo "=== Frontend typecheck and build ==="
  (cd "$REPO_ROOT/frontend" && npm ci 2>/dev/null || npm install --legacy-peer-deps)
  (cd "$REPO_ROOT/frontend" && npm run typecheck)
  (cd "$REPO_ROOT/frontend" && npm run build)
fi

if [[ "$MODE" != "full" ]]; then
  echo "Build and test (fast) completed."
  exit 0
fi

# Full path: postgres-backed integration tests and UAT probes
echo "=== Full mode: Postgres-backed tests ==="
if [[ -n "${DATABASE_URL:-}" ]]; then
  cargo test -p orion-memory --no-fail-fast --features postgres
fi

echo "=== Full mode: UAT probes ==="
if [[ -x "$REPO_ROOT/scripts/uat-probes.sh" ]]; then
  "$REPO_ROOT/scripts/uat-probes.sh"
elif [[ -f "$REPO_ROOT/scripts/uat-probes.sh" ]]; then
  bash "$REPO_ROOT/scripts/uat-probes.sh"
fi

# When keys are present, run orion-uat once per Genesis path (each with a fresh data dir)
if [[ -n "${TEST_LLM_KEY:-}" ]] && [[ -n "${TEST_SEARCH_KEY:-}" ]]; then
  echo "=== Full mode: orion-uat per Genesis path ==="
  cargo build -p orion-uat --release
  for gp in direct soul_forge soul_crystallization; do
    export ORION_DATA_DIR="/tmp/uat_${gp}_$$"
    mkdir -p "$ORION_DATA_DIR"
    echo "Running orion-uat with UAT_GENESIS_PATH=$gp"
    UAT_GENESIS_PATH="$gp" "$REPO_ROOT/target/release/orion-uat"
  done
fi

echo "Build and test (full) completed."
