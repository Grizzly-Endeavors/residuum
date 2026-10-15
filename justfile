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

# Install the tracked git hooks into this repo's hooks directory (all worktrees share it)
[group('setup')]
hooks:
    .githooks/install.sh

# --- run ---------------------------------------------------------------------

# Start the hub in the foreground; extra args go to `residuum serve`
[group('run')]
[positional-arguments]
serve *args: _web-dist
    cargo run -- serve --foreground "$@"

# Run any residuum subcommand, e.g. `just run logs --watch`
[group('run')]
[positional-arguments]
run *args: _web-dist
    cargo run -- "$@"

# --- rust --------------------------------------------------------------------

# Debug build
[group('rust')]
build: _web-dist
    cargo build

# Release build; always rebuilds the web assets, since the binary embeds them
[group('rust')]
release: web-build
    cargo build --release

# Run tests, quietly, and run every test binary even after one fails, so the run lists every failure; args are cargo's filters, e.g. `just test memory::` or `just test --test smoke`
[group('rust')]
[positional-arguments]
test *args: _web-dist
    cargo test --quiet --no-fail-fast "$@"

# Run the tests under load: two copies at once plus CPU burners (STRESS_COPIES, STRESS_ROUNDS, STRESS_BURN); args are a libtest filter, e.g. `just stress hub::`
[group('rust')]
[positional-arguments]
stress *args: _web-dist
    scripts/stress-tests.sh "$@"

# Run the remote access tests that start Pebble (the Let's Encrypt test CA) in Docker
[group('rust')]
pebble: _web-dist
    cargo test --quiet --lib -- --ignored --test-threads=1 pebble system_tests

# Regenerate the TypeScript protocol types in web/src/lib/generated from the Rust types; the files are replaced only if the export succeeds
[group('rust')]
types: _web-dist
    scripts/update-generated-types.sh

# Regenerate the TypeScript types from scratch and fail if they differ from the index, as CI does (stage regenerated files first)
[group('rust')]
types-check: _web-dist
    scripts/check-generated-types.sh

# Clippy with warnings denied, exactly as the pre-push hook runs it
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
[positional-arguments]
web-mock *args: _web-deps
    cd web && npm run dev:mock -- "$@"

# Mock dev server that boots into the first-run setup wizard
[group('web')]
[positional-arguments]
web-mock-setup *args: _web-deps
    cd web && VITE_MOCK_SETUP=1 npm run dev:mock -- "$@"

# Deterministic mock (fixed clock, no delays, fixed artifacts port) on a Vite dev server, headless on a port. `POST /api/mock/reset` restores the scenario.
[group('web')]
[positional-arguments]
web-mock-serve port="5173" *args: _web-deps
    cd web && MOCK_DETERMINISTIC=1 npm run dev:mock -- --port {{ port }} --strictPort "${@:2}"

# Production build served with the whole mock (API, sockets, artifacts page), deterministic and headless on a port
[group('web')]
[positional-arguments]
web-mock-preview port="4173" *args: _web-deps
    cd web && npm run build && MOCK_DETERMINISTIC=1 npm run preview:mock -- --port {{ port }} --strictPort "${@:2}"

# Vite dev server proxying /api and /ws to a real backend on :7700 (start one with `just serve`)
[group('web')]
[positional-arguments]
web-dev *args: _web-deps
    cd web && npm run dev -- "$@"

# Production build into web/dist, which the Rust binary embeds
[group('web')]
web-build: _web-deps
    cd web && npm run build

# Gzipped size of the initial route (the scripts and styles loaded before the first screen), from a fresh build, as CI's web job reports it
[group('web')]
web-size: web-build
    scripts/web-initial-route-size.sh web/dist

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
[positional-arguments]
web-test *args: _web-deps
    cd web && npm test -- "$@"

# Vitest with a coverage summary; the full HTML report is written to web/coverage
[group('web')]
[positional-arguments]
web-coverage *args: _web-deps
    cd web && npm run test:coverage -- "$@"

# End-to-end and accessibility specs in Chromium, desktop and phone. The visual specs are left out. Runs on this machine's Chromium, installed on first use; when that install fails or stalls, the run says so and moves to the Playwright container (needs Docker). Args go to playwright, e.g. `just web-e2e-fast e2e/smoke`
[group('web')]
[positional-arguments]
web-e2e-fast *args: _web-deps
    scripts/with-e2e-browser.sh npm --prefix web run e2e:fast -- "$@"

# The whole end-to-end suite: the fast specs on this machine, the visual comparisons in the Playwright container (needs Docker). When this machine's Chromium can't be installed, the fast specs run in the container too. CI runs the fast specs in its Web End-to-End jobs and the visual comparisons in a job of their own that never fails a pull request
[group('web')]
[positional-arguments]
web-e2e *args: _web-deps
    scripts/with-e2e-browser.sh --visual npm --prefix web run e2e -- "$@"

# Refresh the visual baselines in the Playwright container (needs Docker); `just web-e2e-update -g "<part of a test title>"` refreshes only the matching shots. Review the changed images under web/e2e/__screenshots__ before committing them
[group('web')]
[positional-arguments]
web-e2e-update *args: _web-deps
    scripts/with-playwright-container.sh npm --prefix web run e2e:visual -- --update-snapshots=all "$@"

# The WebKit phone project, in the Playwright container (needs Docker), so this machine needs no WebKit. Local only: CI doesn't run it
[group('web')]
[positional-arguments]
web-e2e-webkit *args: _web-deps
    scripts/with-playwright-container.sh npm --prefix web run e2e:webkit -- "$@"

