# full-stack.ps1 — Comprehensive Orion startup (full stack, optional email profile).
#
# Usage:
#   .\scripts\full-stack.ps1                      # Build + start (profiles: full,email), open browser
#   .\scripts\full-stack.ps1 -NoEmail             # Build + start without email profile
#   .\scripts\full-stack.ps1 -SkipBuild           # Start without rebuilding images
#   .\scripts\full-stack.ps1 -Rebuild             # Force no-cache rebuild before start
#   .\scripts\full-stack.ps1 -Down                # Tear down stack
#   .\scripts\full-stack.ps1 -Down -PruneVolumes  # Tear down + remove named volumes

param(
    [switch]$Down,
    [switch]$Rebuild,
    [switch]$SkipBuild,
    [switch]$NoEmail,
    [switch]$NoBrowser,
    [switch]$PruneVolumes,
    [int]$WaitSeconds = 90
)

$ErrorActionPreference = "Stop"
$RepoRoot = (Get-Item $PSScriptRoot).Parent.FullName
Push-Location $RepoRoot

$ProfileArgs = @("--profile full")
if (-not $NoEmail) { $ProfileArgs += "--profile email" }
$Profiles = $ProfileArgs -join " "
$Compose = "docker compose -f docker/docker-compose.yml $Profiles"

function Invoke-Docker {
    param([string]$Command)
    $prev = $ErrorActionPreference
    $ErrorActionPreference = "SilentlyContinue"
    cmd /c $Command
    $ec = $LASTEXITCODE
    $ErrorActionPreference = $prev
    if ($ec -ne 0) { throw "Command failed (exit $ec): $Command" }
}

function Get-ServiceHealth {
    param([string]$Service)
    try {
        $id = cmd /c "docker compose -f docker/docker-compose.yml $Profiles ps -q $Service" 2>$null
        if (-not $id) { return "unknown" }
        $status = cmd /c "docker inspect --format={{.State.Health.Status}} $id" 2>$null
        if ($status) { return $status.Trim() }
        return "running"
    } catch {
        return "unknown"
    }
}

function Wait-Healthy {
    param(
        [string[]]$Services,
        [int]$TimeoutSeconds = 90
    )
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        $allReady = $true
        foreach ($svc in $Services) {
            $h = Get-ServiceHealth -Service $svc
            if ($h -ne "healthy" -and $h -ne "running") {
                $allReady = $false
                break
            }
        }
        if ($allReady) { return $true }
        Start-Sleep -Seconds 1
    }
    return $false
}

