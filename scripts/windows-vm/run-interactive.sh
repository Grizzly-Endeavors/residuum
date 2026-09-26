#!/usr/bin/env bash
# Run a command in the Windows VM's logged-on desktop session, where toasts
# and windows actually appear, and optionally screenshot the desktop.
#
#   run-interactive.sh [--screenshot FILE] [--detach] [--delay SECS] [--timeout SECS] [--no-sync] -- COMMAND...
#
#   # Show an urgent toast, then capture the screen 5s later:
#   run-interactive.sh --screenshot /tmp/toast.png --detach --delay 5 -- cargo run -- serve --foreground
#   # Screenshot only:
#   run-interactive.sh --screenshot /tmp/desktop.png
#
# Without --detach it waits for the command to exit and returns its exit
# code. With --detach it returns after --delay seconds and leaves the command
# running. See docs/runbooks/windows-harness.md.
set -euo pipefail

# shellcheck source=scripts/windows-vm/common.sh
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"

screenshot=""
detach=0
delay=10
timeout=1800
sync=1
while [ "$#" -gt 0 ]; do
    case "$1" in
        --screenshot) screenshot="${2:?--screenshot needs a file}"; shift 2 ;;
        --detach) detach=1; shift ;;
        --delay) delay="${2:?--delay needs seconds}"; shift 2 ;;
        --timeout) timeout="${2:?--timeout needs seconds}"; shift 2 ;;
        --no-sync) sync=0; shift ;;
        --) shift; break ;;
        *) die "unknown option '$1' (see the usage at the top of this script)" ;;
    esac
done
command="$*"
[ -n "$command" ] || [ -n "$screenshot" ] || die "nothing to do: give a COMMAND, --screenshot, or both"

require_ssh
if [ -n "$command" ] && [ "$sync" = 1 ]; then
    info "syncing working tree"
    sync_tree
fi

guest_copy_to "$HARNESS_DIR/guest/session-run.ps1" "$GUEST_ROOT_FWD/session-run.ps1"

args="-DelaySeconds $delay -TimeoutSeconds $timeout"
if [ -n "$command" ]; then
    args="$args -CommandBase64 '$(printf '%s' "$command" | base64 -w0)'"
fi
[ "$detach" = 1 ] && args="$args -Detach"
guest_shot="$GUEST_ROOT\\interactive\\screenshot.png"
[ -n "$screenshot" ] && args="$args -Screenshot '$guest_shot'"

status=0
guest_ps "& '$GUEST_ROOT\\session-run.ps1' $args; exit \$LASTEXITCODE" || status=$?

if [ -n "$screenshot" ]; then
    guest_copy_from "$GUEST_ROOT_FWD/interactive/screenshot.png" "$screenshot"
    info "screenshot saved to $screenshot"
fi
exit "$status"
