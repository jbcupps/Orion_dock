#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

echo "Validating docker compose file..."
docker compose -f docker/docker-compose.yml config -q
echo "Validating full-stack compose profile..."
docker compose -f docker/docker-compose.yml --profile full config -q

echo "Building Docker validation image..."
docker compose -f docker/docker-compose.yml build orion-build

echo "Running Docker validation pipeline (fast)..."
docker compose -f docker/docker-compose.yml run --rm -e UAT_MODE=fast orion-build

echo "Docker-first local verification completed."
