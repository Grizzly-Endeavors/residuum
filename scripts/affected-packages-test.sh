#!/usr/bin/env bash
# Runs scripts/affected-packages.sh over a scratch tree with three fake members and
# checks, for each class of changed path, the packages --affected, --members and
# --touched print, the integration modules --covering prints, the failure of the
# modes that have no fallback of their own, and the graph --graph prints.
#
# The tree: crates/a with no workspace dependency, crates/b depending on a,
# crates/net holding src/browser_landing.rs and depending on a in its dev
# dependencies, and a root manifest depending on all three. It also holds the
# integration modules and scripts/pending-crates.txt that --covering reads, set
# up below with the cases that use them.
set -euo pipefail

dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
script="$dir/affected-packages.sh"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT

mkdir -p "$scratch/crates/a/src" "$scratch/crates/b/src" "$scratch/crates/net/src"
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

# The integration modules, as tests/it and src/integration hold them, with the
# covers line each carries. one covers residuum-a, two residuum-b and the root,
# three/ (a directory module) residuum-a, later residuum-later, and four (under
# src/integration) residuum-b. The container files and support are not modules.
mkdir -p tests/it/three tests/it/support tests/fixtures src/integration scripts
printf 'mod one;\n' >tests/it/main.rs
printf '//! covers: residuum-a\n' >tests/it/one.rs
printf '//! covers: residuum-b, residuum\n' >tests/it/two.rs
printf '//! covers: residuum-a\n' >tests/it/three/mod.rs
touch tests/it/three/x.rs
printf '//! covers: residuum-later\n' >tests/it/later.rs
touch tests/it/support/mod.rs tests/fixtures/x
touch src/integration/mod.rs
printf '//! covers: residuum-b\n' >src/integration/four.rs
# src/later/ is pending for residuum-later. The longer file entry takes its own
# file out of that, to residuum-b.
cat >scripts/pending-crates.txt <<'EOF'
src/later/ residuum-later
src/later/deep.rs residuum-b
EOF

status=0

# check <label> <mode> <changed path> <expected stdout>
check() {
    local actual
    actual="$(printf '%s\n' "$3" | "$script" "$2" 2>/dev/null)"
    if [ "$actual" != "$4" ]; then
        echo "affected-packages: $1 ($2, $3): expected '$4', got '$actual'" >&2
        status=1
    fi
}

# check_words <label> <mode> <changed path> <expected words, space-separated>
# compares the script's output with its lines joined by single spaces, so the
# expected value fits on one row of a table. Empty output is an empty expectation.
check_words() {
    local actual
    actual="$(printf '%s\n' "$3" | "$script" "$2" 2>/dev/null | tr '\n' ' ' || true)"
    actual="${actual% }"
    if [ "$actual" != "$4" ]; then
        echo "affected-packages: $1 ($2, $3): expected '$4', got '$actual'" >&2
        status=1
    fi
}

all_members="-p residuum-a -p residuum-b -p residuum-net"
all_packages="-p residuum -p residuum-a -p residuum-b -p residuum-net"

# Each row: changed path | expected for --affected | expected for --members.
while IFS='|' read -r path affected members; do
    check "$path" --affected "$path" "$affected"
    check "$path" --members "$path" "$members"
done <<CASES
crates/a/src/lib.rs|$all_packages|$all_members
crates/a/Cargo.toml|$all_packages|$all_members
crates/b/src/lib.rs|-p residuum -p residuum-b|-p residuum-b
crates/net/src/lib.rs|-p residuum -p residuum-net|-p residuum-net
crates/net/src/browser_landing.rs|-p residuum -p residuum-net|-p residuum-net
src/main.rs|-p residuum|
tests/it/gateway.rs|-p residuum|
build.rs|-p residuum|
web/src/lib/generated/types.ts|-p residuum|
web/app-shell-csp.txt|-p residuum|
scripts/check-generated-types.sh|-p residuum|
scripts/export-generated-types.sh|-p residuum|
.github/workflows/release.yml|-p residuum|
web/src/styles/tokens.css|-p residuum -p residuum-net|-p residuum-net
Cargo.toml|--workspace|$all_members
Cargo.lock|--workspace|$all_members
rust-toolchain.toml|--workspace|$all_members
clippy.toml|--workspace|$all_members
rustfmt.toml|--workspace|$all_members
deny.toml|--workspace|$all_members
.cargo/config.toml|--workspace|$all_members
assets/icon.png|--workspace|$all_members
tests/fixtures/data.json|--workspace|$all_members
.github/workflows/ci.yml|--workspace|$all_members
.github/workflows/rust-checks.yml|--workspace|$all_members
scripts/affected-packages.sh|--workspace|$all_members
scripts/affected-packages-test.sh|--workspace|$all_members
scripts/check-crate-isolation.sh|--workspace|$all_members
crates/zzz/README.md|--workspace|$all_members
docs/archive/workspace-split.md||
.githooks/pre-push||
CASES

