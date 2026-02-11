# Canonical UAT entrypoint: run quality gates and optional full-stack checks.
# Usage:
#   .\scripts\uat-run.ps1           # Fast: Docker build+test + docs check
#   .\scripts\uat-run.ps1 -Full     # Start full stack, then run tests + UAT probes (requires Docker)
param([switch]$Full)
$ErrorActionPreference = "Stop"
$RepoRoot = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
Set-Location $RepoRoot

# Docker writes build/container progress to stderr.  PowerShell's "Stop" error
# preference converts any stderr into a terminating NativeCommandError.  We
# temporarily suppress that and let stderr flow directly to the console (no 2>&1
# so PowerShell never wraps it in an ErrorRecord).
function Invoke-Docker {
    param([string]$Command)
    $prev = $ErrorActionPreference
    $ErrorActionPreference = "SilentlyContinue"
    cmd /c $Command
    $ec = $LASTEXITCODE
    $ErrorActionPreference = $prev
    if ($ec -ne 0) { throw "$Command failed (exit $ec)." }
}

Write-Host "=== UAT: Docker build and test ==="
Invoke-Docker "docker compose -f docker/docker-compose.yml config -q"
Invoke-Docker "docker compose -f docker/docker-compose.yml build orion-build"

if ($Full) {
    Write-Host "=== UAT (full): starting full stack ==="
    Invoke-Docker "docker compose -f docker/docker-compose.yml --profile full build orion-api frontend"
    Invoke-Docker "docker compose -f docker/docker-compose.yml --profile full up -d postgres ollama orion-api frontend"
    $env:DATABASE_URL = if ($env:DATABASE_URL) { $env:DATABASE_URL } else { "postgres://orion:orion_dev@localhost:5432/orion" }
    $env:LOCAL_LLM_BASE_URL = if ($env:LOCAL_LLM_BASE_URL) { $env:LOCAL_LLM_BASE_URL } else { "http://localhost:11434" }
    $readyScript = Join-Path $RepoRoot "scripts\ready-full-stack.ps1"
    if (Test-Path $readyScript) { & $readyScript }
    Write-Host "=== UAT (full): running build+test+probes in container ==="
    # Container must use service hostname "postgres", not localhost
    Invoke-Docker "docker compose -f docker/docker-compose.yml --profile full run --rm -e UAT_MODE=full -e DATABASE_URL=postgres://orion:orion_dev@postgres:5432/orion orion-build"
} else {
    Invoke-Docker "docker compose -f docker/docker-compose.yml run --rm -e UAT_MODE=fast orion-build"
}

Write-Host "=== UAT: docs/index.html check ==="
$checkScript = Join-Path $RepoRoot "scripts\check-docs-html.ps1"
& $checkScript -RepoRoot $RepoRoot

Write-Host "UAT completed."
