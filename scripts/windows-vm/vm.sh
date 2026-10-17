#!/usr/bin/env bash
# Day-to-day control of the Windows test VM.
#
#   vm.sh start | stop | restart | status
#   vm.sh ssh [powershell...]      interactive shell, or run one command
#   vm.sh provision [-Upgrade]     (re)install the toolchain in the guest
#   vm.sh snapshot NAME | restore NAME | delete-snapshot NAME | snapshots
#
# See docs/runbooks/windows-harness.md.
set -euo pipefail

# shellcheck source=scripts/windows-vm/common.sh
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"

stop_vm() {
    if ! vm_running; then
        info "$VM_NAME is not running"
        return 0
    fi
    info "shutting down $VM_NAME"
    if ssh_ready; then
        guest_ps 'Stop-Computer -Force' || true
    else
        monitor_cmd system_powerdown
    fi
    local waited=0
    while vm_running; do
        if [ "$waited" -ge 180 ]; then
            info "guest did not shut down in 180s; stopping QEMU"
            monitor_cmd quit || kill "$(cat "$QEMU_PID")"
            break
        fi
        sleep 2
        waited=$((waited + 2))
    done
    rm -f "$QEMU_PID"
}

require_stopped() {
    vm_running && die "stop the VM first (vm.sh stop); snapshots need a consistent disk"
    return 0
}

# Whether the disk has a snapshot named $1. qemu-img lists two header lines,
# then one snapshot per line with its name in the second column.
snapshot_exists() {
    qemu-img snapshot -l "$DISK" | awk -v name="$1" 'NR > 2 && $2 == name { found = 1 } END { exit !found }'
}

cmd="${1:-status}"
shift || true

case "$cmd" in
    start)
        require_vm
        start_vm
        info "waiting for SSH"
        wait_for_ssh 600
        info "ready"
        ;;
    stop)
        stop_vm
        ;;
    restart)
        require_vm
        stop_vm
        start_vm
        wait_for_ssh 600
        info "ready"
        ;;
    status)
        require_vm
        if vm_running; then
            if ssh_ready; then state="running, SSH reachable"; else state="running, SSH not reachable yet"; fi
        else
            state="stopped"
        fi
        echo "$VM_NAME: $state"
        echo "  directory: $VM_DIR"
        echo "  ssh:       127.0.0.1:$SSH_PORT (user $GUEST_USER, key $SSH_KEY)"
        echo "  vnc:       127.0.0.1:$((5900 + VNC_DISPLAY))"
        ;;
    ssh)
        require_ssh
        mapfile -t opts < <(ssh_opts)
        exec ssh "${opts[@]}" "$GUEST_USER@127.0.0.1" "$@"
        ;;
    provision)
        require_ssh
        # Install the compiler the git hooks and CI use, not whatever stable is.
        rust_toolchain="$(sed -n 's/^channel *= *"\(.*\)"$/\1/p' "$REPO_ROOT/rust-toolchain.toml")"
        [[ "$rust_toolchain" =~ ^[0-9A-Za-z._-]+$ ]] || die "no usable channel in $REPO_ROOT/rust-toolchain.toml"
        guest_ps "New-Item -ItemType Directory -Force '$GUEST_ROOT' | Out-Null"
        guest_copy_to "$HARNESS_DIR/guest/provision.ps1" "$GUEST_ROOT_FWD/provision.ps1"
        guest_ps "& '$GUEST_ROOT\\provision.ps1' -RustToolchain '$rust_toolchain' $*"
        ;;
    snapshot)
        require_vm
        require_stopped
        name="${1:?usage: vm.sh snapshot NAME}"
        # qemu-img takes a second snapshot under a name already used, and
        # restoring by that name then picks the older one.
        snapshot_exists "$name" && die "a snapshot named '$name' exists; delete it first (vm.sh delete-snapshot $name)"
        qemu-img snapshot -c "$name" "$DISK"
        info "snapshot '$name' created"
        ;;
    restore)
        require_vm
        require_stopped
        qemu-img snapshot -a "${1:?usage: vm.sh restore NAME}" "$DISK"
        info "disk restored to '$1'"
        ;;
    delete-snapshot)
        require_vm
        require_stopped
        name="${1:?usage: vm.sh delete-snapshot NAME}"
        snapshot_exists "$name" || die "no snapshot named '$name' (vm.sh snapshots lists them)"
        qemu-img snapshot -d "$name" "$DISK"
        info "snapshot '$name' deleted"
        ;;
    snapshots)
        require_vm
        qemu-img snapshot -l "$DISK"
        ;;
    *)
        die "unknown command '$cmd' (start, stop, restart, status, ssh, provision, snapshot, restore, delete-snapshot, snapshots)"
        ;;
esac
