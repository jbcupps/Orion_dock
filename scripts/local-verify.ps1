param()

$ErrorActionPreference = "Stop"

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

$RepoRoot = (Get-Item $PSScriptRoot).Parent.FullName
Push-Location $RepoRoot
try {
    Write-Host "Validating docker compose file..." -ForegroundColor Cyan
    Invoke-Docker "docker compose -f docker/docker-compose.yml config -q"

    Write-Host "Validating full-stack compose profile..." -ForegroundColor Cyan
    Invoke-Docker "docker compose -f docker/docker-compose.yml --profile full config -q"

    Write-Host "Building Docker validation image..." -ForegroundColor Cyan
    Invoke-Docker "docker compose -f docker/docker-compose.yml build orion-build"

    Write-Host "Running Docker validation pipeline (fast)..." -ForegroundColor Cyan
    Invoke-Docker "docker compose -f docker/docker-compose.yml run --rm -e UAT_MODE=fast orion-build"

    Write-Host "Docker-first local verification completed." -ForegroundColor Green
}
finally {
    Pop-Location
}
