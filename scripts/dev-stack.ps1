# dev-stack.ps1 — Build and launch the full Orion stack for manual testing.
#
# Usage:
#   .\scripts\dev-stack.ps1          # Build images, start stack, open browser
#   .\scripts\dev-stack.ps1 -Down    # Tear down the stack
#   .\scripts\dev-stack.ps1 -Rebuild # Force rebuild images before starting
#
# Once running:
#   Web UI:  http://localhost:3000
#   Postgres: localhost:5432  (orion / orion_dev)  — loopback only
#   Ollama:   localhost:11434                      — loopback only

param(
    [switch]$Down,
    [switch]$Rebuild
)

$ErrorActionPreference = "Stop"
$RepoRoot = (Get-Item $PSScriptRoot).Parent.FullName
Push-Location $RepoRoot

function Invoke-Docker {
    param([string]$Command)
    $prev = $ErrorActionPreference
    $ErrorActionPreference = "SilentlyContinue"
    cmd /c $Command
    $ec = $LASTEXITCODE
    $ErrorActionPreference = $prev
    if ($ec -ne 0) { throw "Command failed (exit $ec): $Command" }
}

try {
    $compose = "docker compose -f docker/docker-compose.yml"

    # --- Check ORION_MASTER_KEY ---
    if (-not $env:ORION_MASTER_KEY) {
        Write-Host "`n  WARNING: ORION_MASTER_KEY is not set." -ForegroundColor Yellow
        Write-Host "  The full stack requires this for secrets encryption." -ForegroundColor Yellow
        Write-Host "  Generate one with: openssl rand -base64 32`n" -ForegroundColor Yellow
        $answer = Read-Host "  Continue without ORION_MASTER_KEY? (y/N)"
        if ($answer -ne "y" -and $answer -ne "Y") {
            Write-Host "  Aborted. Set ORION_MASTER_KEY and retry.`n" -ForegroundColor Red
            return
        }
    }

    # --- Tear down ---
    if ($Down) {
        Write-Host "`n  Tearing down Orion stack...`n" -ForegroundColor Yellow
        Invoke-Docker "$compose --profile full down"
        Write-Host "`n  Stack is down.`n" -ForegroundColor Green
        return
    }

    # --- Build ---
    Write-Host "`n  Building Orion Docker images...`n" -ForegroundColor Cyan
    if ($Rebuild) {
        Invoke-Docker "$compose --profile full build --no-cache"
    } else {
        Invoke-Docker "$compose --profile full build"
    }

    # --- Start ---
    Write-Host "`n  Starting full stack (postgres, ollama, orion-api, frontend)...`n" -ForegroundColor Cyan
    Invoke-Docker "$compose --profile full up -d"

    # --- Wait for API health (via docker inspect, port not published) ---
    Write-Host "  Waiting for API to become healthy..." -ForegroundColor Gray
    $maxWait = 60
    $ready = $false
    for ($i = 1; $i -le $maxWait; $i++) {
        try {
            $status = docker inspect --format='{{.State.Health.Status}}' (docker compose -f docker/docker-compose.yml ps -q orion-api) 2>$null
            if ($status -eq "healthy") { $ready = $true; break }
        } catch {}
        Start-Sleep -Seconds 1
    }
    if (-not $ready) {
        Write-Host "`n  WARNING: API did not become healthy within ${maxWait}s. Check 'docker compose logs orion-api'.`n" -ForegroundColor Yellow
    }

    # --- Ensure birth model is available in Ollama ---
    $birthModel = if ($env:BIRTH_MODEL) { $env:BIRTH_MODEL } else { "qwen2.5:3b-instruct" }
    Write-Host "  Ensuring Ollama has birth model ($birthModel)..." -ForegroundColor Gray
    try {
        $models = docker compose -f docker/docker-compose.yml exec -T ollama ollama list 2>&1
        if ($models -notmatch [regex]::Escape($birthModel)) {
            Write-Host "  Pulling $birthModel (first run, may take a few minutes)..." -ForegroundColor Yellow
            Invoke-Docker "$compose exec -T ollama ollama pull $birthModel"
            Write-Host "  Model $birthModel ready." -ForegroundColor Green
        } else {
            Write-Host "  Model $birthModel already available." -ForegroundColor Green
        }
    } catch {
        Write-Host "  WARNING: Could not pull birth model. Chat will fail until model is available." -ForegroundColor Yellow
    }

    # --- Wait for frontend ---
    Write-Host "  Waiting for frontend..." -ForegroundColor Gray
    $feReady = $false
    for ($i = 1; $i -le 30; $i++) {
        try {
            $r = Invoke-WebRequest -Uri "http://localhost:3000" -UseBasicParsing -TimeoutSec 2 -ErrorAction SilentlyContinue
            if ($r.StatusCode -eq 200) { $feReady = $true; break }
        } catch {}
        Start-Sleep -Seconds 1
    }

    # --- Summary ---
    Write-Host ""
    Write-Host "  ============================================" -ForegroundColor Green
    Write-Host "   Orion stack is running!" -ForegroundColor Green
    Write-Host "  ============================================" -ForegroundColor Green
    Write-Host ""
    Write-Host "   Web UI:    http://localhost:3000" -ForegroundColor White
    Write-Host "   Health:    http://localhost:3000/health (via nginx)" -ForegroundColor White
    Write-Host "   Postgres:  127.0.0.1:5432  (orion / orion_dev)" -ForegroundColor DarkGray
    Write-Host "   Ollama:    127.0.0.1:11434" -ForegroundColor DarkGray
    Write-Host ""
    Write-Host "   Tear down: .\scripts\dev-stack.ps1 -Down" -ForegroundColor DarkGray
    Write-Host "   Logs:      docker compose -f docker/docker-compose.yml logs -f orion-api" -ForegroundColor DarkGray
    Write-Host ""

    # Open browser
    if ($feReady) {
        Start-Process "http://localhost:3000"
    }
}
finally {
    Pop-Location
}
