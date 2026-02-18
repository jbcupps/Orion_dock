# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Terminology

- **Entity**: The identity running inside the container at runtime (both operational chat persona and spawned autonomous runs). When discussing behavior, decisions, or actions taken by the runtime identity, use "entity".
- **Agent**: The software system and implementation (APIs, crates, orchestration runtime, skills framework).
- **Mentor**: The human operator guiding the entity.
- **Base identity**: `soul.md` is the entity's foundational identity document. `ethics.md` and `instincts.md` constrain and shape behavior, while `growth.md` captures development over time.

## Interactive Session Commands

### "Please start a dev session"

Spin up the full stack for manual testing. The dev-stack script handles everything automatically: build, start, model pull, health checks.

1. Run: `powershell.exe -File "E:\agents\orion_dock\scripts\dev-stack.ps1"`
   - This builds images, starts containers, pulls the birth model into Ollama if missing, and waits for health.
2. Report to user:
   - Web UI: http://localhost:3000
   - API: http://localhost:8080
   - Logs: `docker compose -f docker/docker-compose.yml logs -f orion-api`

### "Please reset"

Tear the entire stack down to tabula rasa (clean slate). Steps:

1. Stop and remove all containers + volumes: `powershell.exe -File "E:\agents\orion_dock\scripts\dev-stack.ps1" -Down`
2. Remove data volumes (agent data, postgres, ollama models, cargo cache):
   ```
   docker volume rm docker_orion-data docker_orion-pgdata docker_ollama-models docker_orion-cargo-cache 2>$null
   ```
3. Report: "Stack torn down. All agent data, database, and model cache removed."

---

## Building & Testing

**All Rust builds and tests MUST run inside Docker.** The host Windows machine does not have the MSVC CRT libraries needed for native compilation. Use the `orion-dev` container.

```bash
# Quick compilation check
docker compose -f docker/docker-compose.yml run --rm orion-dev bash -c "cargo check --workspace"

# Run all workspace tests
docker compose -f docker/docker-compose.yml run --rm orion-dev bash -c "cargo test --workspace --no-fail-fast"

# Run tests for a single crate
docker compose -f docker/docker-compose.yml run --rm orion-dev bash -c "cargo test -p orion-core"
docker compose -f docker/docker-compose.yml run --rm orion-dev bash -c "cargo test -p orion-birth"

# Run a specific test by name
docker compose -f docker/docker-compose.yml run --rm orion-dev bash -c "cargo test -p orion-core test_name_here"

# Lint: fmt + clippy
docker compose -f docker/docker-compose.yml run --rm orion-dev bash -c "cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings"

# Postgres integration tests (requires DATABASE_URL)
docker compose -f docker/docker-compose.yml run --rm orion-dev bash -c "cargo test -p orion-memory --no-fail-fast --features postgres"

# Full CI suite (fmt, clippy, build, test, frontend)
docker compose -f docker/docker-compose.yml run --rm -e UAT_MODE=fast orion-build

# Interactive dev shell (stays open for multiple commands)
docker compose -f docker/docker-compose.yml up -d orion-dev
docker compose -f docker/docker-compose.yml exec orion-dev bash

# Full stack (postgres, ollama, API, frontend)
docker compose -f docker/docker-compose.yml --profile full up -d
# Web UI: http://localhost:3000  API: http://localhost:8080
```

**Frontend** (runs on host — Node/npm are available natively):
```bash
cd frontend && npm install          # install deps
cd frontend && npx tsc --noEmit     # typecheck
cd frontend && npm run build        # production build
cd frontend && npm run dev          # dev server on :3000 (proxies /api to :8080)
```

### Quick Commands (PowerShell)

**"Build it so I can test"** — Builds images, starts the full stack, opens browser:
```powershell
.\scripts\dev-stack.ps1             # Build + start → opens http://localhost:3000
.\scripts\dev-stack.ps1 -Down       # Tear it all down
.\scripts\dev-stack.ps1 -Rebuild    # Force rebuild (no cache)
```

**"Please perform UAT"** — Automated end-to-end: lint, build, test, full-stack probes, postgres tests, then tear down:
```powershell
.\scripts\run-uat.ps1               # Full UAT (build + 14 probes + postgres tests)
.\scripts\run-uat.ps1 -Fast         # Fast only (fmt, clippy, unit tests, frontend)
.\scripts\run-uat.ps1 -KeepStack    # Full UAT but leave stack running after
```

