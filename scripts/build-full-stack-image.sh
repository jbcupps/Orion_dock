#!/usr/bin/env bash
# Build orion-api and frontend images for production (k8s, matches Docker Compose full stack).
# Usage: ./scripts/build-full-stack-image.sh [registry/prefix]
# Default: orion-api:latest, orion-frontend:latest
set -euo pipefail
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PREFIX="${1:-}"
API_TAG="${PREFIX}orion-api:latest"
FRONTEND_TAG="${PREFIX}orion-frontend:latest"
cd "$REPO_ROOT"

echo "Building orion-api: $API_TAG"
docker build -f docker/Dockerfile.api -t "$API_TAG" .

echo "Building frontend: $FRONTEND_TAG"
docker build -f frontend/Dockerfile -t "$FRONTEND_TAG" .

echo "Built $API_TAG and $FRONTEND_TAG"
echo "For k8s: ensure images are available in cluster (load into kind/minikube, or push to registry)"
