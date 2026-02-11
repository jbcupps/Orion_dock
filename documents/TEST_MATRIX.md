# Orion Dock Test Matrix

Canonical inventory of current tests and target coverage per crate and UI surface. Aligns with [APPLICATION_TEST_PLAN.md](APPLICATION_TEST_PLAN.md) and Docker-first CI.

## Current Test Inventory

### Crates (unit / integration in-tree)

| Crate | Critical modules | Test locations | Notes |
|-------|------------------|----------------|-------|
| orion-core | config, superego, secrets, vault, verifier, local_llm_url | config.rs, superego.rs, secrets.rs, vault.rs, verifier.rs, local_llm_url.rs, global_config.rs, encrypted_storage.rs, email_auth.rs, system_prompt.rs, sao_bridge.rs | Superego and config are routing/safety-critical |
| orion-router | router | router.rs | Id/Ego routing, provider selection, streaming, tools |
| orion-birth | stages, chat, prompts | stages.rs, chat.rs, prompts.rs | Birth orchestration, tool parsing, router wiring |
| orion-memory | store, sqlite_store, postgres_store, schema | store.rs | Persistence; SQLite in-memory and file-backed |
| orion-capabilities | cognitive (provider, local_http, openai_compatible, anthropic, validation), sensory (web_fetch, web_search) | cognitive/*.rs, sensory/*.rs | Provider stubs and validation |
| orion-skills | executor, registry, watcher, protocol/mcp, transport/imap | executor.rs, lib.rs, watcher.rs, protocol/mcp.rs, transport/imap.rs | Sandbox, timeout, tool execution |
| orion-email | pkce | pkce.rs | Auth helpers |
| soul-forge | lib.rs (state/calibration), main.rs (TUI) | lib.rs `#[cfg(test)]` | TUI state and deterministic calibration tested in lib |

### Skills (workspace members)

| Skill | Test locations | Focus |
|-------|----------------|-------|
| skill-filesystem | lib.rs | Path traversal, read/write/list/search |
| skill-shell | lib.rs | Safety (rm -rf, fork bomb, sudo, shutdown), echo/fail/timeout |
| skill-http | lib.rs | Manifest, SSRF checks |
| skill-web-search | lib.rs | Manifest, blocked query, missing key |
| skill-perplexity-search | lib.rs | Manifest, health, blocked query |
| skill-web-browse | lib.rs | SSRF, search-only, blocked URL |

### Frontend / UI surface

| Surface | Location | Current tests |
|---------|----------|---------------|
| soul-forge TUI | crates/soul-forge/src/lib.rs, main.rs | lib: state, archetype, crystallize, tick_boot |
| Static download page | docs/index.html | scripts/check-docs-html.sh, check-docs-html.ps1 |

### Integration / E2E

- **orion-memory**: `tests/integration_persistence.rs` (SQLite), `tests/integration_postgres.rs` (Postgres, when `DATABASE_URL` set and feature `postgres`).
- **Full-stack UAT**: `scripts/uat-probes.sh` runs from the orion-build container when `UAT_MODE=full`; covers API health, status, ready, and optional frontend smoke. Automated in CI via job `docker-full-uat`.

---

## Target Test Matrix

### Frontend (current scope)

- **soul-forge**: Smoke/behavior tests for entry/exit and deterministic calibration prompts (library extraction or test harness).
- **docs/index.html**: Static checks (link format, OS detection branches, anchors) via script or small test.

### Backend crates

- **orion-core**: Config parsing, superego allow/deny, vault/verifier edge cases; negative paths (malformed env).
- **orion-router**: Id/Ego routing, fallback, streaming/tool-assisted paths; deterministic fixtures for classify and provider failure.
- **orion-birth**: Stage transitions, build_birth_router, parse_tool_requests, execute_store_provider_key; negative paths.
- **orion-memory**: SQLite and Postgres (with feature) store behavior; migration/schema handling.
- **orion-capabilities**: Provider selection, validation, timeout/unavailable behavior.
- **orion-skills**: Executor timeout/denial, sandbox policy, registry concurrency; skill-level safety tests retained.

### Routing (cross-cutting)

- Routing mode (IdPrimary / EgoPrimary) with explicit outcomes.
- Superego enforcement before/after route.
- Streaming and tool-assisted route stability.

### Integration

- Birth orchestration → router call → persistence (single flow).
- Skill execution boundaries (allowed vs denied).
- Optional: full-stack with Postgres + local LLM in CI (label-triggered or scheduled).

### UAT (automated, `scripts/uat-probes.sh`)

- UAT-1 Startup health (`/health`)
- UAT-2 Birth journey (`/api/status` birth_complete / birth_stage)
- UAT-3 Routing correctness (`/api/status` local_llm_configured)
- UAT-4 Genesis path listing (`/api/genesis/paths` returns 5+ entries with id/label/description/estimated_time)
- UAT-5 Create agent and start Soul Forge Genesis (create → load → birth flow → genesis/start → assert scenario1)
- UAT-6 Soul Forge scenario progression (3x forge/select → assert archetype + soul_hash)
- UAT-7 Failure handling (`/ready`)
- UAT-8 Birth state with Postgres backend (`GET /api/agents/:id/birth/state` returns 200, stage "Darkness"; validates spawn_blocking runtime isolation, Postgres feature flag, Docker migration availability)
- UAT-9 Advance past Darkness (`POST /api/agents/:id/birth/advance-darkness` returns 200; Darkness → Ignition transition)
- UAT-10 Ignition to Connectivity (`POST /api/agents/:id/birth/ignition` returns 200; Ignition → Connectivity transition, prerequisite for Genesis)

---

## Naming and Fixtures

- **Pure unit**: `test_<module>_<behavior>` (e.g. `test_routing_decision_id_primary`).
- **Integration**: `test_integration_<flow>` or in `tests/` as `integration_*.rs`.
- **Fixtures**: Shared test data in crate `tests/fixtures/` or `#[cfg(test)]` modules; deterministic env vars for routing/birth.

---

## CI Mapping

- **Fast (PR required)**: `docker-build-test` — fmt, clippy, build, `cargo test --workspace --no-fail-fast`, frontend typecheck + build in container; then docs check on host.
- **Full UAT**: `docker-full-uat` — full stack up, then `orion-build` with `UAT_MODE=full` (Postgres tests + `scripts/uat-probes.sh`).
- **Scheduled**: Dependency audit, artifact retention (as needed).

See [APPLICATION_TEST_PLAN.md](APPLICATION_TEST_PLAN.md) and `.github/workflows/ci.yml` for commands and gates.
