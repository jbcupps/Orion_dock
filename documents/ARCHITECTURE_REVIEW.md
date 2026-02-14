# Architecture Review — Orion Dock

**Date:** 2026-02-14
**Version:** 0.0.1

---

## Executive Summary

Orion Dock implements a monolithic Dockerized agent architecture: a single compiled binary (`orion-api`) managing identity, routing, tool execution, and orchestration in-process. This review validated five architectural findings, implemented four quick-win mitigations, and documents a roadmap for larger structural improvements.

| # | Finding | Severity | Status |
|---|---------|----------|--------|
| 1 | Plaintext secret storage on Linux/Docker | HIGH | **MITIGATED** — ChaCha20-Poly1305 with `ORION_MASTER_KEY` |
| 2 | Skill isolation is capability-based, not OS-level | MEDIUM-HIGH | DOCUMENTED (roadmap) |
| 3 | Volatile agentic task state (in-memory only) | HIGH | DOCUMENTED (roadmap) |
| 4 | Context trimming by message count, not tokens | MEDIUM | **MITIGATED** — token-aware heuristic |
| 5 | Superego safety checks are pattern-based | MEDIUM | **PARTIALLY MITIGATED** — Unicode normalization + expanded patterns |

Additionally: LLM health check added before agentic runs (pre-flight verification).

---

## SOTA Gap Analysis

| Dimension | Orion Status | SOTA Benchmark | Gap |
|-----------|-------------|----------------|-----|
| Orchestration | Linear loop | Graph-based (DAG), hierarchical | Partial. Lacks graph definitions for complex workflows. |
| Sandboxing | In-process capability checks | Docker/WASM/MicroVM isolation | Minimal. Native skills share memory space. |
| Memory | SQLite/Postgres store | Semantic graphs, ephemeral/long-term split | Competitive. DB schema supports vector/graph; retrieval logic is basic. |
| Routing | Static Id/Ego with Superego pre-check | Learned routing, model cascading | Competitive. The Id/Ego/Superego pattern is architecturally sound. |
| Identity | Crypto-native (Ed25519) | Config-based / stateless | **Leading.** Signed constitutional documents are unique. |
| Self-Improvement | None | Meta-prompting, strategy optimization | Missing. No feedback loop modifies agent behavior. |
| Persistence | Volatile tasks, disk summaries | Checkpointing / replayability | Minimal. Container restart kills active agentic runs. |

---

## Detailed Findings

### 1. [HIGH] Insecure Secret Storage on Linux/Docker

**Status:** MITIGATED

**Description:** The `dpapi.rs` non-Windows path was a plaintext passthrough (`Ok(data.to_vec())`). All secrets — API keys in `secrets.bin`, signing keys in `keys.bin` and `signing.key`, master keys in `master.key` — were stored unencrypted in Docker volumes.

**File:** `crates/orion-core/src/dpapi.rs`

**Mitigation:** Replaced plaintext stubs with ChaCha20-Poly1305 AEAD encryption keyed from `ORION_MASTER_KEY` env var. Wire format: `[0x01 version][12-byte nonce][ciphertext + auth tag]`. Backward-compatible: without the env var, behavior is unchanged (plaintext with warning). Legacy plaintext data is transparently readable when the key is first set, and re-encrypted on next save.

**Residual risk:** Dev environments without `ORION_MASTER_KEY` still use plaintext by design. Production deployments must set the env var.

### 2. [MEDIUM-HIGH] Skill Isolation is Capability-Based, Not OS-Level

**Status:** DOCUMENTED (Roadmap)

**Description:** Skills compile as Rust crate dependencies and run in the same memory space as the API via `Arc<dyn Skill>` trait objects. The `SkillSandbox` (`crates/orion-skills/src/sandbox.rs`) enforces trust tiers (Verified/AgentBuilt/Untrusted) with permission checks, timeouts (10-30s), and concurrency limits (2-10). However, a malicious or buggy skill can still panic (crashing the process) or bypass sandbox checks via direct `std::fs` calls.

**Files:** `crates/orion-skills/src/executor.rs`, `crates/orion-skills/src/sandbox.rs`, `crates/orion-skills/src/runtime/wasm.rs` (stub)

**Interim controls:** Trust tier filtering strips dangerous permissions from untrusted skills. Audit logging tracks all sandbox decisions.

