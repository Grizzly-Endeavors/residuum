# Contributing to Residuum

Thanks for your interest in contributing. This document covers the workflow and standards for getting changes merged.

## Getting Started

1. Fork the repository and clone your fork
2. Install [Rust](https://rustup.rs/) (the version pinned in `rust-toolchain.toml`; rustup installs it — see [Rust Toolchain](#rust-toolchain)) and [Node.js](https://nodejs.org/) (for the web frontend; the supported versions are the `engines` field in `web/package.json`, see [web/CONTRIBUTING.md](web/CONTRIBUTING.md#prerequisites))
3. Build the web frontend first — the Rust binary embeds the built assets, so `cargo build` will fail without them:
   ```bash
   cd web
   npm install
   npm run build
   cd ..
   ```
4. Build and test the Rust project:
   ```bash
   cargo build
   cargo test --quiet
   ```
5. Install the git hooks — they enforce formatting, linting, and tests on every commit:
   ```bash
   .githooks/install.sh
   ```

If you have [`just`](https://github.com/casey/just) installed, `just setup` installs the web dependencies (step 3) and the git hooks (step 5) in one go, and `just` on its own lists everything else: `just check` runs everything CI runs except the web end-to-end suite, `just test memory::` runs one module's tests, and `just web-mock` starts the web UI against a mock API with no backend. Every recipe wraps a command documented here, so `just` is optional.

## Branch & PR Workflow

All changes go through pull requests.

1. Create a feature branch from `main` with a descriptive name
2. Make your changes and commit frequently
3. Push your branch and open a PR against `main`
4. The pre-commit hook is the first quality gate: formatting, clippy, tests, dependency audit, and the web checks all run locally on every commit. CI then runs the full Rust checks on PRs that change Rust code and the full web checks on PRs that change `web/` (see [Quality Gates](#quality-gates)). CI runs on self-hosted runners, so PRs from forks don't run it; a maintainer re-runs the checks locally before merging a contributor's PR.
5. Cross-platform checks (clippy for aarch64 Linux, Windows, and macOS, plus tests on Windows) are opt-in; see below

### Rust Toolchain

The Rust version is pinned in `rust-toolchain.toml`, and rustup installs exactly that version for every build: your machine, CI, and releases. Everyone lints with the same clippy, so code that passes the pre-commit hook passes CI. Bumping the version is its own PR: change the pin, fix whatever new lints the release brings, and commit.

### Cross-Platform Checks

The pre-commit hook only builds for your own platform. `.github/workflows/cross-compile.yml` runs `cargo clippy -- -D warnings` for the other release targets, so code behind another platform's `cfg` is held to the same lints as your own build, and runs the full test suite on a Windows runner, where `cfg(windows)` code actually executes. It runs when:

- the PR carries the `cross-compile` label (adding the label starts a run, and every later push re-runs it)
- the PR changes `Cargo.toml`, `Cargo.lock`, `build.rs`, or `rust-toolchain.toml` (automatic)
- you dispatch it by hand, with or without a PR: `gh workflow run cross-compile.yml --ref <branch>`

Opt in when a change touches anything platform-sensitive: `unsafe` or FFI code, `#[cfg(target_os = ...)]` / `#[cfg(windows)]` / `#[cfg(unix)]` branches, filesystem paths, process spawning, signals, or file permissions. Anything else that slips through gets caught by the release build, which compiles every target.

From Linux, the Windows and aarch64 Linux targets can also be linted locally once installed (`rustup target add x86_64-pc-windows-gnu`): `cargo clippy --target x86_64-pc-windows-gnu --all-targets --all-features -- -D warnings`.

### Branch Naming

Use prefixed branch names:
- `feat/` — new features
- `fix/` — bug fixes
- `refactor/` — code restructuring without behavior changes
- `docs/` — documentation only
- `ci/` — CI/CD changes
- `chore/` — maintenance, dependency updates

## Code Standards

### Quality Gates

Pre-commit hooks run automatically:
- `cargo fmt` — formatting (auto-applied and staged)
- `cargo clippy` — pedantic linting with strict denials
- `cargo test` — tests for the modules touched by the commit
- `cargo deny check` — dependency audit
- When any file under `web/` is staged: Prettier, ESLint and the style lint (Stylelint), `svelte-check` (warnings fail it), and the web unit and component tests (Vitest)

Do not bypass hooks. If a hook fails, fix the issue before committing.

CI repeats these checks and adds four. Its Rust half runs on every pull request that changes Rust code or what the build embeds (`src/`, `tests/`, `assets/`, the Cargo and lint configuration, the generated web types), its web half on every pull request that changes `web/`, and both on release tags. The Rust half runs the whole test suite, not only the touched modules', and the Pebble tests (`just pebble`), which issue certificates from the Let's Encrypt test CA in Docker. It runs the whole web end-to-end, accessibility and visual suite against the mock, in the Playwright container's browser, so the runners need no Chromium of their own (`just web-e2e` runs it locally; `just web-e2e-fast` leaves out the visual comparisons and runs in your own Chromium; see [web/CONTRIBUTING.md](web/CONTRIBUTING.md#testing)). It regenerates the web's TypeScript protocol types from the Rust types and fails when `web/src/lib/generated/` differs from what is committed, so after changing an exported Rust type run `just types` and commit the result (`just types-check` runs the same check as CI). It also reports web test coverage in the job summary, with no threshold.

### Style

- **Error messages**: lowercase, no trailing period, include context (`"failed to parse config at {path}"` not `"parse error"`)
- **No silent failures**: every error must be visible to the user — not buried in debug logs
- **Comments**: explain *why*, not *what*
- **Visibility**: private-first — only add `pub(crate)` or `pub` when needed
- **Async-first**: sync only for trivial or CPU-bound work

### Testing

Tests are required for new functionality. Unit tests go in `#[cfg(test)] mod tests` at the bottom of the file. Integration tests go in `tests/`.

A test waits on the event it checks, never on elapsed time: a duration that holds on an idle machine fails when several builds share it. A flaky test is fixed by making it wait on an event the code exposes, adding that signal to the code when there is none, never by widening a timeout, adding a sleep, or rerunning past it. `just stress <filter>` runs the tests as two concurrent copies with CPU burners alongside (`STRESS_COPIES`, `STRESS_ROUNDS` and `STRESS_BURN` tune it), and a test you add or fix must pass it.

#### Waiting in tests

Test code waits through `crate::testing`, never with `tokio::time::sleep`, `tokio::time::timeout`, `std::thread::sleep` or wiremock's `set_delay`. A source-scan test (`testing::source_scan`) fails on any of those outside `src/testing/`; the pre-commit hook runs it on every Rust commit. The waits that predate the rule are counted per file in `src/testing/wall_clock_backlog.rs`, and the counts only go down: converting a file's waits means lowering its entry, and the hook refuses a commit that adds one.

- **Something will arrive on a channel or watch.** `testing::wait::next`, `next_where` (everything received up to the match) or `watch_until`. For state with no channel to wait on, `wait::until` polls a check. Each runs under `wait::HANG_GUARD`, the suite's only deadline, which exists to stop a hung test and names what it was waiting for; never shorten it or add another.
- **A state must last while the test looks at it** (an agent mid-turn, a session queued behind another). Hold the call that would end it at a closed `testing::gate::Gate`: `gate.until_held(1)` proves the call is waiting, the test looks, then `gate.release(1)`. The model has gated versions: `testing::model::GatedModel` (an HTTP model, either in front of a wiremock server or with a fixed reply; the hub test fixture puts one in front of every agent's mock), `GatedProvider` in process, and the scripted stream server's `Step::Hold`.
- **Background work must have happened.** Wait on the signal the code gives for it. When there is none, add one to the code: a test that needs to know something finished is usually a sign users and diagnostics need to know too.
- **Something must not happen.** Only after a positive sync point that proves it would have happened by now: a bus barrier (`wait::bus_barrier`, which publishes a marker and waits for it; the broker hands a message to every subscriber at once, so once a subscriber has a later message, every subscriber has the earlier one), a sentinel file written after the cause (file notifications arrive in order), a stop (an agent's watcher drains before `stop` returns), or a pending count back at zero. Then check with `wait::drain` or the current state.
- **The code under test is timer-driven and has no real I/O** (no sockets, child processes or file watchers). Run it on a paused clock (`#[tokio::test(start_paused = true)]`) and move time with `testing::clock::elapse`; `clock::within` checks that something does not complete within a window. Both refuse to run on a real clock.

`clippy.toml` exempts `unwrap_used`, `expect_used`, `panic`, and `dbg_macro` inside `#[cfg(test)]` modules and `#[test]` functions, so unit tests use unwrap, expect, panic, and `dbg!` with no suppression. Do not add `#[expect(clippy::unwrap_used)]` (or `expect_used`, `panic`, `dbg_macro`) on that code: the lint is already exempt, the expectation never fires, and `-D warnings` fails the build.

`clippy::tests_outside_test_module` is denied and is not controllable from `clippy.toml`. Integration tests under `tests/` need `#[expect(clippy::tests_outside_test_module, reason = "...")]`. A helper at the top level of a `tests/*.rs` file — not inside `#[cfg(test)]`, and not itself a `#[test]` — still needs its own `#[expect]` when it uses unwrap, expect, panic, or `dbg!`.

### Lint Denials

These are denied in `Cargo.toml` and will not be relaxed without explicit approval:
- `unsafe_code` (`deny`, not `forbid`, so a real FFI boundary can carry `#[expect(unsafe_code, reason = "...")]` on that item)
- `unwrap_used`, `expect_used`, `panic`, `todo`, `unimplemented`
- `missing_errors_doc`, `missing_panics_doc`, `must_use_candidate`, `print_stderr`
- `indexing_slicing`, `string_slice`, `dbg_macro`, `exit`

`unwrap_used`, `expect_used`, `panic`, and `dbg_macro` are exempt inside `#[cfg(test)]` modules and `#[test]` functions via `clippy.toml`; see Testing. Every other denial is under `[lints]` in `Cargo.toml`.

Any `#[allow]` must be `#[expect]` with a reason string.

## Releases

Releases use [CalVer](https://calver.org/) with the format `YYYY.0M.0D` (e.g., `v2026.03.02`). If multiple releases happen on the same day, a suffix is added: `v2026.03.02-2`.

Releases are automated — pushing a tag matching this format triggers the CI pipeline, which builds cross-platform binaries and creates a GitHub release. Cargo.toml version is not tied to release tags.

## Architecture

See [docs/systems-usage/](docs/systems-usage/) for how each subsystem works — it is the authoritative reference, kept current with the code. [docs/design-philosophy.md](docs/design-philosophy.md) covers the principles behind those choices, and [docs/](docs/) describes the rest of the documentation layout.

## License

By contributing, you agree that your contributions will be licensed under the MIT License.