Bash equivalents: `./scripts/dev-stack.sh` and `./scripts/run-uat.sh`.

---

## Project Overview

Orion Dock is a **Docker-first** Rust workspace for Orion's core agent logic, birth lifecycle, and skills runtime. It provides:

- **Orion API** — Axum HTTP API for the web UI (health, status, identities, create/load agents)
- **Web UI** — React + Vite frontend (intro, ORION HIVE identity selector, birth/status dashboard)
- **Birth lifecycle** — Five-stage interactive ceremony (Darkness → Ignition → Connectivity → Genesis → Emergence) that generates cryptographic identity, configures LLM/cloud, and discovers entity identity through conversation
- **Orchestration layer (MVP)** — Scheduled UTC cron jobs, significance scoring, escalation to agentic runs, and mentor-facing attention flags
- **soul-forge** — Alternative TUI path for scenario-based soul calibration

The **birth process is the key differentiator**: cryptographic identity, conversational discovery, and signed constitutional documents. `soul.md` is treated as the entity's base identity.

### Agent Autonomy (Non-Negotiable)

**Self-directed tool use is the primary purpose of this agent.** Every design decision must preserve the agent's ability to act independently using its registered skills. Security hardening, sandbox policies, and trust tiers exist to make autonomy *safe*, not to prevent it.

Key invariants:
- The agent must always attempt available tools before claiming inability.
- Routing must pass structured tool definitions to providers (`route_with_tools`) so function-calling works reliably. Text-based `tool_request` blocks are a backward-compatible fallback, not the primary path.
- Attachment turns allow read-only/low-risk tools; only high-risk mutation tools are blocked.
- Missing API keys are a *solvable state*. The agent tells the mentor exactly what is needed — it never treats missing keys as permanent inability.
- Any security change that degrades tool execution must include an autonomy regression test proving the agent can still perform its core tasks (search, browse, file access, shell execution, credential storage).

---

## Architecture

### Runtime Cognition Model

- **Id** — Local LLM (Ollama, LM Studio). Used for birth, lightweight periodic checks, simple/low-latency work, and privacy-sensitive flows.
- **Orchestration** — In `orion-api` (MVP module): evaluates job significance, applies escalation policy, and controls scheduled/background execution.
- **Ego** — Cloud LLM (OpenAI, Anthropic, etc.). Primary path for mentor-facing operational chat and deep reasoning, with Id fallback. Model selection is tier-aware (Fast/Standard/Pro).
- **Superego** — Safety pre-check support exists in router; dedicated ethical oversight model remains planned.

### Tier Model System (Fast / Standard / Pro)

Ego model selection is controlled by a three-tier system mapped from router modes:

| Router Mode | Tier | Purpose |
|-------------|------|---------|
| `auto` | Fast | Lightweight, low-latency responses |
| `think_hard` | Standard | Balanced quality/cost |
| `think_harder` | Pro | Highest capability; multi-provider council with draft → critique → synthesis |

Default tier models are configured per provider (see `curated_provider_models()` in `orion-core`). Mentors can override per-provider mappings via the API. The `active_provider_preference` config field controls which Ego provider is used.

**Pro mode council:** When Pro tier is active with 2+ connected providers, the system runs a Rust-native Mixture-of-Agents (MoA) council DAG (`crates/orion-router/src/council.rs`). The council:
1. **Draft phase**: Each provider generates an independent draft in parallel.
2. **Critique phase**: Each provider critiques a different provider's draft (cross-provider review), returning a 0-10 JSON score.
3. **Synthesis phase**: The preferred provider synthesizes the ranked drafts into a single best answer.

The council has a 90-second overall timeout and degrades gracefully — if a provider fails during drafting or critique, the remaining providers continue. Falls back to standard Ego routing if the council fails entirely or fewer than 2 providers are available.

### Cognitive Discipline (Karpathy Principles + Autonomy)

The agent's cognitive model incorporates five operational principles. Four are derived from Andrej Karpathy's observations on LLM behavior; the fifth ensures the agent acts on its capabilities rather than deferring. These are woven into the constitutional documents (instincts, ethics, soul) and the runtime system prompt:

