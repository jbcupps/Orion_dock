#!/usr/bin/env bash
# Check docs/index.html: link format, OS detection script, required elements.
set -euo pipefail
REPO_ROOT="${1:-.}"
HTML="${REPO_ROOT}/docs/index.html"
if [[ ! -f "$HTML" ]]; then
  echo "Missing $HTML"
  exit 1
fi

fail=0

# Required element ids and structure
if ! grep -q 'id="main-download"' "$HTML"; then
  echo "FAIL: main-download link id missing"
  fail=1
fi

# Other downloads section with expected base URL
if ! grep -q 'github.com/jbcupps/abigail/releases' "$HTML"; then
  echo "FAIL: expected download base URL not found"
  fail=1
fi

# OS detection script branches (windows, macos, linux)
if ! grep -q "os = 'windows'" "$HTML" || ! grep -q "os = 'macos'" "$HTML" || ! grep -q "os = 'linux'" "$HTML"; then
  echo "FAIL: OS detection branches (windows/macos/linux) missing"
  fail=1
fi

# Link format: .exe, .dmg, .deb for the three platforms
if ! grep -q 'Abigail-windows-x64-setup.exe' "$HTML"; then
  echo "FAIL: Windows .exe link missing"
  fail=1
fi
if ! grep -q 'Abigail-macos-x64.dmg' "$HTML"; then
  echo "FAIL: macOS .dmg link missing"
  fail=1
fi
if ! grep -q 'Abigail-linux-x64.deb' "$HTML"; then
  echo "FAIL: Linux .deb link missing"
  fail=1
fi

if [[ $fail -eq 0 ]]; then
  echo "check-docs-html: OK"
fi
exit $fail
