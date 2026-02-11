#!/usr/bin/env bash
# Wait for Postgres and Ollama to be ready, then pull the birth model.
# Use with Docker Compose profile "full". Set DATABASE_URL and LOCAL_LLM_BASE_URL if needed.
# Example: docker compose -f docker/docker-compose.yml --profile full up -d postgres ollama
#          ./scripts/ready-full-stack.sh

set -e

POSTGRES_URL="${DATABASE_URL:-postgres://orion:orion_dev@localhost:5432/orion}"
OLLAMA_URL="${LOCAL_LLM_BASE_URL:-http://localhost:11434}"
BIRTH_MODEL="${BIRTH_MODEL:-qwen2.5:3b-instruct}"
MAX_ATTEMPTS=30
SLEEP=2

# Parse host:port from POSTGRES_URL for pg_isready (e.g. postgres://user:pass@host:5432/db -> host 5432)
pg_host_port() {
  if [[ "$POSTGRES_URL" =~ postgres://[^/]*@([^:/]+):([0-9]+)/ ]]; then
    echo "${BASH_REMATCH[1]} ${BASH_REMATCH[2]}"
  else
    echo "localhost 5432"
  fi
}

echo "Waiting for Postgres at $POSTGRES_URL ..."
read -r PG_HOST PG_PORT <<< "$(pg_host_port)"
for i in $(seq 1 "$MAX_ATTEMPTS"); do
  if (command -v pg_isready >/dev/null 2>&1 && pg_isready -h "$PG_HOST" -p "$PG_PORT" -U orion 2>/dev/null) || \
     (command -v nc >/dev/null 2>&1 && nc -z "$PG_HOST" "$PG_PORT" 2>/dev/null); then
    echo "Postgres is ready."
    break
  fi
  if [[ $i -eq $MAX_ATTEMPTS ]]; then
    echo "Postgres did not become ready in time." >&2
    exit 1
  fi
  sleep "$SLEEP"
done

echo "Waiting for Ollama at $OLLAMA_URL ..."
for i in $(seq 1 "$MAX_ATTEMPTS"); do
  if curl -sf "${OLLAMA_URL}/api/tags" >/dev/null 2>&1; then
    echo "Ollama is ready."
    break
  fi
  if [[ $i -eq $MAX_ATTEMPTS ]]; then
    echo "Ollama did not become ready in time." >&2
    exit 1
  fi
  sleep "$SLEEP"
done

echo "Pulling birth model: $BIRTH_MODEL"
if command -v ollama >/dev/null 2>&1; then
  ollama pull "$BIRTH_MODEL"
  echo "Done. Postgres and Ollama are ready; birth model pulled."
else
  echo "ollama CLI not found; pull manually: ollama pull $BIRTH_MODEL"
fi