try {
    # --- Ensure ORION_MASTER_KEY ---
    $keyFile = Join-Path $RepoRoot ".orion-master-key"
    if (-not $env:ORION_MASTER_KEY) {
        if (Test-Path $keyFile) {
            $env:ORION_MASTER_KEY = (Get-Content $keyFile -Raw).Trim()
        } else {
            $bytes = New-Object byte[] 32
            [System.Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($bytes)
            $key = [Convert]::ToBase64String($bytes)
            $key | Set-Content $keyFile -NoNewline
            $env:ORION_MASTER_KEY = $key
            Write-Host ""
            Write-Host "  ================================================================" -ForegroundColor Cyan
            Write-Host "   ORION_MASTER_KEY generated (first run). Copy and store securely." -ForegroundColor Cyan
            Write-Host "  ================================================================" -ForegroundColor Cyan
            Write-Host ""
            Write-Host "   $key" -ForegroundColor White
            Write-Host ""
            Write-Host "   Saved to: $keyFile" -ForegroundColor Gray
            Write-Host "   You need this key to decrypt secrets. Back it up safely.`n" -ForegroundColor Gray
        }
    }

    if ($Down) {
        Write-Host "`n  Tearing down Orion stack ($Profiles)...`n" -ForegroundColor Yellow
        Invoke-Docker "$Compose down"
        if ($PruneVolumes) {
            Write-Host "  Removing named volumes..." -ForegroundColor Gray
            Invoke-Docker "docker volume rm docker_orion-data docker_orion-pgdata docker_ollama-models docker_orion-cargo-cache"
        }
        Write-Host "`n  Stack is down.`n" -ForegroundColor Green
        return
    }

    if (-not $SkipBuild) {
        Write-Host "`n  Building Orion Docker images ($Profiles)...`n" -ForegroundColor Cyan
        if ($Rebuild) {
            Invoke-Docker "$Compose build --no-cache"
        } else {
            Invoke-Docker "$Compose build"
        }
    } else {
        Write-Host "`n  Skipping image build (-SkipBuild).`n" -ForegroundColor DarkGray
    }

    Write-Host "`n  Starting comprehensive Orion stack ($Profiles)...`n" -ForegroundColor Cyan
    Invoke-Docker "$Compose up -d"

    $healthTargets = @("proxy_external", "proxy_internal", "orion-toolbox", "ollama", "orion-api")
    Write-Host "  Waiting for core services to become healthy..." -ForegroundColor Gray
    $coreReady = Wait-Healthy -Services $healthTargets -TimeoutSeconds $WaitSeconds
    if (-not $coreReady) {
        Write-Host "  WARNING: Some core services are not healthy yet after ${WaitSeconds}s." -ForegroundColor Yellow
    }

    $birthModel = if ($env:BIRTH_MODEL) { $env:BIRTH_MODEL } else { "qwen2.5:3b-instruct" }
    Write-Host "  Ensuring Ollama has birth model ($birthModel)..." -ForegroundColor Gray
    try {
        $models = cmd /c "docker compose -f docker/docker-compose.yml $Profiles exec -T ollama ollama list" 2>$null
        if ($models -notmatch [regex]::Escape($birthModel)) {
            Write-Host "  Pulling $birthModel (first run may take several minutes)..." -ForegroundColor Yellow
            Invoke-Docker "$Compose exec -T ollama ollama pull $birthModel"
            Write-Host "  Model $birthModel ready." -ForegroundColor Green
        } else {
            Write-Host "  Model $birthModel already available." -ForegroundColor Green
        }
    } catch {
        Write-Host "  WARNING: Could not verify/pull birth model. API chat may fail until model exists." -ForegroundColor Yellow
    }

    Write-Host "  Waiting for frontend..." -ForegroundColor Gray
    $feReady = $false
    for ($i = 1; $i -le 45; $i++) {
        try {
            $r = Invoke-WebRequest -Uri "http://localhost:3000" -UseBasicParsing -TimeoutSec 2 -ErrorAction SilentlyContinue
            if ($r.StatusCode -eq 200) { $feReady = $true; break }
        } catch {}
        Start-Sleep -Seconds 1
    }

    $apiHealthy = (Get-ServiceHealth -Service "orion-api")
    $proxyInternalHealthy = (Get-ServiceHealth -Service "proxy_internal")
    $proxyExternalHealthy = (Get-ServiceHealth -Service "proxy_external")

    Write-Host ""
    Write-Host "  ===============================================================" -ForegroundColor Green
    Write-Host "   Orion comprehensive startup complete" -ForegroundColor Green
    Write-Host "  ===============================================================" -ForegroundColor Green
    Write-Host ""
    Write-Host "   Profiles:      $Profiles" -ForegroundColor White
    Write-Host "   Web UI:        http://localhost:3000" -ForegroundColor White
    Write-Host "   Health:        http://localhost:3000/health" -ForegroundColor White
    Write-Host "   Postgres:      127.0.0.1:5432 (orion / orion_dev)" -ForegroundColor DarkGray
    Write-Host "   Ollama:        127.0.0.1:11434" -ForegroundColor DarkGray
    Write-Host "   API health:    $apiHealthy" -ForegroundColor DarkGray
    Write-Host "   Proxy health:  internal=$proxyInternalHealthy external=$proxyExternalHealthy" -ForegroundColor DarkGray
    if (-not $NoEmail) {
        Write-Host "   Proton Bridge: protonbridge_ingress (IMAP 1143, SMTP 1025 from containers)" -ForegroundColor DarkGray
    }
    Write-Host ""
    Write-Host "   Compose ps:    docker compose -f docker/docker-compose.yml $Profiles ps" -ForegroundColor DarkGray
    Write-Host "   API logs:      docker compose -f docker/docker-compose.yml $Profiles logs -f orion-api" -ForegroundColor DarkGray
    Write-Host "   Tear down:     .\scripts\full-stack.ps1 -Down" -ForegroundColor DarkGray
    Write-Host ""

    if ($feReady -and -not $NoBrowser) {
        Start-Process "http://localhost:3000"
    }
}
finally {
    Pop-Location
}
