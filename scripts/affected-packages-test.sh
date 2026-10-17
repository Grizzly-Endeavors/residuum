#!/usr/bin/env bash
# Runs scripts/affected-packages.sh over a scratch tree with three fake members and
# checks, for each class of changed path, the packages --affected and --touched
# print, the integration test modules --covering prints, the failure of the modes that
# have no fallback of their own, and the graph --graph prints.
#
# The tree: crates/a with no workspace dependency, crates/b depending on a,
# crates/net holding src/browser_landing.rs and depending on a in its dev
# dependencies, crates/hub with no workspace dependency and no dependents (the
# package the two paths it reads belong to), and a root manifest depending on
# a, b and net. It also holds the integration modules that --covering reads, set
# up below with the cases that use them.
set -euo pipefail

dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
script="$dir/affected-packages.sh"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT

mkdir -p "$scratch/crates/a/src" "$scratch/crates/b/src" "$scratch/crates/net/src" "$scratch/crates/hub/src"
cd "$scratch"
cat > Cargo.toml <<'EOF'
[package]
name = "residuum"
version.workspace = true

[dependencies]
residuum-a = { workspace = true }
residuum-b = { workspace = true }
residuum-net = { workspace = true }
EOF
cat > crates/hub/Cargo.toml <<'EOF'
[package]
name = "residuum-hub"
version.workspace = true
EOF
cat > crates/a/Cargo.toml <<'EOF'
[package]
name = "residuum-a"
version.workspace = true
EOF
cat > crates/b/Cargo.toml <<'EOF'
[package]
name = "residuum-b"
version.workspace = true

[dependencies]
residuum-a = { workspace = true }
EOF
cat > crates/net/Cargo.toml <<'EOF'
[package]
name = "residuum-net"
version.workspace = true

[dev-dependencies]
residuum-a = { workspace = true }
EOF
touch crates/net/src/browser_landing.rs

# The integration modules, as tests/it holds them, with the covers line each
# carries. one covers residuum-a, two residuum-b and the root, three/ (a directory
# module) residuum-a, four residuum-b, and five residuum-hub. The container files
# and support are not modules.
mkdir -p tests/it/three tests/it/support tests/fixtures
printf 'mod one;\n' >tests/it/main.rs
printf '//! covers: residuum-a\n' >tests/it/one.rs
printf '//! covers: residuum-b, residuum\n' >tests/it/two.rs
printf '//! covers: residuum-a\n' >tests/it/three/mod.rs
touch tests/it/three/x.rs
touch tests/it/support/mod.rs tests/fixtures/x
printf '//! covers: residuum-b\n' >tests/it/four.rs
printf '//! covers: residuum-hub\n' >tests/it/five.rs

status=0

# run <label> <mode> <changed path>: sets OUT to the script's stdout. A script that
# exits non-zero is a failure of the case, so it is reported, and the case's
# expectation is not compared with the output of a failed run.
run() {
    if OUT="$(printf '%s\n' "$3" | "$script" "$2" 2>/dev/null)"; then
        return 0
    fi
    echo "affected-packages: $1 ($2, $3): exited non-zero" >&2
    status=1
    return 1
}

# check <label> <mode> <changed path> <expected stdout>
check() {
    run "$1" "$2" "$3" || return 0
    if [ "$OUT" != "$4" ]; then
        echo "affected-packages: $1 ($2, $3): expected '$4', got '$OUT'" >&2
        status=1
    fi
}

# check_words <label> <mode> <changed path> <expected words, space-separated>
# compares the script's output with its lines joined by single spaces, so the
# expected value fits on one row of a table. Empty output is an empty expectation.
check_words() {
    local actual
    run "$1" "$2" "$3" || return 0
    actual="$(printf '%s' "$OUT" | tr '\n' ' ')"
    actual="${actual% }"
    if [ "$actual" != "$4" ]; then
        echo "affected-packages: $1 ($2, $3): expected '$4', got '$actual'" >&2
        status=1
    fi
}

all_packages="-p residuum -p residuum-a -p residuum-b -p residuum-net"

# Each row: changed path | expected for --affected.
while IFS='|' read -r path affected; do
    check "$path" --affected "$path" "$affected"
