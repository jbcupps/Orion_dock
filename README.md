# Orion Dock

[![CI](https://github.com/jbcupps/orion/actions/workflows/ci.yml/badge.svg)](https://github.com/jbcupps/orion/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

Orion Dock is a Docker-first Rust workspace for Orion's core agent logic and skills runtime.
Desktop installer and npm-based deployment paths have been retired in favor of containerized build, test, and delivery.

## What Is Included

- Core crates for identity, memory, routing, capabilities, skills, and birth lifecycle
- **Orion API** (`crates/orion-api`) — HTTP API for the web UI (health, status)
- **Web UI** (`frontend/`) — React + Vite + TypeScript; chat-style status dashboard
- Skill crates under `skills/`
- Docker development and CI workflow under `docker/`
- Full stack Compose profile: postgres, ollama, orion-api, frontend
- GitHub Actions CI: Docker-only fast suite (lint, build, test, frontend typecheck/build in container) and full-stack UAT job

## Quick Start

### Prerequisites

- Docker Desktop (or Docker Engine + Compose v2)

### Run in Docker

```bash
# Build container image
docker compose -f docker/docker-compose.yml build orion-build

# Run lint/build/test (and frontend typecheck/build) in container
docker compose -f docker/docker-compose.yml run --rm -e UAT_MODE=fast orion-build

# Optional interactive shell for development
docker compose -f docker/docker-compose.yml up -d orion-dev
docker compose -f docker/docker-compose.yml exec orion-dev bash

# Full stack (web UI + API + Postgres + Ollama): open http://localhost:3000
docker compose -f docker/docker-compose.yml --profile full up -d
```

## Workspace Commands (inside container)

The canonical path is `scripts/docker-test-suite.sh` (invoked by `orion-build`). Manually:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace
cargo test --workspace --no-fail-fast
# Frontend: cd frontend && npm ci && npm run typecheck && npm run build
```

For full-stack UAT (with postgres, orion-api, frontend up): run the container with `UAT_MODE=full` and `DATABASE_URL` set; see `documents/APPLICATION_TEST_PLAN.md`.

## Environment Variables

- `OPENAI_API_KEY` - enables cloud provider routes when configured
- `LOCAL_LLM_BASE_URL` - local OpenAI-compatible endpoint (e.g. `http://ollama:11434` in Compose)
- `EXTERNAL_PUBKEY_PATH` - explicit public key path override
- `MEMORY_BACKEND` - `sqlite` (default) or `postgres`
- `DATABASE_URL` - Postgres connection string when `MEMORY_BACKEND=postgres`
- `BIRTH_MODEL` - model used for birth stages (default `qwen2.5:3b-instruct`)
- `ID_MODEL_DEFAULT` - default model for non-birth Id flows

Birth runs **local-first** (pinned birth model); once a cloud API key is set, routing uses cloud first with local fallback. See `documents/HOW_TO_RUN_LOCALLY.md` for full stack and birth runtime steps.

## Repository Layout

- `crates/` - core Rust crates (including `orion-api`)
- `frontend/` - React + Vite web UI; env: `VITE_API_URL` for API base URL when not using proxy
- `skills/` - skill plugins
- `docker/` - Dockerfile and Compose stack
- `.github/workflows/ci.yml` - Docker-based CI gate
- `documents/HOW_TO_RUN_LOCALLY.md` - local Docker runbook

## Contributing

Contributions are welcome. See `CONTRIBUTING.md` for workflow and quality expectations.

## License

MIT. See `LICENSE`.
