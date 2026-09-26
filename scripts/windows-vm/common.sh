#!/usr/bin/env bash
# Shared configuration and helpers for the Windows VM harness.
# Sourced by the other scripts in this directory; not meant to be run directly.
# See docs/runbooks/windows-harness.md.

HARNESS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(git -C "$HARNESS_DIR" rev-parse --show-toplevel)"

VM_NAME="${RESIDUUM_WINVM_NAME:-residuum-win11}"
VM_DIR="${RESIDUUM_WINVM_DIR:-$HOME/vms/$VM_NAME}"
WIN_ISO="${RESIDUUM_WINVM_ISO:-$HOME/vms/win11-enterprise-eval.iso}"
SSH_PORT="${RESIDUUM_WINVM_SSH_PORT:-2222}"
VNC_DISPLAY="${RESIDUUM_WINVM_VNC_DISPLAY:-59}"
VM_CPUS="${RESIDUUM_WINVM_CPUS:-4}"
VM_MEMORY_MIB="${RESIDUUM_WINVM_MEMORY_MIB:-8192}"
VM_DISK_SIZE="${RESIDUUM_WINVM_DISK_SIZE:-80G}"

GUEST_USER="dev"
GUEST_ROOT='C:\residuum-harness'
GUEST_ROOT_FWD="C:/residuum-harness"

OVMF_CODE="/usr/share/OVMF/OVMF_CODE_4M.ms.fd"
OVMF_VARS_TEMPLATE="/usr/share/OVMF/OVMF_VARS_4M.ms.fd"

DISK="$VM_DIR/disk.qcow2"
VARS="$VM_DIR/OVMF_VARS.fd"
TPM_DIR="$VM_DIR/tpm"
SSH_KEY="$VM_DIR/id_ed25519"
PASSWORD_FILE="$VM_DIR/password"
KNOWN_HOSTS="$VM_DIR/known_hosts"
QEMU_PID="$VM_DIR/qemu.pid"
MONITOR_SOCK="$VM_DIR/monitor.sock"

die() {
    echo "error: $*" >&2
    exit 1
}

# swtpm and the QEMU monitor listen on UNIX sockets inside VM_DIR, and socket
# paths are limited to 108 bytes.
if [ "${#TPM_DIR}" -gt 90 ]; then
    die "RESIDUUM_WINVM_DIR is too long for its UNIX sockets ($VM_DIR); use a shorter path"
fi

info() {
    echo "==> $*" >&2
}

require_kvm() {
    [ -e /dev/kvm ] || die "/dev/kvm is missing. Enable SVM (AMD) or VT-x (Intel) in the BIOS, then run 'sudo modprobe kvm_amd' (or kvm_intel)."
    if [ ! -r /dev/kvm ] || [ ! -w /dev/kvm ]; then
        die "no access to /dev/kvm. Add yourself to the kvm group ('sudo usermod -aG kvm $USER') and log in again."
    fi
}

require_vm() {
    [ -f "$DISK" ] || die "no VM at $VM_DIR. Create it with scripts/windows-vm/create-vm.sh."
}

vm_running() {
    [ -f "$QEMU_PID" ] && kill -0 "$(cat "$QEMU_PID")" 2>/dev/null
}

# Send one command to the QEMU human monitor.
monitor_cmd() {
    python3 - "$MONITOR_SOCK" "$1" <<'PY'
import socket, sys, time
s = socket.socket(socket.AF_UNIX)
s.connect(sys.argv[1])
s.sendall((sys.argv[2] + "\n").encode())
time.sleep(0.2)
s.close()
PY
}

# Extra QEMU arguments for the next start_vm (create-vm.sh attaches the install
# media this way).
EXTRA_QEMU_ARGS=()

