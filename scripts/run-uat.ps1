# run-uat.ps1 — Full automated UAT: build, lint, test, start stack, run probes, tear down.
#
# Usage:
#   .\scripts\run-uat.ps1            # Run everything (fast tests + full-stack probes)
#   .\scripts\run-uat.ps1 -Fast      # Fast only: fmt, clippy, build, unit tests, frontend
#   .\scripts\run-uat.ps1 -KeepStack # Don't tear down after probes (leave stack for inspection)
#
# Exit code 0 = all tests passed.

param(
    [switch]$Fast,
    [switch]$KeepStack
)

$ErrorActionPreference = "Stop"
$RepoRoot = (Get-Item $PSScriptRoot).Parent.FullName
Push-Location $RepoRoot

$startTime = Get-Date
$results = @()

function Invoke-Docker {
    param([string]$Command)
    $prev = $ErrorActionPreference
    $ErrorActionPreference = "SilentlyContinue"
    cmd /c $Command
    $ec = $LASTEXITCODE
    $ErrorActionPreference = $prev
    if ($ec -ne 0) { throw "Command failed (exit $ec): $Command" }
}

function Run-Phase {
    param([string]$Name, [scriptblock]$Action)
    $phaseStart = Get-Date
    Write-Host "`n=== $Name ===" -ForegroundColor Cyan
    try {
        & $Action
        $elapsed = ((Get-Date) - $phaseStart).TotalSeconds
        $script:results += [PSCustomObject]@{ Phase=$Name; Status="PASS"; Time="${elapsed:N1}s" }
        Write-Host "  $Name PASSED (${elapsed:N1}s)" -ForegroundColor Green
    } catch {
        $elapsed = ((Get-Date) - $phaseStart).TotalSeconds
        $script:results += [PSCustomObject]@{ Phase=$Name; Status="FAIL"; Time="${elapsed:N1}s" }
        Write-Host "  $Name FAILED: $_" -ForegroundColor Red
        throw
    }
}

$compose = "docker compose -f docker/docker-compose.yml"

try {
    # ---- Phase 1: Fast CI (always runs) ----
    Run-Phase "Build + Lint + Unit Tests + Frontend" {
        Invoke-Docker "$compose run --rm -e UAT_MODE=fast orion-build"
    }

    if ($Fast) {
        Write-Host "`n  -Fast flag set; skipping full-stack probes.`n" -ForegroundColor Yellow
        return
    }

    # ---- Phase 2: Build full-stack images ----
    Run-Phase "Build Full-Stack Images" {
        Invoke-Docker "$compose --profile full build"
    }

    # ---- Phase 3: Start full stack ----
    Run-Phase "Start Full Stack" {
        Invoke-Docker "$compose --profile full up -d"
    }

    # ---- Phase 4: Wait for services ----
    Run-Phase "Service Readiness" {
        Write-Host "  Waiting for API..." -ForegroundColor Gray
        $ready = $false
        for ($i = 1; $i -le 90; $i++) {
            try {
                $r = Invoke-WebRequest -Uri "http://localhost:8080/health" -UseBasicParsing -TimeoutSec 2 -ErrorAction SilentlyContinue
                if ($r.StatusCode -eq 200) { $ready = $true; break }
            } catch {}
            Start-Sleep -Seconds 1
        }
        if (-not $ready) { throw "API did not become healthy within 90s" }
        Write-Host "  API is healthy." -ForegroundColor Gray
    }

    # ---- Phase 5: UAT Probes ----
    Run-Phase "UAT Probes (14 endpoint tests)" {
        Invoke-Docker "$compose exec orion-dev bash -c `"ORION_API_URL=http://orion-api:8080 FRONTEND_URL=http://frontend:80 bash /app/scripts/uat-probes.sh`""
    }

    # ---- Phase 6: Postgres integration tests ----
    Run-Phase "Postgres Integration Tests" {
        Invoke-Docker "$compose exec orion-dev bash -c `"DATABASE_URL=postgres://orion:orion_dev@postgres:5432/orion cargo test -p orion-memory --no-fail-fast --features postgres`""
    }
}
catch {
    Write-Host "`n  UAT FAILED`n" -ForegroundColor Red
}
finally {
    # ---- Tear down (unless -KeepStack) ----
    if (-not $KeepStack -and -not $Fast) {
        Write-Host "`n=== Tearing down stack ===" -ForegroundColor Gray
        try { Invoke-Docker "$compose --profile full down" } catch {}
    } elseif ($KeepStack) {
        Write-Host "`n  Stack left running (-KeepStack). Tear down with: .\scripts\dev-stack.ps1 -Down`n" -ForegroundColor Yellow
    }

    # ---- Summary ----
    $totalElapsed = ((Get-Date) - $startTime).TotalSeconds
    Write-Host ""
    Write-Host "  ============================================" -ForegroundColor Cyan
    Write-Host "   UAT Results" -ForegroundColor Cyan
    Write-Host "  ============================================" -ForegroundColor Cyan
    Write-Host ""
    foreach ($r in $results) {
        $color = if ($r.Status -eq "PASS") { "Green" } else { "Red" }
        $mark = if ($r.Status -eq "PASS") { "[PASS]" } else { "[FAIL]" }
        Write-Host "   $mark $($r.Phase)  ($($r.Time))" -ForegroundColor $color
    }
    Write-Host ""
    $failed = ($results | Where-Object { $_.Status -eq "FAIL" }).Count
    if ($failed -eq 0 -and $results.Count -gt 0) {
        Write-Host "   All phases passed in ${totalElapsed:N0}s" -ForegroundColor Green
    } elseif ($failed -gt 0) {
        Write-Host "   $failed phase(s) failed" -ForegroundColor Red
    }
    Write-Host ""
    Pop-Location
}
