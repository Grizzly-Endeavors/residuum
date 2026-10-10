#!/usr/bin/env bash
# Create the Windows test VM end to end: unattended Windows install, SSH
# access, toolchain provisioning, and a "provisioned" snapshot to reset to.
#
#   scripts/windows-vm/create-vm.sh                    # full create (needs KVM)
#   scripts/windows-vm/create-vm.sh --answer-iso-only FILE
#                                                      # build only the answer ISO
#
# See docs/runbooks/windows-harness.md.
set -euo pipefail

# shellcheck source=scripts/windows-vm/common.sh
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"

INSTALL_TIMEOUT_SECS=5400

# Build the answer ISO: autounattend.xml, the first-logon script, the SSH
# public key, and the account password. Windows setup finds autounattend.xml
# at the root of any attached CD.
build_answer_iso() {
    local out="$1" password="$2" pubkey="$3"
    local staging
    staging="$(mktemp -d)"
    sed -e "s/@USER@/$GUEST_USER/g" -e "s/@PASSWORD@/$password/g" \
        "$HARNESS_DIR/guest/autounattend.xml.tmpl" > "$staging/autounattend.xml"
    cp "$HARNESS_DIR/guest/first-logon.ps1" "$staging/first-logon.ps1"
    cp "$pubkey" "$staging/authorized_keys"
    printf '%s' "$password" > "$staging/password.txt"
    xorriso -as mkisofs -quiet -J -r -V ANSWERS -o "$out" "$staging"
    rm -rf "$staging"
}

if [ "${1:-}" = "--answer-iso-only" ]; then
    out="${2:?usage: create-vm.sh --answer-iso-only FILE}"
    [ -f "$out.key" ] || ssh-keygen -q -t ed25519 -N '' -C "residuum-harness-dry-run" -f "$out.key"
    build_answer_iso "$out" "Rv-dryrun-password!" "$out.key.pub"
    info "answer ISO written to $out (SSH key $out.key, password Rv-dryrun-password!)"
    exit 0
fi

require_kvm
[ -f "$WIN_ISO" ] || die "Windows ISO not found at $WIN_ISO. Download it as described in docs/runbooks/windows-harness.md, or set RESIDUUM_WINVM_ISO."
[ -f "$DISK" ] && die "a VM already exists at $VM_DIR. Delete that directory to recreate it."
for tool in qemu-system-x86_64 qemu-img swtpm xorriso ssh-keygen python3; do
    command -v "$tool" >/dev/null || die "$tool is not installed (see the runbook's one-time setup)"
done
[ -f "$OVMF_CODE" ] || die "$OVMF_CODE not found; install the ovmf package"

mkdir -p "$VM_DIR"
chmod 700 "$VM_DIR"

# The sparse disk grows to about 30 GiB by the end of provisioning, and a full
# filesystem mid-install surfaces in the guest as a disk error.
free_gib=$(( $(df --output=avail -k "$VM_DIR" | tail -1) / 1024 / 1024 ))
[ "$free_gib" -ge 40 ] || die "only ${free_gib} GiB free for $VM_DIR; the VM needs at least 40 GiB. Set RESIDUUM_WINVM_DIR to a larger filesystem (not a tmpfs such as /tmp)."

info "generating credentials in $VM_DIR"
password="Rv-$(head -c 12 /dev/urandom | od -An -tx1 | tr -d ' \n')!"
printf '%s\n' "$password" > "$PASSWORD_FILE"
chmod 600 "$PASSWORD_FILE"
[ -f "$SSH_KEY" ] || ssh-keygen -q -t ed25519 -N '' -C "residuum-harness" -f "$SSH_KEY"

info "building answer ISO"
build_answer_iso "$VM_DIR/answer.iso" "$password" "$SSH_KEY.pub"

info "creating $VM_DISK_SIZE disk and UEFI variable store"
qemu-img create -q -f qcow2 "$DISK" "$VM_DISK_SIZE"
cp "$OVMF_VARS_TEMPLATE" "$VARS"

EXTRA_QEMU_ARGS=(
    -drive "id=installer,file=$WIN_ISO,if=none,media=cdrom,readonly=on"
    -device "ide-cd,drive=installer,bus=ahci.1,bootindex=0"
    -drive "id=answers,file=$VM_DIR/answer.iso,if=none,media=cdrom,readonly=on"
    -device "ide-cd,drive=answers,bus=ahci.2"
)
start_vm

# The Windows DVD waits for a key press ("Press any key to boot from CD or
# DVD") before it boots, so keep pressing Enter while firmware hands off.
info "booting the installer"
for _ in $(seq 1 30); do
    monitor_cmd "sendkey ret" 2>/dev/null || true
    sleep 1
done

info "installing Windows unattended; this takes 20-40 minutes (watch with a VNC viewer on 127.0.0.1:$((5900 + VNC_DISPLAY)))"
wait_for_ssh "$INSTALL_TIMEOUT_SECS"
info "SSH is up"

"$HARNESS_DIR/vm.sh" provision

info "restarting without install media and taking the 'provisioned' snapshot"
"$HARNESS_DIR/vm.sh" stop
"$HARNESS_DIR/vm.sh" snapshot provisioned
"$HARNESS_DIR/vm.sh" start

info "done. Try: scripts/windows-vm/run.sh cargo test --quiet"
