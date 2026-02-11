# Contributing to Orion Dock

Thanks for contributing.
This repository follows a Docker-first development and CI model.

## Prerequisites

- Docker + Compose v2
- Git

## Development Setup

```bash
git clone https://github.com/jbcupps/orion.git
cd orion

docker compose -f docker/docker-compose.yml build orion-build
docker compose -f docker/docker-compose.yml run --rm orion-build
```

For an interactive shell:

```bash
docker compose -f docker/docker-compose.yml up -d orion-dev
docker compose -f docker/docker-compose.yml exec orion-dev bash
```

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
- Ensure CI passes
- Avoid unrelated formatting-only churn

## Security

Do not commit secrets, credentials, or private keys.
For vulnerability reporting, see `.github/SECURITY.md`.
