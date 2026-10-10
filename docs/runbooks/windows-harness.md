# Windows test harness (local VM)

A Windows 11 VM on a Linux host for running residuum on real Windows: the test suite, clippy with the native toolchain, and anything that needs a desktop, such as toast notifications. The scripts live in `scripts/windows-vm/`. They drive plain QEMU (no libvirt) with KVM, UEFI Secure Boot, and an emulated TPM 2.0, and reach the guest over SSH on a localhost port.

For quick cross-platform signal without a VM, the opt-in CI checks (the `cross-compile` label; see CONTRIBUTING.md) run clippy for each target and the test suite on a Windows runner. Use this harness when you need to iterate locally or see what a user would see on screen.

## Prerequisites

- **Hardware virtualization enabled in the firmware.** AMD: "SVM Mode". Intel: "Intel Virtualization Technology (VT-x)". A BIOS or CMOS reset turns it off on some boards. Check with `ls /dev/kvm`. If it's missing, `sudo dmesg | grep -i svm` shows `SVM disabled (by BIOS)` when this is the cause.
- **KVM access.** Your user must be in the `kvm` group (`sudo usermod -aG kvm $USER`, then log in again).
- **Host packages** (Debian/Ubuntu): `sudo apt install qemu-system-x86 qemu-utils ovmf swtpm xorriso`.
- **Disk and memory.** The VM uses 4 vCPUs, 8 GiB of RAM, and an 80 GiB sparse disk (about 30 GiB used after provisioning); `create-vm.sh` requires 40 GiB free in the VM directory, which must be on a real disk, not a tmpfs such as `/tmp`. Override with `RESIDUUM_WINVM_CPUS`, `RESIDUUM_WINVM_MEMORY_MIB`, and `RESIDUUM_WINVM_DISK_SIZE`.
- **A built web UI** (`npm run build` in `web/`). The harness copies `web/dist` into the guest because `build.rs` requires it.

## One-time setup

1. Download the Windows 11 Enterprise evaluation ISO (64-bit, English (United States)) from the Microsoft Evaluation Center: https://www.microsoft.com/en-us/evalcenter/download-windows-11-enterprise. Save it as `~/vms/win11-enterprise-eval.iso`, or point `RESIDUUM_WINVM_ISO` at it.
2. Verify it against the SHA256 in Microsoft's hash list, linked from the same page ("Windows 11 Enterprise hash values"): `sha256sum ~/vms/win11-enterprise-eval.iso`.
3. Create the VM: `scripts/windows-vm/create-vm.sh`.

`create-vm.sh` does the whole setup unattended and prints progress. It generates an account password and an SSH key in the VM directory (`~/vms/residuum-win11`, or `RESIDUUM_WINVM_DIR`). It builds an answer ISO, boots the installer, and waits 20–40 minutes for Windows to install and log in automatically. Then it installs the toolchain (`vm.sh provision`), restarts without the install media, and takes a `provisioned` snapshot. To watch the install, connect a VNC viewer to `127.0.0.1:5959`.

The guest is set up as follows:

- A local administrator `dev` that logs on automatically at every boot, so a desktop session exists for toasts and screenshots.
- OpenSSH server with PowerShell as the default shell, accepting only the generated key.
- Automatic Windows Update and update reboots turned off; sleep, lock screen, and password expiry disabled.
- Git, Node.js LTS, WinLibs MinGW-w64 GCC (msvcrt, with CMake and NASM), and Rust stable with the `x86_64-pc-windows-gnu` host toolchain, matching the release target. The gnu toolchain bundles a linker but no C compiler, and the build compiles C through the `cc` crate (bundled SQLite, zstd, ring, aws-lc), so GCC is required.
- Defender exclusions for the harness directory, cargo, rustup, and the compiler.

## Daily use

Start and stop the VM:

```sh
scripts/windows-vm/vm.sh start     # boots and waits for SSH
scripts/windows-vm/vm.sh status
scripts/windows-vm/vm.sh stop      # clean shutdown
```

Run commands against your working tree:

```sh
scripts/windows-vm/run.sh cargo test --quiet
scripts/windows-vm/run.sh cargo clippy --all-targets --all-features -- -D warnings
scripts/windows-vm/run.sh cargo test --quiet daemon::
```