done <<CASES
crates/a/src/lib.rs|$all_packages
crates/a/Cargo.toml|$all_packages
crates/b/src/lib.rs|-p residuum -p residuum-b
crates/net/src/lib.rs|-p residuum -p residuum-net
crates/net/src/browser_landing.rs|-p residuum -p residuum-net
src/main.rs|-p residuum
tests/it/gateway.rs|-p residuum
build.rs|-p residuum
web/src/lib/generated/types.ts|-p residuum
web/app-shell-csp.txt|-p residuum-hub
scripts/check-generated-types.sh|-p residuum
scripts/export-generated-types.sh|-p residuum
.github/workflows/release.yml|-p residuum-hub
web/src/styles/tokens.css|-p residuum -p residuum-net
Cargo.toml|--workspace
Cargo.lock|--workspace
rust-toolchain.toml|--workspace
clippy.toml|--workspace
rustfmt.toml|--workspace
deny.toml|--workspace
.cargo/config.toml|--workspace
assets/icon.png|--workspace
tests/fixtures/data.json|--workspace
.github/workflows/ci.yml|--workspace
.github/workflows/rust-checks.yml|--workspace
scripts/affected-packages.sh|--workspace
scripts/affected-packages-test.sh|--workspace
scripts/check-crate-isolation.sh|--workspace
crates/zzz/README.md|--workspace
docs/archive/workspace-split.md|
.githooks/pre-push|
CASES

expected_graph="residuum: residuum-a residuum-b residuum-net
residuum-a:
residuum-b: residuum-a
residuum-hub:
residuum-net: residuum-a"
actual_graph="$("$script" --graph)"
if [ "$actual_graph" != "$expected_graph" ]; then
    echo "affected-packages: --graph: expected" >&2
    echo "$expected_graph" >&2
    echo "got" >&2
    echo "$actual_graph" >&2
    status=1
fi

# Each row: changed path | expected for --touched | expected for --covering.
while IFS='|' read -r path touched covering; do
    check_words "$path" --touched "$path" "$touched"
    check_words "$path" --covering "$path" "$covering"
done <<'CASES'
crates/a/src/lib.rs|residuum-a|it:one it:three
crates/a/Cargo.toml|residuum-a|it:one it:three
crates/b/src/lib.rs|residuum-b|it:four it:two
crates/b/Cargo.toml|residuum-b|it:four it:two
crates/net/src/lib.rs|residuum-net|
crates/net/src/browser_landing.rs|residuum-net|
crates/net||
web/src/styles/tokens.css|residuum-net|
web/app-shell-csp.txt|residuum-hub|it:five
.github/workflows/release.yml|residuum-hub|it:five
src/lib.rs||it:two
src/main.rs||it:two
src/daemon.rs||it:two
tests/it/one.rs||it:one
tests/it/three/x.rs||it:three
tests/it/main.rs||it:ALL
tests/it/support/mod.rs||it:ALL
tests/fixtures/x||it:ALL
assets/icon.png||it:ALL
tests/it/four.rs||it:four
Cargo.toml||
Cargo.lock||
rust-toolchain.toml||
.github/workflows/ci.yml||
scripts/affected-packages.sh||
docs/archive/workspace-split.md||
CASES

# An unreadable manifest checks more, never less. Root can read any file, so the
# case needs an unprivileged run.
chmod 000 crates/b/Cargo.toml
if [ -r crates/b/Cargo.toml ]; then
    echo "affected-packages-test: skipped the unreadable-manifest case: the manifest is still readable (running as root?)" >&2
else
    check "unreadable member manifest" --affected crates/a/src/lib.rs "--workspace"
    warning="$(printf '%s\n' crates/a/src/lib.rs | "$script" --affected 2>&1 >/dev/null || true)"
    if ! grep -q '^warning:' <<<"$warning"; then
        echo "affected-packages: unreadable member manifest printed no warning" >&2
        status=1
    fi
    if "$script" --graph >/dev/null 2>&1; then
        echo "affected-packages: --graph succeeded with an unreadable manifest" >&2
        status=1
    fi
    # --touched and --covering have no every-package fallback of their own: they
    # fail, and the hook runs the every-member or every-module fallback.
    for mode in --touched --covering; do
        if printf '%s\n' crates/a/src/lib.rs | "$script" "$mode" >/dev/null 2>&1; then
            echo "affected-packages: $mode succeeded with an unreadable manifest" >&2
            status=1
        fi
    done
fi
chmod 644 crates/b/Cargo.toml

# An unreadable module file fails --covering, since its covers line is unknown.
chmod 000 tests/it/one.rs
if [ -r tests/it/one.rs ]; then
    echo "affected-packages-test: skipped the unreadable-module case: the module is still readable (running as root?)" >&2
elif printf '%s\n' crates/a/src/lib.rs | "$script" --covering >/dev/null 2>&1; then
    echo "affected-packages: --covering succeeded with an unreadable tests/it module" >&2
    status=1
fi
chmod 644 tests/it/one.rs

if [ "$status" -eq 0 ]; then
    echo "affected-packages: every path class, mode, fallback and the graph match"
fi
exit "$status"
