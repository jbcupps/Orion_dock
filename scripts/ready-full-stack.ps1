# Wait for Postgres and Ollama to be ready, then pull the birth model.
# Use with Docker Compose profile "full". Set $env:DATABASE_URL and $env:LOCAL_LLM_BASE_URL if needed.

$ErrorActionPreference = "Stop"
$PostgresUrl = if ($env:DATABASE_URL) { $env:DATABASE_URL } else { "postgres://orion:orion_dev@localhost:5432/orion" }
$OllamaUrl = if ($env:LOCAL_LLM_BASE_URL) { $env:LOCAL_LLM_BASE_URL } else { "http://localhost:11434" }
$BirthModel = if ($env:BIRTH_MODEL) { $env:BIRTH_MODEL } else { "qwen2.5:3b-instruct" }
$MaxAttempts = 30
$SleepSeconds = 2

# Parse host and port from DATABASE_URL (simple: assume format ...@host:port/...)
if ($PostgresUrl -match "@([^:]+):(\d+)/") {
    $PgHost = $Matches[1]
    $PgPort = [int]$Matches[2]
} else {
    $PgHost = "localhost"
    $PgPort = 5432
}

Write-Host "Waiting for Postgres at $PgHost`:$PgPort ..."
$pgReady = $false
for ($i = 1; $i -le $MaxAttempts; $i++) {
    try {
        $tcp = New-Object System.Net.Sockets.TcpClient
        $tcp.Connect($PgHost, $PgPort)
        $tcp.Close()
        $pgReady = $true
        Write-Host "Postgres is ready."
        break
    } catch {}
    Start-Sleep -Seconds $SleepSeconds
}
if (-not $pgReady) {
    Write-Error "Postgres did not become ready in time."
    exit 1
}

Write-Host "Waiting for Ollama at $OllamaUrl ..."
$ollamaReady = $false
for ($i = 1; $i -le $MaxAttempts; $i++) {
    try {
        $r = Invoke-WebRequest -Uri "$OllamaUrl/api/tags" -UseBasicParsing -TimeoutSec 2 -ErrorAction SilentlyContinue
        if ($r.StatusCode -eq 200) { $ollamaReady = $true; Write-Host "Ollama is ready."; break }
    } catch {}
    Start-Sleep -Seconds $SleepSeconds
}
if (-not $ollamaReady) {
    Write-Error "Ollama did not become ready in time."
    exit 1
}

Write-Host "Pulling birth model: $BirthModel"
$ollama = Get-Command ollama -ErrorAction SilentlyContinue
if ($ollama) {
    & ollama pull $BirthModel
    Write-Host "Done. Postgres and Ollama are ready; birth model pulled."
} else {
    Write-Host "ollama CLI not found; pull manually: ollama pull $BirthModel"
}
