param()

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

function Invoke-DockerAllowFailure {
    param([string]$Command)
    $prev = $ErrorActionPreference
    $ErrorActionPreference = "SilentlyContinue"
    cmd /c $Command
    $ec = $LASTEXITCODE
    $ErrorActionPreference = $prev
    return $ec
}

function Write-Pass {
    param([string]$Message)
    Write-Host "[PASS] $Message" -ForegroundColor Green
}

function Fail {
    param([string]$Message)
    throw "[FAIL] $Message"
}

try {
    $compose = "docker compose -f docker/docker-compose.yml --profile full"

    Write-Host "Bringing up proxy test dependencies (proxy_external, proxy_internal, ollama, nettest)..." -ForegroundColor Cyan
    Invoke-Docker "$compose up -d proxy_external proxy_internal ollama nettest"

    Write-Host "Waiting for Ollama internal endpoint..." -ForegroundColor Gray
    $ollamaReady = $false
    for ($i = 1; $i -le 45; $i++) {
        $ec = Invoke-DockerAllowFailure "$compose exec -T nettest sh -lc ""curl -fsS --connect-timeout 2 --max-time 5 http://ollama:11434/api/tags -o /dev/null"""
        if ($ec -eq 0) {
            $ollamaReady = $true
            break
        }
        Start-Sleep -Seconds 2
    }
    if (-not $ollamaReady) { Fail "Ollama did not become reachable from nettest." }

    # 1) Direct egress should fail when bypassing proxy env.
    $directEgress = Invoke-DockerAllowFailure "$compose exec -T nettest sh -lc ""unset HTTP_PROXY HTTPS_PROXY http_proxy https_proxy ALL_PROXY all_proxy; curl -fsS --connect-timeout 8 --max-time 12 --noproxy '*' https://example.com -o /dev/null"""
    if ($directEgress -eq 0) {
        Fail "Direct egress unexpectedly succeeded."
    } else {
        Write-Pass "Direct egress is blocked."
    }

    # 2) Proxied egress should work with container proxy env.
    Invoke-Docker "$compose exec -T nettest sh -lc ""curl -fsS --connect-timeout 8 --max-time 20 https://example.com -o /dev/null"""
    Write-Pass "Proxied egress works."

    # 3) SSRF/metadata target must be blocked by external proxy policy.
    $metadata = Invoke-DockerAllowFailure "$compose exec -T nettest sh -lc ""curl -fsS --connect-timeout 8 --max-time 12 http://169.254.169.254 -o /dev/null"""
    if ($metadata -eq 0) {
        Fail "Metadata endpoint unexpectedly reachable."
    } else {
        Write-Pass "Metadata endpoint is blocked."
    }

    # 4) Internal service call should bypass proxy via NO_PROXY and succeed.
    Invoke-Docker "$compose exec -T nettest sh -lc ""curl -fsS --connect-timeout 8 --max-time 12 http://ollama:11434/api/tags -o /dev/null"""
    Write-Pass "Internal NO_PROXY bypass works (ollama reachable)."

    Write-Host "All proxy smoke tests passed." -ForegroundColor Green
}
finally {
    Pop-Location
}