**Roadmap:** Implement Wasmtime-based WASM runtime for Untrusted and AgentBuilt tier skills. The `WasmRuntimeStub` in `runtime/wasm.rs` provides the integration point.

### 3. [HIGH] Volatile Agentic Task State

**Status:** DOCUMENTED (Roadmap)

**Description:** Active `AgenticTask` instances are stored in an in-memory `HashMap` in `AppState` (`Arc<TokioMutex<HashMap<String, Arc<TokioMutex<AgenticTask>>>>>`). Only completion summaries are persisted to disk via `persist_run_summary()`. A deployment, crash, or restart kills all running autonomous tasks with no ability to resume.

**Files:** `crates/orion-api/src/main.rs` (AppState), `crates/orion-api/src/agentic.rs` (run loop, persist)

**Roadmap:** Persist task state (messages, steps, current turn) to SQLite/Postgres after every LLM turn. On startup, scan for incomplete tasks and respawn their loops.

### 4. [MEDIUM] Context Trimming by Message Count

**Status:** MITIGATED

**Description:** `trim_context` trimmed by message pair count (`max_pairs`), not by token estimate. Since messages vary wildly in size (a tool output can be 10x a thinking message), count-based trimming was unreliable — potentially discarding too many small messages or keeping too few large ones.

**File:** `crates/orion-api/src/agentic.rs`

**Mitigation:** Replaced with token-aware trimming using a character-based heuristic (~4 chars/token + 4 overhead per message). Budget: Auto=16K, ThinkHard=24K, ThinkHarder=32K tokens. The function now counts from the end, keeping as many recent messages as fit in the remaining budget after reserving space for system prompt and goal.

**Future:** Drop-in `tiktoken-rs` for exact token counts per model.

### 5. [MEDIUM] Superego is Pattern-Based

**Status:** PARTIALLY MITIGATED

**Description:** `check_message()` and `check_search_query()` used simple `.to_lowercase()` + `.contains()` matching. This was trivially bypassed with Unicode fullwidth characters (e.g., fullwidth "ignore"), zero-width character injection, or soft hyphens.

**File:** `crates/orion-core/src/superego.rs`

**Mitigation:** Added NFKC Unicode normalization (collapses fullwidth/compatibility characters to ASCII), zero-width character stripping, and expanded jailbreak pattern coverage (DAN, bypass, override, jailbreak keywords).

**Residual risk:** Pattern-based checks remain fundamentally limited. The optional LLM-based Superego path exists in the router (`with_superego()`) but is not enabled by default. A lightweight classification model would provide stronger coverage.

---

## Improvement Roadmap

### Phase 1: Security Hardening (Implemented)

- [x] ChaCha20-Poly1305 encryption for non-Windows secret storage
- [x] Unicode normalization in Superego safety checks
- [x] Expanded jailbreak pattern coverage
- [x] LLM health check before agentic runs
- [x] Token-aware context window management

### Phase 2: Reliability

- [ ] Task checkpointing to SQLite/Postgres (per-turn state persistence)
- [ ] Task recovery on API restart (scan incomplete tasks, respawn loops)
- [ ] Graceful shutdown with in-flight task persistence
- [ ] Missed scheduled job catch-up after downtime

### Phase 3: Isolation & Architecture

- [ ] Wasmtime-based WASM runtime for untrusted skills (via `runtime/wasm.rs`)
- [ ] WIT interface definitions for skill capabilities (Network, FileSystem)
- [ ] Graph-based orchestration (StateGraph with Plan/Act/Observe nodes)
- [ ] Internal skills exposed via MCP schema for unified tool interface

### Phase 4: Observability & Intelligence

- [ ] `tiktoken-rs` integration for exact token counting
- [ ] Per-agent token usage tracking and budgeting
- [ ] Orchestration analytics (trend views, per-job metrics)
- [ ] Self-improvement feedback loop (strategy optimization from run outcomes)

---

## Appendix: Dependency Changes

| Crate | Version | Added To | Purpose |
|---|---|---|---|
| `chacha20poly1305` | 0.10 | workspace + orion-core | AEAD encryption for secrets |
| `sha2` | 0.10 | workspace + orion-core | Key derivation from ORION_MASTER_KEY |
| `unicode-normalization` | 0.1 | orion-core | Superego Unicode bypass prevention |