| Principle | Where It Lives | What It Does |
|-----------|---------------|--------------|
| **Deliberate before acting** | Instincts (Deliberation Instinct), Operational Prompt | Forces the agent to surface assumptions, present ambiguities, and ask when confused rather than guessing |
| **Simplicity first** | Instincts (Precision Instinct), Ethics (Virtue), Operational Prompt | Prevents overengineering — minimum action that solves the problem, no speculative features |
| **Surgical precision** | Instincts (Precision Instinct), Operational Prompt, Agentic Prompt | Constrains changes to only what was requested — no drive-by refactoring or adjacent "improvements" |
| **Goal-driven execution** | Instincts (Precision Instinct), Operational Prompt, Agentic Prompt | Transforms vague tasks into verifiable success criteria; iterates with verification at each step |
| **Autonomy first** | Instincts (Autonomy Instinct), Operational Prompt, Agentic Prompt | Agent attempts tools before claiming inability; exhausts fallbacks; treats missing keys as solvable |

These principles appear at multiple layers:
- **Instincts** (`instincts.md`): Pre-cognitive Deliberation, Precision, and Autonomy instincts
- **Ethics** (`ethics.md`): "Simplicity over cleverness" and "precision over thoroughness" as virtues
- **Soul** (`soul.md` / template): Nature statement: "I think before I act, I simplify before I build, I verify before I move on"
- **System prompt** (`system_prompt.rs`): Cognitive Discipline section in operational awareness; Autonomy First section; goal-driven workflow in agentic mode
- **Growth** (`GROWTH_MD`): Aspirational development of these disciplines over time

### Crate Map

| Crate | Role |
|-------|------|
| `orion-core` | Config, keyring, DPAPI, templates, vault, verifier, secrets, document signing, email auth, provider presets |
| `orion-memory` | SQLite (default) and Postgres (feature: `postgres`) store, migrations |
| `genesis-core` | Shared genesis contract: GenesisStrategy trait, SoulManifest, CompassEthicWeights, StepRequest/StepResponse, GenesisRegistry |
| `orion-birth` | Birth stage machine, chat runtime, prompts, Genesis path enum and dispatch |
| `orion-soul-crystallization` | Depth-based psychometric engine (CrystallizationEngine, extraction, 4-dim Compass Ethic calibration) |
| `orion-capabilities` | Cognitive (LLM) providers (OpenAI, Anthropic, local), sensory modules, provider model catalog |
| `orion-router` | IdEgoRouter, tier-aware model override, ego model override, Pro council DAG |
| `orion-email` | Email OAuth2 adapters (Gmail, Outlook) with PKCE |
| `orion-api` | Axum HTTP API server |
| `orion-uat` | Headless UAT driver |
| `orion-skills` | Skill framework, MCP protocol, WASM runtime, sandbox, transport layer (IMAP, SMTP, service discovery) |
| `soul-forge` | Scenario-based calibration (14 TOML scenarios, 16 combinatorial archetypes, Compass Ethic weights, sigil generation) |
| `genesis-paths/*` | GenesisStrategy implementations wrapping each genesis engine (quick-start, direct-discovery, soul-crystallization, soul-forge) |
| `skills/*` | Skill plugins (filesystem, http, shell, web-search, web-browse, perplexity-search, email, proton-mail) |

Orchestration is currently implemented as an MVP module at `crates/orion-api/src/orchestration.rs` (not yet split into a dedicated crate).

### Capability / Skill Taxonomy

The project distinguishes **capabilities** from **skills**:

- **Capabilities** are protocol-level, reusable implementations with no agent-facing tools. They live in `orion-capabilities/` (cognitive, sensory), `orion-skills/src/transport/` (IMAP, SMTP, service discovery), and `orion-email/` (OAuth2 adapters).
- **Skills** are agent-facing tool wrappers that compose capabilities into tools with parameters, confirmations, and events. They live in `skills/`. Each skill's `skill.toml` declares which capabilities it uses via `[[capabilities]]`.

Capability categories (tracked in `CapabilityDescriptor.category`):

