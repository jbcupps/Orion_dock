# How to Run Orion Locally (Docker-First)

This repository is operated with a Docker-first workflow.
Native Tauri/installer paths are intentionally out of scope.

## Prerequisites

- Docker Engine 20+ (or Docker Desktop)
- Docker Compose v2

## Build and Verify

From repo root:

```bash
# Build the image used for checks
docker compose -f docker/docker-compose.yml build orion-build

# Run formatting, lint, build, and test in container
docker compose -f docker/docker-compose.yml run --rm orion-build
```

## Interactive Development Shell

```bash
# Start long-running dev container
docker compose -f docker/docker-compose.yml up -d orion-dev

# Enter container shell
docker compose -f docker/docker-compose.yml exec orion-dev bash
```

Inside the shell:

```bash
cargo build --workspace
cargo test --workspace --no-fail-fast
```

## Full Stack (Web UI + API + Postgres + Ollama)

To run the full stack with the web UI, HTTP API, Postgres memory backend, and Ollama:

```bash
# Start all services (postgres, ollama, orion-api, frontend)
docker compose -f docker/docker-compose.yml --profile full up -d

# Optional: wait for Postgres/Ollama and pull the birth model (from host)
./scripts/ready-full-stack.sh
# Or on Windows: .\scripts\ready-full-stack.ps1
```

- **Web UI:** http://localhost:3000
- **API:** http://localhost:8080 (health: `/health`, status: `/api/status`)

The frontend container proxies `/api` and `/health` to the API service, so the UI uses the same origin when served via Docker.

## Full Stack (Postgres + Ollama, dev shell only)

To run with a Postgres memory backend and Ollama for development (no web UI):

```bash
# Start Postgres and Ollama (profile "full")
docker compose -f docker/docker-compose.yml --profile full up -d postgres ollama

# Wait for services and pull the birth model (from host)
./scripts/ready-full-stack.sh
# Or on Windows: .\scripts\ready-full-stack.ps1

# Start dev container with Postgres/Ollama env (same profile)
docker compose -f docker/docker-compose.yml --profile full up -d orion-dev
docker compose -f docker/docker-compose.yml exec orion-dev bash
```

Inside the container, set (or rely on Compose defaults):

- `MEMORY_BACKEND=postgres`
- `DATABASE_URL=postgres://orion:orion_dev@postgres:5432/orion`
- `LOCAL_LLM_BASE_URL=http://ollama:11434`
- `BIRTH_MODEL=qwen2.5:3b-instruct`

Migrations for Postgres (pgvector, memories, birth, embeddings, edges) run automatically on first connection from the app.

To only pull the birth model when Ollama is already running: `./scripts/pull-birth-model.sh` (or set `BIRTH_MODEL` and run `ollama pull $BIRTH_MODEL`).

## Quick-Start Birth

For rapid setup without the interactive ceremony, create an agent with `quick_start: true`:

```bash
curl -X POST http://localhost:8080/api/agents \
  -H 'Content-Type: application/json' \
  -d '{"name": "MyAgent", "quick_start": true}'
```

This auto-generates identity, fills standard constitutional documents, signs them, and marks birth complete in one call.

## Birth sequence (local-first, then cloud)

The interactive birth flow (Connectivity and Genesis chat stages) uses a **local-first** model: the Id (local LLM) is used with a pinned birth model (`BIRTH_MODEL`, default `qwen2.5:3b-instruct`). When you add a cloud API key (e.g. during Connectivity via the birth chat or config), the router enables Ego (cloud) and uses **Ego-primary with local fallback**: it tries the cloud provider first and falls back to the local model if the cloud call fails. So birth runs on local only until a key is present; after that, cloud is used when available and local covers failures.

- Set `LOCAL_LLM_BASE_URL` and optionally `BIRTH_MODEL` for birth stages.
- Set `OPENAI_API_KEY` (or use Trinity config) to enable cloud routing; the birth chat adapter and soul-forge path use `build_birth_router(config)`, which picks up keys from config and vault.

## Tier Model Configuration

After birth, Ego model selection follows the tier system:

| Tier | Purpose | Example (OpenAI) |
|------|---------|-------------------|
| Fast | Lightweight, low-latency | `gpt-4o-mini` |
| Standard | Balanced quality/cost | `gpt-4o` |
| Pro | Highest capability | `o1` |

Manage tier models via the API:

```bash
# View current tier-model mappings
curl http://localhost:8080/api/agents/<id>/tier-models

# Refresh provider catalogs from upstream APIs
curl -X POST http://localhost:8080/api/agents/<id>/tier-models/refresh

# Set preferred provider
curl -X PUT http://localhost:8080/api/agents/<id>/active-provider \
  -H 'Content-Type: application/json' \
  -d '{"provider": "anthropic"}'
```

## Pro Mode Sidecar (optional)

For Pro-tier best-of-two provider comparison, run the LangChain sidecar:

```bash
cd services/pro-router
pip install -r requirements.txt
uvicorn main:app --port 8100
```

Then set `PRO_MODE_SIDECAR_URL=http://localhost:8100` in your `.env`. When Pro tier is selected with 2+ providers configured, the sidecar compares responses and returns the best one.

## Local Verification Script

- macOS/Linux: `./scripts/local-verify.sh`
- Windows PowerShell: `.\scripts\local-verify.ps1`

Both scripts run the same Docker-first validation path.

## Runtime Data Notes

Runtime artifacts and secrets are managed by the crates in this workspace.
When testing inside containers, state is ephemeral unless you add explicit volume mounts.
Postgres data is persisted in the `orion-pgdata` volume when using the full stack.