expected_graph="residuum: residuum-a residuum-b residuum-net
residuum-a:
residuum-b: residuum-a
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
crates/b/src/lib.rs|residuum-b|integration:four it:two
crates/b/Cargo.toml|residuum-b|integration:four it:two
crates/net/src/lib.rs|residuum-net|
crates/net/src/browser_landing.rs|residuum-net|
crates/net||
web/src/styles/tokens.css|residuum-net|
web/app-shell-csp.txt||it:two
.github/workflows/release.yml||it:two
src/lib.rs||it:two
src/main.rs||it:two
src/later/x.rs||it:later
src/later/deep.rs||integration:four it:two
tests/it/one.rs||it:one
tests/it/three/x.rs||it:three
tests/it/main.rs||it:ALL
tests/it/support/mod.rs||it:ALL
tests/fixtures/x||it:ALL
assets/icon.png||it:ALL
src/integration/four.rs||integration:four
src/integration/mod.rs||integration:ALL
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
    check "unreadable member manifest" --members crates/a/src/lib.rs "$all_members"
    warning="$(printf '%s\n' crates/a/src/lib.rs | "$script" --affected 2>&1 >/dev/null || true)"
    if ! grep -q '^warning:' <<<"$warning"; then
        echo "affected-packages: unreadable member manifest printed no warning" >&2
        status=1
    fi
    if "$script" --graph >/dev/null 2>&1; then
        echo "affected-packages: --graph succeeded with an unreadable manifest" >&2
        status=1
    fi
    # The new modes have no every-package fallback of their own: they fail, and
    # the hook runs the every-member or every-module fallback.
    for mode in --touched --covering; do
        if printf '%s\n' crates/a/src/lib.rs | "$script" "$mode" >/dev/null 2>&1; then
            echo "affected-packages: $mode succeeded with an unreadable manifest" >&2
            status=1
        fi
    done
fi
chmod 644 crates/b/Cargo.toml

# An unreadable pending-crates file fails --covering, for the same reason.
mv scripts/pending-crates.txt scripts/pending-crates.txt.saved
if printf '%s\n' src/later/x.rs | "$script" --covering >/dev/null 2>&1; then
    echo "affected-packages: --covering succeeded without scripts/pending-crates.txt" >&2
    status=1
fi
mv scripts/pending-crates.txt.saved scripts/pending-crates.txt

# The hub reads two of the paths by path: web/app-shell-csp.txt from
# crates/hub/src/web/assets.rs and .github/workflows/release.yml from
# crates/hub/src/update.rs. Each goes to residuum-hub only while the hub's
# manifest and the file that reads it exist, and to the root otherwise. The
# hub is added here, after the cases above, which run without it.

# readers <state> <csp --affected> <csp --members> <release --affected> <release --members>
readers() {
    check "web/app-shell-csp.txt, $1" --affected web/app-shell-csp.txt "$2"
    check "web/app-shell-csp.txt, $1" --members web/app-shell-csp.txt "$3"
    check ".github/workflows/release.yml, $1" --affected .github/workflows/release.yml "$4"
    check ".github/workflows/release.yml, $1" --members .github/workflows/release.yml "$5"
}

mkdir -p crates/hub/src/web
cat > crates/hub/Cargo.toml <<'EOF'
[package]
name = "residuum-hub"
version.workspace = true
EOF
touch crates/hub/src/web/assets.rs crates/hub/src/update.rs
readers "hub member with both readers" "-p residuum-hub" "-p residuum-hub" "-p residuum-hub" "-p residuum-hub"

rm crates/hub/src/web/assets.rs
readers "hub member without the CSP reader" "-p residuum" "" "-p residuum-hub" "-p residuum-hub"

touch crates/hub/src/web/assets.rs
rm crates/hub/src/update.rs
readers "hub member without the workflow reader" "-p residuum-hub" "-p residuum-hub" "-p residuum" ""

rm crates/hub/src/web/assets.rs
readers "hub member without either reader" "-p residuum" "" "-p residuum" ""

touch crates/hub/src/web/assets.rs crates/hub/src/update.rs
rm crates/hub/Cargo.toml
readers "readers without the hub manifest" "-p residuum" "" "-p residuum" ""
rm -rf crates/hub

if [ "$status" -eq 0 ]; then
    echo "affected-packages: every path class, mode, fallback and the graph match"
fi
exit "$status"
