# Contributing to Orion Dock

Thanks for contributing.
This repository follows a Docker-first development and CI model.

## Prerequisites

- Docker + Compose v2
- Git

## Development Setup

```bash
git clone https://github.com/jbcupps/Orion_dock.git
cd Orion_dock

docker compose -f docker/docker-compose.yml build orion-build
docker compose -f docker/docker-compose.yml run --rm orion-build
```

For an interactive shell:

```bash
docker compose -f docker/docker-compose.yml up -d orion-dev
docker compose -f docker/docker-compose.yml exec orion-dev bash
```

### Pro Council Engine

Pro-tier provider comparison is built in via the native Rust council engine; no external sidecar service is required.

## Branch and Commit Conventions

- Create short-lived branches from `main`
- Use Conventional Commits (`feat:`, `fix:`, `docs:`, `ci:`, etc.)
- Keep commits focused and reviewable

## Local Quality Gate

Before opening a PR, run:

```bash
./scripts/local-verify.sh
```

Or on Windows:

```powershell
.\scripts\local-verify.ps1
```

These scripts execute the same Docker validation path used in CI.

## Pull Requests

- Fill out the PR template
- Link related issues when applicable
- Run local verification (see above). GitHub Actions CI and Dependabot may be disabled for this repo; when enabled, ensure CI passes.
- Avoid unrelated formatting-only churn

## Security

Do not commit secrets, credentials, or private keys.
For vulnerability reporting, see `.github/SECURITY.md`.

## Autonomy-First Development

The agent's ability to act independently using its skills is the core product value. When making changes that affect routing, tool execution, sandbox policies, or skill registration:

- **Always use `route_with_tools()`** for structured function-calling. The text-based `tool_request` fallback exists for backward compatibility, not as the primary path.
- **Security hardening must not blanket-block tool execution.** Use risk-based policies: block high-risk/mutation tools where needed, allow read-only tools.
- **Include autonomy regression coverage** — any security change must be accompanied by a test proving the agent can still search the web, read files, execute safe commands, and store credentials.
- **Missing keys are solvable states.** The agent should tell the user what is needed, not claim inability.
