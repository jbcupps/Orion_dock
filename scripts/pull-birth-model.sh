#!/usr/bin/env bash
# Pull the configured birth model in Ollama. Requires ollama CLI and a running Ollama service.
# BIRTH_MODEL defaults to qwen2.5:3b-instruct.

set -e

BIRTH_MODEL="${BIRTH_MODEL:-qwen2.5:3b-instruct}"
echo "Pulling birth model: $BIRTH_MODEL"
ollama pull "$BIRTH_MODEL"
echo "Done."
