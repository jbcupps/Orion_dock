# Orion Dock

[![CI](https://github.com/jbcupps/Orion_dock/actions/workflows/ci.yml/badge.svg)](https://github.com/jbcupps/Orion_dock/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

Orion Dock is a Docker-first Rust workspace for Orion's core agent logic and skills runtime.
Desktop installer and npm-based deployment paths have been retired in favor of containerized build, test, and delivery.

## What Is Included

- Core crates for identity, memory, routing, capabilities, skills, and birth lifecycle
- **Autonomous agent runtime** — structured tool-calling via `route_with_tools()` with legacy text fallback; risk-based security policies that enable safe autonomous action
- **Orion API** (`crates/orion-api`) — HTTP API for the web UI (health, status, tier-model management)
- **Web UI** (`frontend/`) — React + Vite + TypeScript; chat dashboard with Fast/Standard/Pro tier selection
- **Council Engine (MoA DAG)** (`crates/orion-router/src/council.rs`) — native Rust multi-model debate/synthesis for Pro tier
- **Tier Model Orchestration** — Per-provider Fast/Standard/Pro model selection with catalog validation
- **Quick-Start Birth** — One-call agent creation with auto identity and standard constitutional documents
- Skill crates under `skills/` (web search, web browse, shell, filesystem, HTTP, email, Docker exec, Perplexity)
- Docker development and CI workflow under `docker/`
- Full stack Compose profile: postgres, ollama, orion-api, frontend
- GitHub Actions CI (when enabled): Docker-only fast suite (lint, build, test, frontend typecheck/build in container) and full-stack UAT job. CI and Dependabot may be disabled; run `scripts/local-verify` (or `run-uat.ps1` / `dev-stack.ps1`) locally before pushing.

## Ecosystem Role & Alignment

This repository is one piece of a deliberate three-part identity ecosystem (see [sao-ecosystem-article.md](https://github.com/jbcupps/SAO/blob/main/sao-ecosystem-article.md) and diagrams below).

- **Abigail** – personal local agent with full free will (owner-controlled keys).
- **Orion Dock** – enterprise container agents (same soul + skills model, SAO-provisioned).
- **SAO** – central management, cryptographic vault, agent registry, enterprise IDP bridge.

### Agent Soul Contract

Every running agent instance carries the same archetype:
- `soul.md` + `ethics.md` + `org-map.md`
- Merged at birth into the runtime system prompt.
- Skills always split: **tool** (code/env) + **how-to-use.md** (ego guidance).

### Visual References

- Modular Crate Architecture (Orion)
- Birth Lifecycle
- Bicameral Mind / IdEgo Router
- Zero Trust Security Model
- Autonomous Execution Loop
- SAO Trust Chain & Ecosystem Overview

## Quick Start

### Prerequisites

- Docker Desktop (or Docker Engine + Compose v2)

### Environment setup

Copy `example.env` to `.env` and set variables as needed (API keys, `LOCAL_LLM_BASE_URL`, etc.). The file `.env` is gitignored; never commit secrets.

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

# Full stack with dual-proxy egress boundary (proxy always on in full profile)
docker compose -f docker/docker-compose.yml --profile full up -d --build
```

### Comprehensive startup script (Windows)

For a one-command full environment startup (including optional email ingress profile), use:

```powershell
.\scripts\full-stack.ps1
```

Common variants:

```powershell
.\scripts\full-stack.ps1 -NoEmail      # start without email profile
.\scripts\full-stack.ps1 -SkipBuild    # start without rebuilding images
.\scripts\full-stack.ps1 -Rebuild      # no-cache rebuild before start
.\scripts\full-stack.ps1 -Down         # tear down running stack
```
Dual-proxy mode keeps app services on an internal-only network and routes outbound HTTP(S) via
`proxy_internal -> proxy_external`, with SSRF deny rules at the egress proxy. See
`documents/HOW_TO_RUN_LOCALLY.md` and `docker/proxy/README.md` for modes (`allow_all` vs
`allowlist`), host access toggles, ingress sidecars, and audit logs.

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

## Security (quick reference)

- **Private key:** During birth setup, the Ed25519 private key is shown **once**. Save it securely; it is not stored. See `documents/SECURITY_NOTES.md`.
- **Non-Windows:** Secret storage uses a plaintext stub on non-Windows platforms (development only). Do not use for production on macOS/Linux until a cross-platform secret store is integrated.
- **Local LLM URL:** Only `localhost` / `127.0.0.1` / `::1` are allowed to prevent SSRF. Do not point at internal or cloud URLs.

## Environment Variables

- `OPENAI_API_KEY` - enables cloud provider routes when configured
- `LOCAL_LLM_BASE_URL` - local OpenAI-compatible endpoint (e.g. `http://ollama:11434` in Compose)
- `EXTERNAL_PUBKEY_PATH` - explicit public key path override
- `MEMORY_BACKEND` - `sqlite` (default) or `postgres`
- `DATABASE_URL` - Postgres connection string when `MEMORY_BACKEND=postgres`
- `BIRTH_MODEL` - model used for birth stages (default `qwen2.5:3b-instruct`)
- `ID_MODEL_DEFAULT` - default model for non-birth Id flows

Birth runs **local-first** (pinned birth model); once a cloud API key is set, routing uses cloud first with local fallback. Ego model selection follows the **tier system**: Fast (lightweight), Standard (balanced), or Pro (highest capability with native council debate/synthesis). See `documents/HOW_TO_RUN_LOCALLY.md` for full stack and birth runtime steps.

### Id vs Ego Responsibility Matrix (Non-Negotiable)

| Responsibility                       | Id (local, heartbeat) | Ego (cloud tiers)      |
|--------------------------------------|-----------------------|------------------------|
| Birth process                        | Yes                   | No                     |
| Heartbeat / cron lightweight checks  | Yes                   | No                     |
| Privacy-sensitive / local-only       | Yes                   | No                     |
| Operational chat (mentor)            | No                    | Yes                    |
| Agentic / tool-calling loops         | No                    | Yes (min Standard)     |
| Orchestration spawn_agentic          | No                    | Yes                    |

Id is a heartbeat-class model. It must never be used for tool-calling, autonomous loops, or any task requiring function-calling reliability.

## Repository Layout

- `crates/` - core Rust crates (including `orion-api`)
- `frontend/` - React + Vite web UI; env: `VITE_API_URL` for API base URL when not using proxy
- `services/` - auxiliary runtime services
- `skills/` - skill plugins
- `docker/` - Dockerfile and Compose stack
- `.github/workflows/ci.yml` - Docker-based CI gate
- `documents/HOW_TO_RUN_LOCALLY.md` - local Docker runbook

## Contributing

Contributions are welcome. See `CONTRIBUTING.md` for workflow and quality expectations.

## License

MIT. See `LICENSE`.