| Category | Where | Examples |
|----------|-------|----------|
| `protocol` | `orion-skills/transport/`, `orion-email/` | IMAP, SMTP, STARTTLS, OAuth2, MCP, service discovery |
| `cognitive` | `orion-capabilities/cognitive/` | OpenAI, Anthropic, local LLM providers |
| `sensory` | `orion-capabilities/sensory/` | Web search, browser automation, vision, file ingestion |

**Connection process pattern:** Skills follow a discover → configure → connect → operate lifecycle. The `discover_email_service` tool pattern (DNS probing, autoconfig, presets) can be reused for other skills needing service configuration.

**Skill troubleshooting:** When a skill fails to build, configure, or execute, the entity follows a deterministic troubleshooting protocol embedded in its system prompt (see `crates/orion-core/src/system_prompt.rs`, "Skill Troubleshooting Protocol"). The full playbook with binary-split diagnosis, failure records, and Orion-specific gotchas (trust tier timeouts, network permission mismatches, MCP persistence) lives at `documents/SKILL_TROUBLESHOOTING_PLAYBOOK.md`.

### Runtime Surfaces

- **Web**: Frontend (port 3000) → orion-api (port 8080) → Postgres, Ollama. Frontend proxies `/api`, `/health` to API via Vite dev proxy or nginx.
- **Pro Council**: Rust-native MoA council DAG in `crates/orion-router/src/council.rs`. Multi-provider draft → critique → synthesis with 90s timeout and graceful degradation. All three phases receive the full agent system prompt (identity, ethics, instincts, operational awareness) and conversation history. No external sidecar needed.
- **Orchestration**: In-process scheduler loop in API (UTC cron semantics), scanning per-agent jobs and triggering Id checks or agentic runs.
- **Dynamic Skill Registration**: Agentic runs can connect MCP servers at runtime via `register_mcp_skill` synthetic tool. Registered skills persist in `config.mcp_servers` and reload on agent startup.
- **TUI**: `soul-forge` binary — Boot → Intro → 3 ethical scenarios → Crystallize.
- **Dev**: `orion-dev` container with bind-mounted source.

### API Routes (orion-api)

- `/health` — Health check
- `/ready` — Readiness check
- `/api/status` — Agent status (birth_complete, birth_stage)
- `/api/identities` — List agents (Hive)
- `/api/agents/:id/*` — Agent-specific operations (create, load, identity/constitution/verify/export)
- `/api/agents/:id/chat` and `/api/agents/:id/chat/history` — Operational mentor chat
- `/api/agents/:id/skills*` — Skill listing, missing secrets, direct execution
- `/api/agents/:id/agent/*` — Agentic runs (start, stream, status, respond, confirm, cancel, list)
- `/api/agents/:id/orchestration/*` — Scheduled jobs CRUD, run-now, and orchestration logs
- `/api/genesis/paths` + `/api/agents/:id/genesis/*` — Genesis path selection, unified step protocol, and session state
- `/api/agents/:id/tier-models` (GET/PUT) — Read/update per-provider tier model mappings
- `/api/agents/:id/tier-models/refresh` (POST) — Refresh provider catalogs from upstream APIs
- `/api/agents/:id/tier-models/validate` (POST) — Validate selected tier models against catalogs
- `/api/agents/:id/tier-models/reset` (POST) — Reset provider tier models to built-in defaults
- `/api/agents/:id/active-provider` (PUT) — Set preferred Ego provider for routing
- `POST /api/agents` — Create agent (always enters birth flow; quick-start is now a Genesis path)

---

## The Birth Lifecycle

The birth flow is a **state machine** in `crates/orion-birth/`. Most stages are **interactive** — the user must participate.

```
Darkness → Ignition → Connectivity → Genesis (path selector) → Emergence
```