# The end-to-end specs this branch adds or changes (every spec with WEB_STRESS_ALL=1), each run repeatedly (WEB_STRESS_REPEAT, default 5) with the browser's CPU throttled (WEB_STRESS_CPU_THROTTLE, default 4); on this machine's Chromium, or in the Playwright container when that can't be installed; args go to playwright, e.g. `just web-stress --grep "tab is titled"`
[group('web')]
[positional-arguments]
web-stress *args: _web-deps
    scripts/with-e2e-browser.sh scripts/web-stress.sh "$@"

# --- checks ------------------------------------------------------------------

web_checks := "web-fmt-check web-lint web-typecheck web-test web-size"

# CI's web job except the end-to-end suite, which has its own recipes (web-e2e-fast, web-e2e); web-size builds, then prints the initial route's size. Runs every check and lists the failures.
[group('check')]
web-check: _web-deps
    {{ quote(just_executable()) }} _run-parts {{ web_checks }}

# Everything CI's rust job runs. Runs every check and lists the failures.
[group('check')]
rust-check: _web-dist
    {{ quote(just_executable()) }} _run-parts rust-fmt-check clippy test types-check deny

# Everything CI runs except the web end-to-end suite (web-e2e-fast, web-e2e); web goes first because the Rust build embeds web/dist, so a broken web build stops rust-check before its checks run. Runs every check and lists the failures.
[group('check')]
check: _web-deps
    {{ quote(just_executable()) }} _run-parts {{ web_checks }} rust-check actionlint

# Format Rust and web sources
[group('check')]
fmt: rust-fmt web-fmt

# Check Rust and web formatting without rewriting; runs both and lists the failures
[group('check')]
fmt-check:
    {{ quote(just_executable()) }} _run-parts rust-fmt-check web-fmt-check

# Lint the GitHub Actions workflows with actionlint and shellcheck, at the versions CI pins in .github/workflows/actionlint.yml
[group('check')]
actionlint:
    #!/usr/bin/env bash
    set -euo pipefail
    pinned="$(sed -n 's/^ *ACTIONLINT_VERSION: \(.*\)$/\1/p' .github/workflows/actionlint.yml)"
    shellcheck_pinned="$(sed -n 's/^ *SHELLCHECK_VERSION: \(.*\)$/\1/p' .github/workflows/actionlint.yml)"
    command -v actionlint >/dev/null || { echo "actionlint is not installed: get v${pinned} from https://github.com/rhysd/actionlint/releases" >&2; exit 1; }
    installed="$(actionlint -version | sed -n 1p)"
    [ "$installed" = "$pinned" ] || { echo "actionlint ${installed} is installed; CI pins ${pinned}" >&2; exit 1; }
    command -v shellcheck >/dev/null || { echo "shellcheck is not installed, so actionlint would skip its shell checks: install it (apt install shellcheck, or v${shellcheck_pinned} from https://github.com/koalaman/shellcheck/releases)" >&2; exit 1; }
    actionlint

# Every failing test in the latest CI runs for a PR, branch, release tag or commit (default: current branch), with touched and seen-elsewhere flags
[group('check')]
[positional-arguments]
ci-failures *args:
    scripts/ci-failures.sh "$@"

# Checks the CI log parser behind ci-failures against its fixtures
[group('check')]
ci-failures-test:
    scripts/ci-failures-test.sh

# Checks which issues the nightly stress run's issue filer would open for the parser's fixtures (a dry run; GitHub is not queried)
[group('check')]
stress-issues-test:
    scripts/stress-issues-test.sh

# --- misc --------------------------------------------------------------------

# Lines-of-code summary (needs cloc); extra args go to cloc
[group('misc')]
[positional-arguments]
loc *args:
    scripts/loc.sh "$@"

# Remove build output; the next cargo recipe rebuilds web/dist on its own
[group('misc')]
clean:
    cargo clean
    rm -rf web/dist

# Reinstall when package-lock.json is newer than the one npm ci leaves in node_modules (or that is missing)
[private]
_web-deps:
    @[ web/node_modules/.package-lock.json -nt web/package-lock.json ] || (cd web && npm ci)

# build.rs panics without web/dist, and the gateway's asset tests read dist/sw.js. Rebuilds a dist that is
# missing, incomplete or stale: sw.js is the last file a build writes, so a dist without it is unfinished,
# and a source newer than it was edited since. Tests and the mock aren't built into dist and don't count.
[private]
_web-dist: _web-deps
    #!/usr/bin/env bash
    set -euo pipefail
    cd web
    if [ -f dist/index.html ] && [ -f dist/sw.js ]; then
        newer="$(find src public build index.html package-lock.json vite.config.ts svelte.config.js \
            -newer dist/sw.js -type f -not -name '*.test.ts' -not -path 'src/test/*' -print -quit)"
        [ -n "$newer" ] || exit 0
        echo "web/dist is older than $newer: rebuilding"
    fi
    npm run build

# Runs each named recipe in its own `just` process, so its prerequisites still run. A failing recipe does not stop the rest; the failures are listed at the end and the exit status is non-zero if any failed.
[private]
_run-parts +parts:
    #!/usr/bin/env bash
    set -uo pipefail
    trap 'echo "interrupted" >&2; exit 130' INT TERM
    parts=({{ parts }})
    failed=()
    for part in "${parts[@]}"; do
        echo "==> $part"
        if ! {{ quote(just_executable()) }} "$part"; then
            failed+=("$part")
        fi
    done
    if [ "${#failed[@]}" -gt 0 ]; then
        echo "${#failed[@]} of ${#parts[@]} checks failed: ${failed[*]}" >&2
        exit 1
    fi
    echo "all ${#parts[@]} checks passed"
