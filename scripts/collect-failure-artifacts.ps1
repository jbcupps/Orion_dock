# Write triage artifacts after a failed run. Call from CI or locally after a failure.
# Usage: .\scripts\collect-failure-artifacts.ps1 [output_dir]
param([string]$OutputDir = ".\artifacts")
$ErrorActionPreference = "Stop"
$RepoRoot = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
Set-Location $RepoRoot
Write-Host "Collecting failure artifacts into $OutputDir"
if ($env:UAT_LOG_CAPTURE -and (Test-Path $env:UAT_LOG_CAPTURE)) {
    Copy-Item $env:UAT_LOG_CAPTURE (Join-Path $OutputDir "uat-failure.log") -Force
}
Get-ChildItem Env: | Sort-Object Name | Out-File (Join-Path $OutputDir "env.txt") -Encoding utf8
Write-Host "Done. Inspect $OutputDir for triage."