| Stage | Purpose | What Happens |
|-------|---------|--------------|
| **Darkness** | Generate cryptographic identity | `generate_external_keypair()` creates Ed25519 keypair. Public key saved to `external_pubkey.bin`. Private key returned as base64 **once** via `get_private_key_base64()`. User must save it before advancing. Hive master key signs the agent's pubkey (`hive_lineage.sig`) for lineage proof. |
| **Ignition** | Configure local LLM | User sets `local_llm_base_url`. URL validated (localhost/127.0.0.1 only for SSRF protection). |
| **Connectivity** | Acquire cloud API keys | Id (local LLM) chats with user. User pastes API keys. Provider auto-detected from key prefix (sk-ant- → anthropic, sk- → openai). Keys stored in SecretsVault. |
| **Genesis** | Discover identity | Mentor chooses a Genesis path (Quick Start, Direct Discovery, Soul Crystallization, Soul Forge). Each produces (name, purpose, personality) and calls `crystallize_soul()`. |
| **Emergence** | Finalize birth | Signs soul.md, ethics.md, instincts.md with held signing key; writes `.sig` files; drops key from memory; writes birth memory. |

### Hive Master Key Lineage

Each Hive installation generates an Ed25519 master key on first agent creation (`master.key` in data root, DPAPI-encrypted). During the Darkness stage, the agent's public key is signed by the Hive master key, creating a `hive_lineage.sig` file containing the master pubkey, agent pubkey, signature, and timestamp. This proves an agent belongs to a specific Hive. The `/api/agents/:id/identity` endpoint returns `lineage_verified: true/false`.

Key functions: `generate_master_key()`, `sign_agent_lineage()`, `verify_agent_lineage()` in `crates/orion-core/src/keyring.rs`.

### Key Code Locations

- **Stages/orchestrator**: `crates/orion-birth/src/stages.rs` — `BirthStage`, `BirthOrchestrator`, `generate_identity()`, `get_private_key_base64()`, `advance_past_darkness()`, `crystallize_soul()`, `complete_emergence()`
- **Birth chat/tools**: `crates/orion-birth/src/chat.rs` — `build_birth_messages()`, `birth_chat_turn()`, `parse_tool_requests()`, `execute_store_provider_key()`
- **Stage prompts**: `crates/orion-birth/src/prompts.rs` — `CONNECTIVITY_SYSTEM_PROMPT`, `GENESIS_SYSTEM_PROMPT`, `BIRTH_TOOLS_DEFINITION`
- **Keypair generation**: `crates/orion-core/src/keyring.rs` — `generate_external_keypair()`, `sign_constitutional_documents()`, `sign_agent_lineage()`, `verify_agent_lineage()`
- **Hive config**: `crates/orion-core/src/global_config.rs` — `GlobalConfig`, `master_key_path`, agent registry
- **Tier config**: `crates/orion-core/src/config.rs` — `TierModels`, `ProviderCatalogEntry`, `effective_tier_model()`, `curated_provider_models()`
- **Model catalog**: `crates/orion-capabilities/src/cognitive/model_catalog.rs` — Provider catalog fetching (OpenAI, Anthropic, Google, xAI, Perplexity)
- **Default templates**: `crates/orion-core/src/templates.rs` — `DEFAULT_PURPOSE`, `DEFAULT_PERSONALITY`, `fill_soul_template_default()`
- **Pro council**: `crates/orion-router/src/council.rs` — `GraphExecutor`, `run_council()`, MoA draft→critique→synthesis DAG

### Modular Genesis Paths

Every Genesis path implements the `GenesisStrategy` trait from `genesis-core` and produces a `SoulManifest`. The unified step protocol uses `StepRequest` (NeedUserMessage, NeedChoice, NeedForm, Complete) and `StepResponse` (Message, Choice, Form) for all path types. The API manages sessions via `GenesisRegistry`.

| Path | Mechanism |
|------|-----------|
| **Quick Start** | Auto-generate standard identity and constitutional documents from agent name. No ceremony — signs and completes immediately. (~5 sec) |
| **Direct Discovery** | LLM chat with `GENESIS_SYSTEM_PROMPT`; `recommend_crystallize` tool. (~1 min) |
| **Soul Crystallization** | `CrystallizationEngine` (Awakening → Lattice → Reflection → Crucible → Crystallization → Complete); depth-based (Quick Start/Conversation/Deep Dive), 4-dim Compass Ethic calibration. (30s–15min) |
| **Soul Forge** | 5-7 ethical dilemmas from 14 TOML scenarios → 4-dim Compass Ethic weights, 16 combinatorial archetypes (4 pure + 12 compound), SHA-256 soul hash, visual sigil. (~2 min) |

