#!/usr/bin/env bash
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

cloc . --exclude-dir=target,node_modules,.git --not-match-f='\.md$' "$@"
