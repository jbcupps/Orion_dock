# Orion Application Test Plan (Docker-First)

This document defines the canonical test path for this repository. See [TEST_MATRIX.md](TEST_MATRIX.md) for test inventory and target coverage.

## Canonical Test Contract

All automated verification runs through Docker. No host Cargo or Node is required for the fast or full suite.

- **Fast suite** (default): Formatting, clippy, build, workspace tests, frontend typecheck + build, then (on host) docs check.
- **Full suite**: Same as fast, plus Postgres-backed integration tests, full-stack UAT probes (API health, status, Genesis path listing, create/forge flow, frontend smoke), and optionally `orion-uat` once per Genesis path when `TEST_LLM_KEY` and `TEST_SEARCH_KEY` are set.

The single runner is `scripts/docker-test-suite.sh`, invoked by the `orion-build` container with `UAT_MODE=fast` or `UAT_MODE=full`. It runs:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo build --workspace`
- `cargo test --workspace --no-fail-fast`
- Frontend: `npm ci` (or install), `npm run typecheck`, `npm run build` (inside container)
- If `UAT_MODE=full`: `cargo test -p orion-memory --features postgres` (when `DATABASE_URL` set), then `scripts/uat-probes.sh` (UAT-1 through UAT-7, including UAT-4/5/6 Genesis API probes). If `TEST_LLM_KEY` and `TEST_SEARCH_KEY` are set, builds `orion-uat --release` and runs it once per Genesis path (`direct`, `soul_forge`, `soul_crystallization`) with a fresh data dir each time.

Docs check: `scripts/check-docs-html.sh` (or `.ps1`) for `docs/index.html` runs on the host after the container step.

## Primary Commands

From repo root:

```bash
# Fast (same as CI required gate)
docker compose -f docker/docker-compose.yml build orion-build
docker compose -f docker/docker-compose.yml run --rm -e UAT_MODE=fast orion-build
./scripts/check-docs-html.sh .