New paths plug in by: (1) implementing `GenesisStrategy` in a `genesis-paths/*` crate, (2) registering the factory in `build_genesis_registry()` in `orion-api`, (3) producing a `SoulManifest` as the Complete step.

### Quick-Start Genesis Path

Quick Start is now a Genesis path (not a Hive creation checkbox). When the mentor selects Quick Start in the Genesis path selector:

1. Auto-fills soul template with `DEFAULT_PURPOSE` and `DEFAULT_PERSONALITY` from `orion-core::templates`.
2. Writes constitutional documents (soul.md, ethics.md, instincts.md, growth.md).
3. Signs constitutional documents with the held signing key.
4. Sets `birth_complete: true` — skips Emergence confirmation.

The agent still goes through Darkness (identity), Ignition (LLM), and Connectivity (API keys) before Quick Start is available as a Genesis path choice.

### Constitutional Documents

Templates live in `templates/`. At Emergence, soul.md, ethics.md, instincts.md are signed with Ed25519 key; signatures stored as `{doc}.sig`. Verified on every boot.

---

## Orchestration Layer (MVP)

The orchestration layer is implemented in `crates/orion-api/src/orchestration.rs` and integrated into API startup.

- **Persistence (per agent)**:
  - `orchestration_jobs.json` — Job definitions (cron, mode, goal template, policy)
  - `orchestration_job_logs.json` — Execution log entries (decision, significance, status, summary)
- **Modes**:
  - `id_check` — Lightweight local check via Id (`id_only`)
  - `agentic_run` — Directly launches an autonomous task
- **Significance**:
  - Levels: `low`, `medium`, `high`
  - Decisions: `silent_log`, `spawn_agentic`, `flag_mentor`
  - Policy toggles per job: `escalate_medium`, `flag_high_to_mentor`
- **Scheduler behavior**:
  - UTC cron interpretation
  - Background loop scans jobs on a fixed interval
  - Does not launch overlapping agentic runs for the same agent
- **Run provenance**:
  - Agentic run summaries include `source` (`manual` or `scheduled:<job_id>`)

---

## Security Model

- **Ed25519 identity**: External keypair generated in Darkness. Private key shown **once**, never persisted. Public key in `external_pubkey.bin`.
- **Hive lineage**: Master Ed25519 key generated per Hive (`master.key`). Signs each agent's pubkey at birth → `hive_lineage.sig`. Proves agent belongs to this Hive. Future: SAO/blockchain verification via `jbcupps/SAO` and `jbcupps/Ethical_AI_Reg`.
- **Document signing**: Format `{doc_name}|{tier}|{content}` → signature. `.sig` files store base64 signature, tier, timestamp. Verified on boot via `orion-core` verifier.
- **Per-agent Postgres scoping**: When using Postgres as the memory backend, the `birth` table uses `agent_id TEXT` as the primary key so multiple agents can share one database. Migration `003_birth_agent_scoped.sql` handles the schema change.
- **Secrets**: API keys in SecretsVault. DPAPI (Windows) for mentor keyring; plaintext stub on other platforms.
- **Local LLM URL**: Validated to localhost/127.0.0.1 to prevent SSRF.
- **MCP trust policy**: `McpTrustPolicy` restricts which hosts agent-registered MCP servers can connect to. Default allows localhost, `orion-toolbox`, and Docker-internal hostnames. Cloud metadata endpoints (169.254.169.254) are unconditionally blocked. `AgentBuilt` tier applies sandbox resource limits to dynamically registered skills.

---

## Current State vs Intended State

### Implemented now
- Full five-stage birth flow (Darkness → Emergence) with web UI progression and API support.
- Hive master key lineage: auto-generated master key signs each agent's pubkey at birth; verified on identity requests.
- Four modular Genesis paths (Quick Start, Direct Discovery, Soul Crystallization with web chat UI, Soul Forge).
- Per-agent Postgres birth table scoping (agent_id column) — multiple agents can share one Postgres database.
- Operational mentor chat with skill tool execution and separate activity logging.
- Tier-based model orchestration: Fast/Standard/Pro mapped to per-provider model selections.
- Provider model catalog: API-fetched and curated catalogs with validation and lifecycle warnings.
- Pro mode council: Rust-native MoA DAG with multi-provider draft, cross-provider critique, and synthesis. All three council phases receive full agent context (identity, ethics, instincts, operational awareness) and conversation history.
- Active provider preference: mentor selects preferred Ego provider per agent.
- Agentic loop with SSE timeline, mentor ask/confirm controls, run history, and cancellation.
- Orchestration MVP: scheduled jobs, significance policy, escalation decisions, and job logs.
- Dynamic MCP skill registration: agentic runs can connect to MCP servers and register new tools mid-run via `register_mcp_skill` synthetic tool. Registered skills persist across restarts.

