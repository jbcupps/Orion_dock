#!/bin/sh
# Ensure BIRTH_MODEL is pulled before serving. Part of every deploy.
set -e
BIRTH_MODEL="${BIRTH_MODEL:-qwen2.5:3b-instruct}"

# Start Ollama in background so we can pull the model in the same container
ollama serve &
OLLAMA_PID=$!

# Wait for server to be ready (same volume, so pull will persist)
wait_for_ready() {
  n=0
  max=60
  while [ "$n" -lt "$max" ]; do
    if ollama list >/dev/null 2>&1; then
      return 0
    fi
    n=$((n + 1))
    sleep 1
  done
  return 1
}

if wait_for_ready; then
  echo "Ollama ready; pulling birth model: $BIRTH_MODEL"
  ollama pull "$BIRTH_MODEL" || echo "WARN: pull failed; birth chat may be unavailable until model is present."
else
  echo "WARN: Ollama did not become ready in time; skipping model pull."
fi

# Keep container running (wait for background serve process)
wait $OLLAMA_PID