`run.sh` copies the working tree (tracked and untracked files that aren't gitignored, plus `web/dist`) into a fresh `C:\residuum-harness\src`, then runs the command there in PowerShell and returns its exit code. `CARGO_TARGET_DIR` is `C:\residuum-harness\target`, outside the synced tree, so builds stay incremental between runs. Set `RESIDUUM_WINVM_NO_SYNC=1` to rerun without copying.

Open a shell in the guest with `scripts/windows-vm/vm.sh ssh`. Pass a command to run just that command: `vm.sh ssh 'Get-Process residuum'`.

## Desktop checks and toasts

SSH sessions have no desktop, so notifications and windows started from them never appear. `run-interactive.sh` runs a command in the logged-on user's desktop session instead, and can capture a screenshot:

```sh
# Wait for the command to finish, then capture the screen:
scripts/windows-vm/run-interactive.sh --screenshot /tmp/win.png -- cargo run -- serve --foreground

# Leave a long-running command up, and capture the screen 15s after it starts:
scripts/windows-vm/run-interactive.sh --detach --delay 15 --screenshot /tmp/toast.png -- cargo run -- serve --foreground

# Screenshot only:
scripts/windows-vm/run-interactive.sh --screenshot /tmp/desktop.png
```

To verify a toast, trigger the notification with the gateway running in the desktop session, then take a screenshot. Reminder-scenario toasts (urgent results) stay on screen until dismissed, so they show up reliably. Normal toasts disappear after a few seconds, so use a short `--delay`, or open the notification center over VNC. Stop a detached command with `vm.sh ssh 'Stop-Process -Name residuum'`.

## Snapshots

Snapshots are internal to the qcow2 disk and require the VM to be stopped:

```sh
scripts/windows-vm/vm.sh stop
scripts/windows-vm/vm.sh snapshot before-experiment
scripts/windows-vm/vm.sh restore provisioned
scripts/windows-vm/vm.sh snapshots
```

Restoring to `provisioned` resets the guest to a clean toolchain install. Firmware variables and TPM state are not part of a snapshot. Nothing in the guest depends on them after installation.

## Evaluation expiry

The Enterprise evaluation runs for 90 days. After that, Windows shows a watermark and shuts down periodically. To extend it, run `vm.sh ssh 'slmgr /rearm'` and then `vm.sh restart`; the number of rearms is limited. Otherwise, recreate the VM: stop it, delete `~/vms/residuum-win11`, download the current evaluation ISO, and run `create-vm.sh` again (about an hour, unattended).

## Troubleshooting

- **`/dev/kvm is missing`.** Virtualization is off in the firmware; see Prerequisites. Then `sudo modprobe kvm_amd` (or `kvm_intel`).
- **`create-vm.sh` waits for SSH forever.** Watch over VNC (`127.0.0.1:5959`). If setup shows the language screen, the answer ISO wasn't found. If it stopped at "Press any key to boot from CD", the Enter key presses were missed; stop the VM, delete the VM directory, and run `create-vm.sh` again. If the desktop is up but SSH isn't, read `C:\first-logon.log` in the guest over VNC.
- **`SSH not reachable`** after a restart. Windows may still be booting; `vm.sh status` shows whether QEMU is running. Something else may be using port 2222; set `RESIDUUM_WINVM_SSH_PORT` to a different port before starting.
- **`no desktop session`** from `run-interactive.sh`. Autologon is off, or the user signed out over VNC. Restart with `vm.sh restart`.
- **Toolchain problems.** Rerun `vm.sh provision`, which only installs what's missing, or `vm.sh provision -Upgrade` to move everything to the current releases.
- **`RESIDUUM_WINVM_DIR is too long`.** swtpm and the QEMU monitor use UNIX sockets in the VM directory, and socket paths are limited to 108 bytes. Use a shorter directory.
- **Two VMs at once.** Give each its own `RESIDUUM_WINVM_NAME` (or `RESIDUUM_WINVM_DIR`), `RESIDUUM_WINVM_SSH_PORT`, and `RESIDUUM_WINVM_VNC_DISPLAY`.
