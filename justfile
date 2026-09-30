# Local dev tasks for Residuum. Run `just` to list them; `just check` runs what CI runs.
#
# Every recipe wraps a command you can run by hand (see CONTRIBUTING.md), so
# nothing here is required to build or contribute.

set shell := ["bash", "-euo", "pipefail", "-c"]

[private]
default:
    @just --list --unsorted

# --- setup -------------------------------------------------------------------

# One-time setup: git hooks and web dependencies
[group('setup')]
setup: hooks web-install

# Symlink the tracked git hooks into this repo
[group('setup')]
hooks:
    .githooks/install.sh

# --- run ---------------------------------------------------------------------

# Start the hub in the foreground; extra args go to `residuum serve`
[group('run')]
serve *args: _web-dist
    cargo run -- serve --foreground {{ args }}

# Run any residuum subcommand, e.g. `just run logs --watch`
[group('run')]
run *args: _web-dist
    cargo run -- {{ args }}

# --- rust --------------------------------------------------------------------

# Debug build
[group('rust')]
build: _web-dist
    cargo build

# Release build; always rebuilds the web assets, since the binary embeds them
[group('rust')]
release: web-build
    cargo build --release

# Run tests, quietly; args are cargo's filters, e.g. `just test memory::` or `just test --test smoke`
[group('rust')]
test *args: _web-dist
    cargo test --quiet {{ args }}

# Regenerate the TypeScript protocol types in web/src/lib/generated from the Rust types
[group('rust')]
types: _web-dist
    cargo test --quiet --test ts_export

# Clippy with warnings denied, exactly as the pre-commit hook runs it
[group('rust')]
clippy: _web-dist
    cargo clippy --all-targets --all-features -- -D warnings

# Format Rust code
[group('rust')]
rust-fmt:
    cargo fmt --all

# Check Rust formatting without rewriting
[group('rust')]
rust-fmt-check:
    cargo fmt --all --check

# Dependency audit (advisories, licenses, sources)
[group('rust')]
deny:
    @command -v cargo-deny >/dev/null || { echo "cargo-deny is not installed: cargo install cargo-deny" >&2; exit 1; }
    cargo deny check

# Lint one other release target from Linux: aarch64-unknown-linux-gnu or x86_64-pc-windows-gnu
[group('rust')]
cross-clippy target="x86_64-pc-windows-gnu": _web-dist
    rustup target add {{ target }}
    cargo clippy --target {{ target }} --all-targets --all-features -- -D warnings

# Lint every cross-compile target CI covers
[group('rust')]
cross-check: (cross-clippy "aarch64-unknown-linux-gnu") (cross-clippy "x86_64-pc-windows-gnu")

# --- web ---------------------------------------------------------------------

# Install web dependencies from the lockfile
[group('web')]
web-install:
    cd web && npm ci

# Vite dev server against the mock API; no Rust backend needed. Extra args go to vite, e.g. `just web-mock --port 5199`
[group('web')]
web-mock *args: _web-deps
    cd web && npm run dev:mock -- {{ args }}

# Mock dev server that boots into the first-run setup wizard
[group('web')]
web-mock-setup *args: _web-deps
    cd web && VITE_MOCK_SETUP=1 npm run dev:mock -- {{ args }}

# Vite dev server proxying /api and /ws to a real backend on :7700 (start one with `just serve`)
[group('web')]
web-dev *args: _web-deps
    cd web && npm run dev -- {{ args }}

# Production build into web/dist, which the Rust binary embeds
[group('web')]
web-build: _web-deps
    cd web && npm run build

# Format web sources
[group('web')]
web-fmt: _web-deps
    cd web && npm run format

# Check web formatting without rewriting
[group('web')]
web-fmt-check: _web-deps
    cd web && npm run format:check

# ESLint plus the style lint (design tokens in stylesheets and component styles)
[group('web')]
web-lint: _web-deps
    cd web && npm run lint

# ESLint with autofix
[group('web')]
web-lint-fix: _web-deps
    cd web && npm run lint:fix

# svelte-check type checking
[group('web')]
web-typecheck: _web-deps
    cd web && npm run check

# Vitest; args are vitest's filters, e.g. `just web-test src/lib/time.test.ts`
[group('web')]
web-test *args: _web-deps
    cd web && npm test -- {{ args }}

# --- checks ------------------------------------------------------------------

# Everything CI's web job runs
[group('check')]
web-check: web-fmt-check web-lint web-typecheck web-test web-build

# Everything CI's rust job runs
[group('check')]
rust-check: rust-fmt-check clippy test deny

# Everything CI runs; web goes first because the Rust build embeds web/dist
[group('check')]
check: web-check rust-check

# Format Rust and web sources
[group('check')]
fmt: rust-fmt web-fmt

# Check Rust and web formatting without rewriting
[group('check')]
fmt-check: rust-fmt-check web-fmt-check

# --- misc --------------------------------------------------------------------

# Lines-of-code summary (needs cloc); extra args go to cloc
[group('misc')]
loc *args:
    scripts/loc.sh {{ args }}

# Remove build output; the next cargo recipe rebuilds web/dist on its own
[group('misc')]
clean:
    cargo clean
    rm -rf web/dist

# Reinstall when package-lock.json is newer than the one npm ci leaves in node_modules (or that is missing)
[private]
_web-deps:
    @[ web/node_modules/.package-lock.json -nt web/package-lock.json ] || (cd web && npm ci)

# build.rs panics without web/dist. A stale dist is refreshed by `just web-build`.
[private]
_web-dist: _web-deps
    @[ -f web/dist/index.html ] || (cd web && npm run build)