# Full (full stack must be up: postgres, ollama, orion-api, frontend)
docker compose -f docker/docker-compose.yml --profile full up -d postgres ollama orion-api frontend
# ... wait for orion-api health ...
docker compose -f docker/docker-compose.yml --profile full run --rm -e UAT_MODE=full -e DATABASE_URL=postgres://orion:orion_dev@postgres:5432/orion orion-build
./scripts/check-docs-html.sh .
```

## Local Verification and UAT

- **Fast (same as CI):**  
  - Unix: `./scripts/local-verify.sh`  
  - Windows: `.\scripts\local-verify.ps1`
- **UAT (Docker build + test + docs check):**  
  - Unix: `./scripts/uat-run.sh` (fast), `./scripts/uat-run.sh --full` (full stack + probes)  
  - Windows: `.\scripts\uat-run.ps1` (fast), `.\scripts\uat-run.ps1 -Full` (full stack + probes)

Local scripts that run only `cargo test` (e.g. `build_check.sh`) use `--no-fail-fast` to match the Docker contract.

## CI Mapping

`.github/workflows/ci.yml` runs:

1. **docker-build-test (required):**  
   - `docker compose ... config -q` (base and `--profile full`)  
   - `docker compose ... build orion-build`  
   - `docker compose ... run --rm -e UAT_MODE=fast orion-build` (includes frontend typecheck/build in container)  
   - `scripts/check-docs-html.sh` (docs surface check)

2. **docker-full-uat:**  
   - Build and start full stack (postgres, ollama, orion-api, frontend)  
   - Wait for orion-api health  
   - `docker compose ... --profile full run --rm -e UAT_MODE=full -e DATABASE_URL=... orion-build`  
   - On failure: collect and upload artifacts

PR required gates are the fast suite plus docs check. The full UAT job runs the same runner contract against the live stack.

## Genesis Path Coverage

Genesis can be exercised via three paths: **Direct**, **Soul Crystallization** (QuickStart depth in UAT), and **Soul Forge**. The following ensure all paths are tested:

- **`UAT_GENESIS_PATH`** (env): Set to `direct` (default), `soul_forge`, or `soul_crystallization` when running the `orion-uat` binary or the agentic UAT scripts. The report table row for Genesis shows the path used (e.g. `| 4. Genesis (soul_forge) | [OK] |`).
- **orion-uat binary**: Reads `UAT_GENESIS_PATH`, calls `advance_to_genesis_with_path(path)`, then produces soul content per path (Direct: template fill; Soul Crystallization: engine check + template; Soul Forge: run three scenarios, `soul_output()`, assert archetype/soul_hash/sigil_art). Asserts `soul.md` and `growth.md` exist and contain the agent name after crystallization.
- **UAT probes** (`scripts/uat-probes.sh`): **UAT-4** — `GET /api/genesis/paths` returns at least 5 entries with `id`, `label`, `description`, `estimated_time`. **UAT-5** — Create agent, load, then drive through birth stages (UAT-8/9/10) to reach Connectivity; `POST /api/agents/:id/genesis/start` with `{"path":"soul_forge"}`; assert `state` scenario1 and `choices`. **UAT-6** — Three `POST /api/agents/:id/genesis/forge/select` with `{"choice":0}`; assert final response has `archetype` and `soul_hash`. **UAT-8** — `GET /api/agents/:id/birth/state` returns 200 with `stage: "Darkness"` for a new agent; validates Postgres backend connectivity, spawn_blocking runtime isolation, migration availability in Docker. **UAT-9** — `POST /api/agents/:id/birth/advance-darkness` returns 200; validates stage transition from Darkness to Ignition. **UAT-10** — `POST /api/agents/:id/birth/ignition` with `{}` returns 200; validates stage transition from Ignition to Connectivity (required before Genesis can start).
- **Full mode** (`docker-test-suite.sh`): When `TEST_LLM_KEY` and `TEST_SEARCH_KEY` are set, runs `orion-uat` once for each of `direct`, `soul_forge`, and `soul_crystallization` with a dedicated temp data dir per run.

## Agentic UAT (birth to usable operation)

A full automated UAT run drives the application from birth through early operation. The only action you take is providing an LLM API key and a search API key at the proper points.

1. Set environment variables (or have the Agent prompt you and set them):
   - `TEST_LLM_KEY` — e.g. OpenAI, Anthropic (required).
   - `TEST_SEARCH_KEY` — e.g. Tavily (required).
   - Optional: `TEST_LLM_PROVIDER`, `TEST_SEARCH_PROVIDER` (default `auto`; detected from key prefix).
   - Optional: `UAT_GENESIS_PATH` — `direct` (default), `soul_forge`, or `soul_crystallization`; determines which Genesis path the run uses. The report shows the path in the Genesis row.
2. Run the script:
   - Unix: `./scripts/agentic-uat.sh`
   - Windows: `.\scripts\agentic-uat.ps1`
3. The script starts the full stack (postgres, ollama, orion-api, frontend), builds and runs `orion-uat` inside the container (Darkness → Ignition → Connectivity → Genesis → Emergence → early operation), then writes a dated report.
4. Output: `UAT_REPORT_YYYY-MM-DD.md` with sectional checks: each stage and "API status visible" are marked [OK] or [X], plus a log excerpt.

The `orion-uat` binary (crate `crates/orion-uat`) drives `BirthOrchestrator` headlessly and injects the provided keys during Connectivity, then advances to Genesis with the selected path (`UAT_GENESIS_PATH`), crystallizes soul (path-aware: Direct template, Soul Crystallization engine check, or Soul Forge scenarios + `soul_output()`), and completes emergence. Early operation is verified by a quick router check.

## Birth Flow Connectivity (Regression Coverage)

The following issues were identified during interactive testing and are now covered by UAT-8/9/10:

| Issue | Root Cause | Fix | UAT Coverage |
|-------|-----------|-----|--------------|
| 502 Bad Gateway on birth/state | `PostgresStore::connect()` creates a nested Tokio runtime inside async Axum handlers, causing a panic | Wrapped all `BirthOrchestrator` endpoints in `tokio::task::spawn_blocking()` | UAT-8: birth/state returns 200 with Postgres backend |
| "postgres backend requires orion-memory with postgres feature" | `orion-birth/Cargo.toml` depended on `orion-memory` without the `postgres` feature flag | Added `features = ["postgres"]` to `orion-birth/Cargo.toml` | UAT-8: orchestrator creates successfully with Postgres |
| "while resolving migrations: No such file or directory" | `Dockerfile.api` runtime stage only copied the binary; `env!("CARGO_MANIFEST_DIR")` migrations path was absent | Added `COPY crates/orion-memory/migrations` to `Dockerfile.api` runtime stage | UAT-8: migrations run on first connect in Docker |
| Birth flow stuck at "Generating identity" | No API endpoints to drive Darkness → Ignition → Connectivity stages from the web UI | Added `GET birth/state`, `POST advance-darkness`, `POST ignition` endpoints and corresponding frontend UI | UAT-8/9/10: full birth stage progression |

## Failure Triage

On failure, collect artifacts for triage:

- Unix: `./scripts/collect-failure-artifacts.sh`
- Windows: `.\scripts\collect-failure-artifacts.ps1`

This writes `artifacts/env.txt` and, if `UAT_LOG_CAPTURE` was set during the run, copies the captured log to `artifacts/uat-failure.log`. CI may upload the `artifacts/` directory on failure.

## Optional Focused Debugging

From an interactive container shell:

```bash
docker compose -f docker/docker-compose.yml up -d orion-dev
docker compose -f docker/docker-compose.yml exec orion-dev bash
```

Then run targeted crate tests, for example:

```bash
cargo test -p orion-core
```
