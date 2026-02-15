# Changelog

All notable changes to Orion Dock are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/), and this project adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- **Tier-based model orchestration**: Fast/Standard/Pro thinking modes mapped to per-provider model selections (OpenAI, Anthropic, Google, xAI, Perplexity)
- **Provider model catalog**: API-fetched and curated catalogs with validation, lifecycle warnings, and refresh support
- **Council engine** (`crates/orion-router/src/council.rs`): native Rust MoA DAG for Pro-tier multi-provider debate and synthesis
- **Quick-start birth**: One-call agent creation with auto identity generation and standard constitutional documents
- **Active provider preference**: Mentor-selectable preferred Ego provider per agent
- **Tier model API endpoints**: GET/PUT tier-models, refresh/validate/reset catalogs, set active provider
- **Config schema v7**: `tier_models`, `active_provider_preference`, `provider_catalog`
- **Default soul templates**: `DEFAULT_PURPOSE`, `DEFAULT_PERSONALITY`, `fill_soul_template_default()` for quick-start birth
- **Router ego model override**: `build_ego_provider()` accepts optional model override for tier-based selection
- **Frontend tier UI**: Fast/Standard/Pro labels in chat and agentic panels, catalog management, validation badges, provider selection
- Cooperative skill install with mentor script fallback
- Attachment support and file ingestion in chat
- Karpathy cognitive discipline integration into agent model
- Architecture security review with mitigations for 5 critical findings
- Orchestration MVP: scheduled jobs, significance policy, escalation, and job logs
- Agent export, autonomous agentic loop with SSE streaming
- Full five-stage birth flow with web UI and modular Genesis paths
- Public release readiness: LICENSE, CONTRIBUTING.md, CODE_OF_CONDUCT.md, SECURITY.md
- CI workflow for pull request validation (cargo fmt, clippy, test, frontend build)
- CodeQL static analysis workflow
- GitHub issue and PR templates
- CODEOWNERS file
- All GitHub Actions pinned by commit SHA for supply chain security

### Changed

- Enhanced README.md with badges, system requirements, and troubleshooting
- Updated .gitignore with additional patterns for generated and data files
- Router now threads tier-based model selection through both operational chat and agentic loop
- OpenAiProvider supports `.with_model()` for configurable model selection

## [0.0.1] - 2026-02-03

### Added

- Initial release of Abigail desktop agent
- Interactive birth flow with staged onboarding
- First-run Ed25519 signing key generation with one-time private key presentation
- Constitutional document signing and verification (soul.md, ethics.md, instincts.md)
- Local LLM discovery and manual connect for Ollama/LM Studio-compatible endpoints
- In-app API key vaulting and validation for cloud/model/search providers
- Dual persona UI modes (surface chat and Forge mode toggle)
- Id/Ego routing: local LLM (Id) for routine queries, cloud LLM (Ego) for complex queries
- Skill-based tool execution with web-search capability
- DPAPI-encrypted secrets storage on Windows
- Cross-platform builds: Windows (NSIS), Ubuntu (deb), macOS (dmg universal binary)
- npm CLI installer (`npx abigail-desktop`)
- Docker development and build containers
- Security audit CI (cargo audit, npm audit)
- Dependabot configuration for Cargo, npm, and GitHub Actions

[Unreleased]: https://github.com/jbcupps/Orion_dock/compare/v0.0.1...HEAD
[0.0.1]: https://github.com/jbcupps/Orion_dock/releases/tag/v0.0.1
