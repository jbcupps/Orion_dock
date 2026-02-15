# Orion Dock Project Context

## Purpose

Orion Dock is a Docker-first desktop-agent platform that combines:

- a guided **birth lifecycle** for agent identity and constitutional setup,
- a **mentor-facing operational chat** experience,
- a **skill execution runtime** with sandbox controls,
- and an **orchestration layer** for scheduled background jobs and escalation.

The core goal is an agent that can act autonomously when appropriate while preserving clear safety and mentor control boundaries.

## Quick start for new contributors

1. **Run the full stack** (build, start, health):
   ```powershell
   .\scripts\dev-stack.ps1
   ```
   Web UI: http://localhost:3000 — API: http://localhost:8080

2. **Tear everything down** (clean slate):
   ```powershell
   .\scripts\dev-stack.ps1 -Down
   ```

3. **Check Rust compiles** (must run in Docker):
   ```bash
   docker compose -f docker/docker-compose.yml run --rm orion-dev bash -c "cargo check --workspace"
   ```

4. **Run Rust tests**:
   ```bash
   docker compose -f docker/docker-compose.yml run --rm orion-dev bash -c "cargo test --workspace --no-fail-fast"
   ```

5. **Frontend typecheck** (on host):
   ```bash
   cd frontend && npm install && npx tsc --noEmit
   ```

See `CLAUDE.md` for more commands (UAT, lint, single-crate tests, postgres tests).

## Runtime Model

- **Id (local model)**:
  - birth-stage interactions,
  - lightweight periodic checks,
  - privacy-sensitive/local-only reasoning.
- **Ego (cloud model, tier-aware)**:
  - primary mentor-facing operational conversation,
  - deeper reasoning/tool-planning tasks,
  - model selected by tier: Fast (lightweight), Standard (balanced), Pro (highest capability),
  - per-provider model mappings with catalog validation,
  - Pro tier uses native Rust council debate/synthesis across providers when multiple providers are available,
  - agentic runs can self-extend by registering MCP server tools at runtime (`register_mcp_skill`).
- **Orchestration (policy + scheduler)**:
  - runs scheduled UTC cron jobs,
  - scores significance (`low`, `medium`, `high`),
  - decides action (`silent_log`, `spawn_agentic`, `flag_mentor`).
- **Superego (safety layer)**:
  - pre-check support exists in routing; expanded model-driven oversight is planned.

## Main User Flows

1. **Birth flow**: Darkness -> Ignition -> Connectivity -> Genesis -> Emergence  
   Produces cryptographic identity and signed constitutional docs.
2. **Quick-start birth**: One-call agent creation (`quick_start: true`) with auto identity and standard constitutional documents. Skips interactive stages.
3. **Operational chat**: Mentor chats with the agent (Ego-primary, tier-aware model selection).
4. **Agentic run**: Mentor starts autonomous multi-step tasks with timeline + approvals.
5. **Scheduled orchestration jobs**: Periodic jobs run in background and escalate only when needed.
6. **Provider management**: Mentor configures per-provider tier models, validates against catalogs, sets active provider preference.
7. **Dynamic tool creation**: Agent deploys an MCP server (e.g., in toolbox container) and registers it as a skill, extending its own capabilities within a single agentic run.

## Key Backend Areas

- `crates/orion-api/src/main.rs`:
  - HTTP routes,
  - operational chat handler (tier-aware model resolution),
  - agentic endpoints (tier-aware model resolution),
  - tier-model and provider catalog management endpoints,
  - quick-start birth endpoint,
  - Pro council integration,
  - orchestration endpoints + scheduler startup.
- `crates/orion-api/src/agentic.rs`:
  - autonomous loop,
  - tool-call flow, mentor confirmation, SSE events,
  - run summary persistence.
- `crates/orion-api/src/orchestration.rs`:
  - job models/persistence,
  - cron scheduling helpers,
  - significance and decision engine.
- `crates/orion-core/src/config.rs`:
  - config schema v7: tier models, provider catalog, active provider preference.
- `crates/orion-capabilities/src/cognitive/model_catalog.rs`:
  - provider model catalog fetching (OpenAI, Anthropic, Google, xAI, Perplexity),
  - curated fallback catalogs, lifecycle warnings.
- `crates/orion-router/src/router.rs`:
  - Id/Ego routing, fallback behavior, tier-aware ego model override.
- `crates/orion-skills/src/*`:
  - skill registry, executor, sandbox, trust tiers.
- `crates/orion-router/src/council.rs`:
  - Pro-tier native Rust MoA DAG for parallel drafting, critique, and synthesis.

## Frontend Areas

- `frontend/src/App.tsx`:
  - overall app surface composition.
- `frontend/src/components/OperationalChat.tsx`:
  - mentor chat + activity log display.
- `frontend/src/components/AgenticPanel.tsx`:
  - start/manage agentic runs with SSE timeline.
- `frontend/src/components/JobsTable.tsx`:
  - historical and active run list.
- `frontend/src/components/OrchestrationJobsPanel.tsx`:
  - scheduled job CRUD, run-now, and orchestration logs.

## Persistence Notes

Per-agent data is stored under `ORION_DATA_DIR/identities/<agent_id>/`.

Important files include:

- `config.json` (agent config, routing state, tier models, provider catalog cache, active provider preference, `mcp_servers` for agent-registered MCP connections)
- `operational_chat.json` (chat history)
- `agentic_runs/*.json` (agentic summaries, including run source)
- `orchestration_jobs.json` (scheduled jobs)
- `orchestration_job_logs.json` (orchestration execution logs)
- constitutional docs + signatures (`soul.md`, `ethics.md`, `instincts.md`, `.sig`)

## Guardrails and Safety

- Skill execution goes through `SkillExecutor` with tier-aware limits.
- Sandbox permission filtering differs by trust tier (`Verified`, `AgentBuilt`, `Untrusted`).
- Mentor confirmation is available for risky tool actions in agentic mode.
- Orchestration is designed to avoid noisy interruptions and only surface high-significance events.

## Build/Test Constraints

- Rust build/test must run in Docker (`orion-dev`), not host-native.
- Frontend typecheck/build runs on host (`npm`, `tsc`, `vite`).

## Current Direction

The current architecture favors:

- Ego for mentor conversation quality, with **tier-based model selection** (Fast/Standard/Pro) for cost/quality balance,
- Id for lightweight autonomous checks,
- orchestration as the control plane for when and how autonomy escalates,
- **provider catalog management** so mentors can validate and customize model choices,
- **Pro council engine** for highest-tier multi-provider drafting/critique/synthesis when multiple providers are available.

Dynamic self-extension via MCP skill registration allows the agent to build new capabilities at runtime, with `AgentBuilt` trust tier and MCP trust policy for safety.

This keeps the system capable without overloading the local lightweight model, while giving mentors granular control over model selection and provider preference.
