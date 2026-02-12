# dev-stack.ps1 — Build and launch the full Orion stack for manual testing.
#
# Usage:
#   .\scripts\dev-stack.ps1          # Build images, start stack, open browser
#   .\scripts\dev-stack.ps1 -Down    # Tear down the stack
#   .\scripts\dev-stack.ps1 -Rebuild # Force rebuild images before starting
#
# Once running:
#   Web UI:  http://localhost:3000
#   API:     http://localhost:8080
#   Postgres: localhost:5432  (orion / orion_dev)
#   Ollama:   localhost:11434

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

    # --- Wait for API health ---
    Write-Host "  Waiting for API to become healthy..." -ForegroundColor Gray
    $maxWait = 60
    $ready = $false
    for ($i = 1; $i -le $maxWait; $i++) {
        try {
            $r = Invoke-WebRequest -Uri "http://localhost:8080/health" -UseBasicParsing -TimeoutSec 2 -ErrorAction SilentlyContinue
            if ($r.StatusCode -eq 200) { $ready = $true; break }
        } catch {}
        Start-Sleep -Seconds 1
    }
    if (-not $ready) {
        Write-Host "`n  WARNING: API did not respond within ${maxWait}s. Check 'docker compose logs orion-api'.`n" -ForegroundColor Yellow
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
    Write-Host "   API:       http://localhost:8080" -ForegroundColor White
    Write-Host "   Health:    http://localhost:8080/health" -ForegroundColor White
    Write-Host "   Postgres:  localhost:5432  (orion / orion_dev)" -ForegroundColor DarkGray
    Write-Host "   Ollama:    localhost:11434" -ForegroundColor DarkGray
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
