# Check docs/index.html: repo link, title, required elements for Orion Dock.
param([string]$RepoRoot = ".")
$HTML = Join-Path $RepoRoot "docs\index.html"
if (-not (Test-Path $HTML)) {
    Write-Error "Missing $HTML"
    exit 1
}
$content = Get-Content $HTML -Raw
$fail = 0

if ($content -notmatch 'Orion Dock') {
    Write-Host "FAIL: Orion Dock title/heading missing"
    $fail = 1
}
if ($content -notmatch 'github\.com/jbcupps/Orion_dock') {
    Write-Host "FAIL: expected repo URL (Orion_dock) not found"
    $fail = 1
}
if ($content -notmatch 'readme|HOW_TO_RUN|Getting started') {
    Write-Host "FAIL: README or getting started link missing"
    $fail = 1
}

if ($fail -eq 0) {
    Write-Host "check-docs-html: OK"
}
exit $fail
