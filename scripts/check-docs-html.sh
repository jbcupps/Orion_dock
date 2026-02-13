#!/usr/bin/env bash
# Check docs/index.html: repo link, title, required elements for Orion Dock.
set -euo pipefail
REPO_ROOT="${1:-.}"
HTML="${REPO_ROOT}/docs/index.html"
if [[ ! -f "$HTML" ]]; then
  echo "Missing $HTML"
  exit 1
fi

fail=0

# Orion Dock title
if ! grep -q 'Orion Dock' "$HTML"; then
  echo "FAIL: Orion Dock title/heading missing"
  fail=1
fi

# Repo link
if ! grep -q 'github.com/jbcupps/Orion_dock' "$HTML"; then
  echo "FAIL: expected repo URL (Orion_dock) not found"
  fail=1
fi

# README or getting started link
if ! grep -q 'readme\|HOW_TO_RUN\|Getting started' "$HTML"; then
  echo "FAIL: README or getting started link missing"
  fail=1
fi

if [[ $fail -eq 0 ]]; then
  echo "check-docs-html: OK"
fi
exit $fail
