#!/usr/bin/env bash
# Build the deploy image (release binaries, minimal runtime).
# Usage: ./scripts/build-deploy-image.sh [tag]
# Default tag: orion:latest
set -euo pipefail
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TAG="${1:-orion:latest}"
cd "$REPO_ROOT"
echo "Building deploy image: $TAG"
docker build -f docker/Dockerfile.deploy -t "$TAG" .
echo "Built $TAG. Run: docker run -it --rm $TAG"
echo "Push: docker tag $TAG <registry>/orion:<tag> && docker push <registry>/orion:<tag>"
