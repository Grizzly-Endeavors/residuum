#!/usr/bin/env bash
# Line-of-code summary for this repo
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

cloc . --exclude-dir=target,node_modules,.git --not-match-f='\.md$' "$@"
