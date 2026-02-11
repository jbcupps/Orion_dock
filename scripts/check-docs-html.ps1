# Check docs/index.html: link format, OS detection script, required elements.
param([string]$RepoRoot = ".")
$HTML = Join-Path $RepoRoot "docs\index.html"
if (-not (Test-Path $HTML)) {
    Write-Error "Missing $HTML"
    exit 1
}
$content = Get-Content $HTML -Raw
$fail = 0

if ($content -notmatch 'id="main-download"') {
    Write-Host "FAIL: main-download link id missing"
    $fail = 1
}
if ($content -notmatch 'github\.com/jbcupps/abigail/releases') {
    Write-Host "FAIL: expected download base URL not found"
    $fail = 1
}
if ($content -notmatch "os = 'windows'" -or $content -notmatch "os = 'macos'" -or $content -notmatch "os = 'linux'") {
    Write-Host "FAIL: OS detection branches (windows/macos/linux) missing"
    $fail = 1
}
if ($content -notmatch 'Abigail-windows-x64-setup\.exe') {
    Write-Host "FAIL: Windows .exe link missing"
    $fail = 1
}
if ($content -notmatch 'Abigail-macos-x64\.dmg') {
    Write-Host "FAIL: macOS .dmg link missing"
    $fail = 1
}
if ($content -notmatch 'Abigail-linux-x64\.deb') {
    Write-Host "FAIL: Linux .deb link missing"
    $fail = 1
}

if ($fail -eq 0) {
    Write-Host "check-docs-html: OK"
}
exit $fail