### Near-term evolution areas
- Council-to-agentic bridge: parse Pro council output for capability-building directives and auto-dispatch as agentic runs.
- Sub-task spawning: `spawn_subtask` synthetic tool for agentic decomposition with depth limits.
- Script-as-tool wrapper: `register_script_tool` for lightweight non-MCP tool creation.
- WASM runtime: complete `WasmRuntimeStub` for sandboxed agent-built code execution.
- Promote orchestration from API module to dedicated crate when interfaces stabilize.
- Persist active agentic task state beyond in-memory process lifetime.
- Expand orchestration UI ergonomics (filters, richer per-job analytics, trend views).
- Extend Pro council with configurable selection strategies, weighted scoring, and provider-specific policies.

---

## Environment Variables

See `example.env` for the full list. Key variables:

| Variable | Purpose |
|----------|---------|
| `OPENAI_API_KEY` | Enables Ego (cloud) routing |
| `LOCAL_LLM_BASE_URL` | Local LLM endpoint (Ollama: `http://localhost:11434`, LM Studio: `http://localhost:1234`) |
| `MEMORY_BACKEND` | `sqlite` (default) or `postgres` |
| `DATABASE_URL` | Postgres connection string when using postgres backend |
| `BIRTH_MODEL` | Model for birth stages (default: `qwen2.5:3b-instruct`) |
| `ORION_DATA_DIR` | Agent data directory |
| `EXTERNAL_PUBKEY_PATH` | Explicit public key path override |
| `ORION_MASTER_KEY` | Encrypts secrets on Linux/macOS/Docker (ChaCha20-Poly1305). Without it, secrets are plaintext. |
| `MCP_SERVER_URLS` | Comma-separated MCP server URLs |
| `VITE_API_URL` | Frontend API base URL (empty = use proxy) |

---

## CI & Scripts

**CI** (`.github/workflows/ci.yml`): Runs on push/PR to main. Two jobs:
1. `docker-build-test` — Builds orion-build image, runs `UAT_MODE=fast` (fmt, clippy, build, test, frontend).
2. `docker-full-uat` — Full stack (postgres, ollama, API, frontend), `UAT_MODE=full` with postgres tests and UAT probes.

**Key scripts** (`scripts/`):
- `docker-test-suite.sh` — Canonical CI test runner (fast: fmt/clippy/build/test/frontend; full: adds postgres tests + UAT probes)
- `uat-probes.sh` — API endpoint smoke tests (UAT-1 through UAT-7)
- `local-verify.sh` / `local-verify.ps1` — Local Docker validation before push
- `agentic-uat.sh` — Full birth simulation with LLM keys (per Genesis path)

---

## Conventions

- **Commits**: Conventional commits: `feat()`, `fix()`, `refactor()`, `chore()`, `docs()`, `ci()`.
- **Rust**: `cargo fmt`, `cargo clippy` with `-D warnings`. **Always via Docker.**
- **Frontend**: React + Vite, TypeScript strict mode.
- **Docker**: Compose under `docker/`. Kubernetes manifests under `deploy/k8s/`.

---

## Cross-Repo Context (Phoenix Stack)

Orion Dock is part of the **Phoenix** AI Ethical Stack. It shares the Compass Ethic (4-dimension: duty/virtue/outcome/welfare), Ed25519 identity, and constitutional document patterns with:
- **SAO** (Secure Agent Orchestrator) — multi-agent coordination, identity verified via Ed25519.
- **Ethical_AI_Reg** — ethics layer for scoring; potential Superego integration.
- **abigail** — sister agent project.

Naming: `orion-*` crates; constitutional docs are `soul.md`, `ethics.md`, `instincts.md`. API routes under `/api/`.
