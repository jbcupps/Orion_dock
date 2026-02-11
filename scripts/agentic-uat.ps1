# Agentic UAT: full automated test from birth to usable operation.
# Requires: $env:TEST_LLM_KEY, $env:TEST_SEARCH_KEY (optional: TEST_LLM_PROVIDER, TEST_SEARCH_PROVIDER, UAT_GENESIS_PATH).
# UAT_GENESIS_PATH: direct (default), soul_forge, soul_crystallization.
# Generates: UAT_REPORT_YYYY-MM-DD.md with sectional checkmarks.
$ErrorActionPreference = "Stop"
$RepoRoot = (Get-Item $PSScriptRoot).Parent.FullName
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

if (-not $env:TEST_LLM_KEY -or -not $env:TEST_SEARCH_KEY) {
    Write-Host "Missing required env: TEST_LLM_KEY and TEST_SEARCH_KEY. Set them and re-run." -ForegroundColor Red
    exit 1
}

$Date = Get-Date -Format "yyyy-MM-dd"
$Report = Join-Path $RepoRoot "UAT_REPORT_$Date.md"
$Log = Join-Path $RepoRoot "uat_run_$Date.log"

Write-Host "=== Agentic UAT: starting full stack ==="
Invoke-Docker "docker compose -f docker/docker-compose.yml --profile full build orion-api frontend"
Invoke-Docker "docker compose -f docker/docker-compose.yml --profile full up -d postgres ollama orion-api frontend"

if (-not $env:DATABASE_URL) { $env:DATABASE_URL = "postgres://orion:orion_dev@localhost:5432/orion" }
if (-not $env:LOCAL_LLM_BASE_URL) { $env:LOCAL_LLM_BASE_URL = "http://localhost:11434" }
$readyScript = Join-Path $RepoRoot "scripts\ready-full-stack.ps1"
if (Test-Path $readyScript) { & $readyScript }

Write-Host "=== Building and running orion-uat ==="
$env:ORION_DATA_DIR = "/var/lib/orion"
$env:DATABASE_URL_CONTAINER = "postgres://orion:orion_dev@postgres:5432/orion"
$env:LOCAL_LLM_CONTAINER = "http://ollama:11434"
$BirthModel = if ($env:BIRTH_MODEL) { $env:BIRTH_MODEL } else { "qwen2.5:3b-instruct" }
$LlmProvider = if ($env:TEST_LLM_PROVIDER) { $env:TEST_LLM_PROVIDER } else { "auto" }
$SearchProvider = if ($env:TEST_SEARCH_PROVIDER) { $env:TEST_SEARCH_PROVIDER } else { "auto" }
$GenesisPath = if ($env:UAT_GENESIS_PATH) { $env:UAT_GENESIS_PATH } else { "direct" }
$Pass = 0

$dockerCmd = "docker compose -f docker/docker-compose.yml --profile full run --rm" +
    " -v orion-data:/var/lib/orion" +
    " -e ORION_DATA_DIR=/var/lib/orion" +
    " -e DATABASE_URL=postgres://orion:orion_dev@postgres:5432/orion" +
    " -e LOCAL_LLM_BASE_URL=http://ollama:11434" +
    " -e MEMORY_BACKEND=postgres" +
    " -e BIRTH_MODEL=$BirthModel" +
    " -e TEST_LLM_KEY=$($env:TEST_LLM_KEY)" +
    " -e TEST_SEARCH_KEY=$($env:TEST_SEARCH_KEY)" +
    " -e TEST_LLM_PROVIDER=$LlmProvider" +
    " -e TEST_SEARCH_PROVIDER=$SearchProvider" +
    " -e UAT_GENESIS_PATH=$GenesisPath" +
    " orion-build bash -c 'cargo build -p orion-uat --release 2>&1 && /app/target/release/orion-uat 2>&1'"

$prev = $ErrorActionPreference
$ErrorActionPreference = "SilentlyContinue"
$runOutput = cmd /c $dockerCmd 2>&1
$ec = $LASTEXITCODE
$ErrorActionPreference = $prev
$runOutput | Set-Content -Path $Log
if ($ec -eq 0) { $Pass = 1 }

$ApiOk = $false
try {
    $null = Invoke-WebRequest -Uri "http://localhost:8080/api/status" -UseBasicParsing -TimeoutSec 5
    $ApiOk = $true
} catch {}

$ApiStatus = if ($ApiOk) { "[OK]" } else { "[X]" }
$header = '| Stage | Status |'
$sep = '|-------|--------|'
$lines = @(
    "# UAT Report - $Date",
    "",
    $header,
    $sep
)
if ($Pass) {
    $lines += '| 1. Darkness (identity generated) | [OK] |'
    $lines += '| 2. Ignition (local LLM) | [OK] |'
    $lines += '| 3. Connectivity (API keys stored) | [OK] |'
    $lines += "| 4. Genesis ($GenesisPath) | [OK] |"
    $lines += '| 5. Emergence (birth complete) | [OK] |'
    $lines += '| 6. Early operation (router check) | [OK] |'
} else {
    $lines += '| 1. Darkness | [X] |'
    $lines += '| 2. Ignition | [X] |'
    $lines += '| 3. Connectivity | [X] |'
    $lines += "| 4. Genesis ($GenesisPath) | [X] |"
    $lines += '| 5. Emergence | [X] |'
    $lines += '| 6. Early operation | [X] |'
}
$lines += "| 7. API status visible | $ApiStatus |"
$lines += ""
$lines += "## Log"
$lines += '```'
if (Test-Path $Log) {
    $lines += Get-Content $Log -Tail 80
}
$lines += '```'
$lines | Set-Content -Path $Report -Encoding UTF8

Write-Host "Report written to $Report"
if (-not $Pass) { exit 1 }
exit 0
