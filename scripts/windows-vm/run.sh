#!/usr/bin/env bash
set -euo pipefail

# shellcheck source=scripts/windows-vm/common.sh
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"

[ "$#" -gt 0 ] || die "usage: run.sh COMMAND [ARGS...]"

require_ssh
if [ -z "${RESIDUUM_WINVM_NO_SYNC:-}" ]; then
    info "syncing working tree"
    sync_tree
fi

# When the command runs a native program, its exit code is the result:
# Windows PowerShell marks a native command failed whenever its stderr is
# redirected (cargo ... 2>&1), even on exit 0. Otherwise the result is whether
# the PowerShell command succeeded.
guest_ps "
    Set-Location '$GUEST_ROOT\\src'
    \$env:CARGO_TARGET_DIR = '$GUEST_ROOT\\target'
    \$ErrorActionPreference = 'Continue'
    \$global:LASTEXITCODE = \$null
    $*
    \$succeeded = \$?
    if (\$null -ne \$LASTEXITCODE) { exit \$LASTEXITCODE }
    if (-not \$succeeded) { exit 1 }
"
