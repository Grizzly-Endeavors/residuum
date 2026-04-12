#!/usr/bin/env sh
# Enforces CSS motion conventions:
#   1. `transition: all` is banned outright (stock SaaS tempo).
#   2. Raw duration literals (e.g. `0.3s ease`) in transitions must use
#      `--dur-*` / `--ease-out-*` tokens instead. Animations are exempt.

set -e

FOUND=$(grep -rnE 'transition:[[:space:]]*all|[0-9]+(\.[0-9]+)?s[[:space:]]+(ease|linear|ease-in|ease-out|ease-in-out)' src/styles/ \
  | grep -vE 'animation[-:]' || true)

if [ -n "$FOUND" ]; then
  echo "lint-css: raw durations or banned patterns found — use --dur-*/--ease-out-* tokens:" >&2
  echo "$FOUND" >&2
  exit 1
fi