# Start swtpm and QEMU in the background.
start_vm() {
    require_kvm
    if vm_running; then
        info "$VM_NAME is already running"
        return 0
    fi

    mkdir -p "$TPM_DIR"
    swtpm socket --tpm2 \
        --tpmstate dir="$TPM_DIR" \
        --ctrl type=unixio,path="$TPM_DIR/swtpm-sock" \
        --pid file="$TPM_DIR/swtpm.pid" \
        --log file="$TPM_DIR/swtpm.log" \
        --terminate --daemon

    local display_args=(-display none -vnc "127.0.0.1:$VNC_DISPLAY")
    if [ "${RESIDUUM_WINVM_DISPLAY:-}" = "gtk" ]; then
        display_args=(-display gtk)
    fi

    qemu-system-x86_64 \
        -name "$VM_NAME" \
        -machine q35,accel=kvm,smm=on \
        -global driver=cfi.pflash01,property=secure,value=on \
        -cpu host,hv_relaxed,hv_spinlocks=0x1fff,hv_vapic,hv_time \
        -smp "$VM_CPUS" -m "$VM_MEMORY_MIB" \
        -rtc base=localtime \
        -drive if=pflash,format=raw,readonly=on,file="$OVMF_CODE" \
        -drive if=pflash,format=raw,file="$VARS" \
        -chardev socket,id=chrtpm,path="$TPM_DIR/swtpm-sock" \
        -tpmdev emulator,id=tpm0,chardev=chrtpm \
        -device tpm-crb,tpmdev=tpm0 \
        -device ahci,id=ahci \
        -drive id=disk,file="$DISK",if=none,format=qcow2,discard=unmap \
        -device ide-hd,drive=disk,bus=ahci.0,bootindex=1 \
        -netdev user,id=net0,hostfwd=tcp:127.0.0.1:"$SSH_PORT"-:22 \
        -device e1000e,netdev=net0 \
        -device qemu-xhci -device usb-tablet \
        -monitor unix:"$MONITOR_SOCK",server,nowait \
        -pidfile "$QEMU_PID" \
        -daemonize \
        "${display_args[@]}" \
        "${EXTRA_QEMU_ARGS[@]}"

    info "$VM_NAME started (VNC on 127.0.0.1:$((5900 + VNC_DISPLAY)))"
}

ssh_opts() {
    printf '%s\n' \
        -i "$SSH_KEY" \
        -p "$SSH_PORT" \
        -o IdentitiesOnly=yes \
        -o StrictHostKeyChecking=accept-new \
        -o UserKnownHostsFile="$KNOWN_HOSTS" \
        -o ConnectTimeout=5 \
        -o LogLevel=ERROR
}

# Run a PowerShell snippet in the guest. The snippet is base64-encoded so no
# quoting survives (or breaks on) the trip through ssh and PowerShell.
guest_ps() {
    local encoded
    encoded="$(printf '%s' "$1" | base64 -w0)"
    local remote_command="iex ([Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('$encoded')))"
    local opts
    mapfile -t opts < <(ssh_opts)
    ssh "${opts[@]}" "$GUEST_USER@127.0.0.1" "$remote_command"
}

guest_copy_to() {
    local opts
    mapfile -t opts < <(ssh_opts | sed 's/^-p$/-P/')
    scp -q "${opts[@]}" "$1" "$GUEST_USER@127.0.0.1:$2"
}

guest_copy_from() {
    local opts
    mapfile -t opts < <(ssh_opts | sed 's/^-p$/-P/')
    scp -q "${opts[@]}" "$GUEST_USER@127.0.0.1:$1" "$2"
}

ssh_ready() {
    guest_ps 'exit 0' >/dev/null 2>&1
}

wait_for_ssh() {
    local timeout_secs="$1" waited=0
    until ssh_ready; do
        vm_running || die "$VM_NAME stopped while waiting for SSH"
        [ "$waited" -ge "$timeout_secs" ] && die "SSH not reachable after ${timeout_secs}s"
        sleep 15
        waited=$((waited + 15))
    done
}

require_ssh() {
    require_vm
    vm_running || die "$VM_NAME is not running. Start it with scripts/windows-vm/vm.sh start."
    wait_for_ssh 300
}

# Copy the working tree (tracked and untracked-but-not-ignored files, plus the
# built web/dist that build.rs needs) to a fresh source directory in the guest.
sync_tree() {
    [ -f "$REPO_ROOT/web/dist/index.html" ] || die "web/dist is not built. Run 'npm run build' in web/ first."
    local tarball
    tarball="$(mktemp --suffix=.tar)"
    (
        cd "$REPO_ROOT" || exit 1
        {
            git ls-files -co --exclude-standard -z | while IFS= read -r -d '' f; do
                [ -e "$f" ] && printf '%s\0' "$f"
            done
            find web/dist -type f -print0
        } | tar --null -cf "$tarball" -T -
    )
    guest_ps "New-Item -ItemType Directory -Force '$GUEST_ROOT' | Out-Null"
    guest_copy_to "$tarball" "$GUEST_ROOT_FWD/sync.tar"
    rm -f "$tarball"
    guest_ps "
        Remove-Item -Recurse -Force '$GUEST_ROOT\\src' -ErrorAction SilentlyContinue
        New-Item -ItemType Directory '$GUEST_ROOT\\src' | Out-Null
        tar -xf '$GUEST_ROOT\\sync.tar' -C '$GUEST_ROOT\\src'
        if (\$LASTEXITCODE) { exit \$LASTEXITCODE }
        Remove-Item '$GUEST_ROOT\\sync.tar'
    "
}
