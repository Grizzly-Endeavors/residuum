# Contributing to Residuum

Thanks for your interest in contributing. This document covers the workflow and standards for getting changes merged.

## Getting Started

1. Fork the repository and clone your fork.
2. Install [Rust](https://rustup.rs/) (rustup picks up the version pinned in `rust-toolchain.toml`) and [Node.js](https://nodejs.org/) (the versions in `web/package.json`'s `engines`; see [web/CONTRIBUTING.md](web/CONTRIBUTING.md#prerequisites)).
3. Build the web frontend. The `residuum` binary embeds it, so `cargo build` fails without it (the member crates under `crates/` build without it):
   ```bash
   cd web && npm install && npm run build && cd ..
   ```
4. Build and test:
   ```bash
   cargo build
   cargo test --quiet
   ```
5. Install the git hooks:
   ```bash
   .githooks/install.sh
   ```

With [`just`](https://github.com/casey/just), `just setup` does steps 3 and 5, and `just` lists every recipe. Every recipe wraps a command you can run by hand, so `just` is optional. The ones you'll reach for:

- `just check` — everything CI runs except the web end-to-end suite, listing every failure
- `just test <filter>` / `just test-crate <crate> <filter>` — root tests / one member crate's tests
- `just web-mock` — the web UI against a mock API, no backend needed
- `just ci-failures` — every failure in your branch's latest CI runs

Rust builds lease a warm target dir from a pool shared by every worktree; see [docs/runbooks/build-dirs.md](docs/runbooks/build-dirs.md). Use `just clean` rather than `cargo clean`.

## Branch & PR Workflow

1. Branch from `main` with a prefixed name: `feat/`, `fix/`, `refactor/`, `docs/`, `ci/` or `chore/`.
2. Commit often. The hooks check each commit and push ([Quality Gates](#quality-gates)).
3. Push and open a PR against `main`, with `closes #N` in the PR body for each issue it closes.
4. Enqueue it with `gh pr merge <n> --auto` (or "Merge when ready").

### Merging

`main` changes only through the GitHub merge queue. The ruleset in [`.github/rulesets/main.json`](.github/rulesets/main.json) refuses direct and force pushes, and merges a PR only through the queue once its `CI gate` check passes. No approvals are required.

- The queue merges your PR onto the tip of `main` together with any PRs ahead of it, runs the full suite on that batch, and merges the batch only when it passes.
- On the PR itself, CI runs two early warnings that don't block it: the Windows clippy and the visual comparisons. Fix a Windows clippy failure before enqueueing.
- Put `closes #N` in the PR body. The queue writes the merge commit message, and a keyword in a comment closes nothing.
- Check status with `gh pr view <n> --json state,autoMergeRequest,statusCheckRollup`, or see the queue at https://github.com/Grizzly-Endeavors/residuum/queue/main.
- A PR ejected from the queue (failed batch, conflict, or a four-hour timeout) stays out until re-enqueued. Fix the cause (`just ci-failures <n>`), push, and enqueue again.
- CI runs on self-hosted runners, so a fork's PR runs no checks until a maintainer enqueues it. Maintainers: read the whole diff, including `.github/`, before enqueueing a fork's PR.
- Admins can merge past the queue with `gh pr merge <n> --merge --admin`, for emergencies such as broken CI.

Changing the ruleset itself: [docs/runbooks/ci.md](docs/runbooks/ci.md).

### Rust Toolchain

`rust-toolchain.toml` pins the Rust version for every build: your machine, CI and releases. Code that passes the hooks lints the same in CI. A toolchain bump is its own PR, with fixes for any new lints.

### Cross-Platform Checks

Releases build Linux x86_64 and aarch64, macOS aarch64 and Windows x86_64 (`x86_64-pc-windows-gnu`). The hooks only build for your own platform, so:

- Every Rust PR runs clippy for the Windows target as an early warning, and every merge queue batch runs it as a gate. `just cross-clippy` runs it locally.
- `cross-compile.yml` adds aarch64 Linux and macOS clippy and the full test suite on Windows. It gates batches that change `Cargo.toml`, `Cargo.lock`, `build.rs`, `rust-toolchain.toml`, a member's `Cargo.toml` or the workflow itself. On a PR it reports without gating when one of those changes, when the PR has the `cross-compile` label, or when dispatched with `gh workflow run cross-compile.yml --ref <branch>`.
- Add the `cross-compile` label when a change touches `unsafe`/FFI, `cfg(target_os)`/`cfg(windows)`/`cfg(unix)` branches, filesystem paths, process spawning, signals or permissions.
- To run code on real Windows, use the VM harness: [docs/runbooks/windows-harness.md](docs/runbooks/windows-harness.md).

## Code Standards

### Quality Gates

Do not bypass the hooks: `--no-verify` is not allowed on `git commit` or `git push`. If a hook fails, fix the issue.

**pre-commit**, on what is staged:
- `cargo fmt` and Prettier, applied and re-staged. A file with both staged and unstaged edits is refused; stage it whole or stash the rest.
- `cargo deny check` when a manifest is staged (needs `cargo install --locked cargo-deny`).
- Blocks `dbg!()`, `#[allow(...)]` and growth of the wall-clock backlog ([Waiting in tests](#waiting-in-tests)).
- Runs the fixture tests for `scripts/ci-failures*`, `scripts/stress-issues*` and `scripts/affected-packages*` when those are staged.

**pre-push**, on each pushed commit as pushed (not merged with `main`):
- When Rust changes: clippy over the affected packages and their dependents, the tests for the code the push touches, the source scans, the generated-types check (`scripts/check-generated-types.sh`), and `cargo shear` for unused dependencies (`just shear` installs the version CI pins).
- When `web/` changes (Markdown excepted): ESLint, Stylelint, `svelte-check`, and the Vitest tests that import a changed file.
- It prints which tests it chose, runs every check even after one fails, and ends with a summary.

The hook runs the tests of the code you changed, not the whole suite. When you change something other modules depend on (a shared helper, a trait), run their tests too, or `cargo test --quiet`.

| Stage | Runs on | What it runs |
|-------|---------|--------------|
| pre-commit | every commit | Formatting, `cargo deny` for manifests, the `dbg!` / `#[allow]` / backlog blocks |
| pre-push | every push | Clippy, affected tests, generated types, `cargo shear`; web lint, types and related tests |
| PR (`ci.yml`) | every push to a PR | Early warnings only: Windows clippy, visual comparisons. `CI gate` passes once the diff is read |
| Merge queue (`ci.yml`) | every batch | The full suite: fmt, host and Windows clippy, rustdoc, `cargo deny`, `cargo shear`, all tests, Pebble tests, generated types, crate isolation; web format, lint, types, unit tests on Node 22 and 24, build, end-to-end; the Docker image when `docker/` changes; `cross-compile.yml` when dependencies or the toolchain change; actionlint when a workflow changes |
| Nightly (`nightly-stress.yml`) | `main`, 07:23 UTC | The Rust suite under CPU load, the end-to-end specs CPU-throttled; files an `investigate` issue per failing test. Gates nothing |
| Release (`release.yml`) | a `v*` tag | The merge queue's Rust and web checks beside the release builds; nothing publishes unless both pass |

`CI gate` is the only required check. After a red run, `just ci-failures` lists every failure with `touched` and `likely flake` flags; [docs/runbooks/ci.md](docs/runbooks/ci.md) explains them.

### Style

- **Error messages**: lowercase, no trailing period, with context (`"failed to parse config at {path}"`, not `"parse error"`)
- **No silent failures**: every error is visible to the user, not buried in debug logs
- **Comments**: explain *why*, not *what*
- **Visibility**: private-first; add `pub(crate)` or `pub` only when needed
- **Async-first**: sync only for trivial or CPU-bound work

### Testing

Tests are required for new functionality.

- **Unit tests** go in `#[cfg(test)] mod tests` at the bottom of the file.
- **Integration tests** are modules of the one test binary in `tests/it/`: add a file and a `mod` line in `tests/it/main.rs`. Each module's doc comment carries one `//! covers: <package>, …` line, which the pre-push hook uses to pick tests. Modules share a process, so a test never changes environment variables, the current directory or the global tracing subscriber. Member crates have no `tests/` directory; tests spanning several crates are `it` modules.
- **Filters** match a test's path, and a member crate's paths start below the crate: `residuum-memory`'s search tests are `search::tests::…`, so run them with `just test-crate memory search::`, not `just test memory::`.
- **Shared helpers** live in the lowest crate that can hold them, in a `testing` module behind that crate's `test-support` feature.

#### Waiting in tests

A test waits on the event it checks, never on elapsed time: a duration that holds on an idle machine fails when several builds share it. Fix a flaky test by making it wait on an event the code exposes (adding the signal when there is none), never by widening a timeout, adding a sleep, or rerunning.

Test code waits through `residuum_core::testing::{wait, gate, clock}` and `residuum_bus::testing`, never `tokio::time::sleep`, `tokio::time::timeout`, `std::thread::sleep` or wiremock's `set_delay`. A source scan (`testing::source_scan`) enforces this. Files that predate the rule are counted in `src/testing/wall_clock_backlog.rs`; counts only go down.

| You need… | Use |
|---|---|
| Something to arrive on a channel or watch | `wait::next`, `next_matching`, `next_where`, `watch_until`; `residuum_bus::testing::next_event` for a bus subscriber; `wait::until` to poll state with no channel |
| A state to last while you look at it | Hold the call at a `gate::Gate`: `until_held(1)`, look, `release(1)`. `GatedModel` / `GatedProvider` and the scripted stream's `Step::Hold` gate the model |
| Background work to have happened | The signal the code gives for it; add one if there is none |
| Something *not* to happen | A positive sync point first (`bus_barrier`, a sentinel file, a stop, a pending count at zero), then `wait::drain` or the current state |
| Timer-driven code with no real I/O | `#[tokio::test(start_paused = true)]` with `clock::elapse`; `clock::within` to check something doesn't complete |
| A child-process timeout to fire | Real clock until the child shows it started, then `tokio::time::pause()` and `advance` past the timeout |

Every wait runs under `wait::HANG_GUARD`, the suite's only deadline; don't shorten it or add another. Don't leave a paused clock running on its own while a child process runs: on Windows a blocking pipe read holds the clock still.

Write fake executables with `residuum_core::testing::script::write_executable`. Writing and executing a script in-process fails with "Text file busy" when another test thread forks at the wrong moment; the source scan rejects `chmod` and execute-bit modes in test code.

A test you add or fix must pass `just stress <filter>` (`-p residuum-<crate>` for a member) before you push. The nightly run is the backstop: each failure there becomes an `investigate` issue titled `Nightly stress: <test>`. A failure under load only means the test waits on time.

#### Lints in tests

- `clippy.toml` allows `unwrap`, `expect`, `panic` and `dbg!` in `#[cfg(test)]` and `#[test]` code. Don't add `#[expect]` for them there: it's never fulfilled and fails the build.
- Code behind `#[cfg(any(test, feature = "test-support"))]` is production code to `cargo clippy --all-features`, so it needs `#[expect(clippy::…, reason = "test-support helper: …")]`.
- `tests/it/` modules need `#![expect(clippy::tests_outside_test_module, reason = "...")]`, and their top-level helpers need their own suppressions.

### Lint Denials

The lint table is `[workspace.lints]` in the root `Cargo.toml`; every crate inherits it with `[lints] workspace = true`. Clippy pedantic is on, with a long deny list. The ones you'll meet most:

- `unsafe_code` (`deny`, not `forbid`, so a genuine FFI boundary can carry a scoped `#[expect]`)
- `unwrap_used`, `expect_used`, `panic`, `todo`, `unimplemented`, `dbg_macro`, `exit`
- `indexing_slicing`, `string_slice`
- `missing_errors_doc`, `missing_panics_doc`, `must_use_candidate`
- `print_stderr` (residuum is a daemon; diagnostics go through `tracing`)
- `allow_attributes`: use `#[expect(lint, reason = "...")]`, never `#[allow]`

Lint levels are not relaxed without the maintainer's approval.

## Workspace layout

One Cargo workspace. The root `Cargo.toml` is the workspace manifest and the `residuum` package: the library, the binary, `build.rs`, the embedded `web/dist` and the integration tests. The lower layers are member crates in `crates/<dir>/`, named `residuum-<dir>`. `cargo build`, `cargo test` and `cargo clippy` from the root cover every package; `-p residuum-<dir>` narrows to one.

### The crates

| crate | holds | depends on (besides core and config) |
|---|---|---|
| `residuum-core` | filesystem and spawn utilities, `time`, diagnostics, `build_info`, `process`, shared test helpers | — |
| `residuum-config` | config loading and resolution, the secret store, `agent_keys`, `WorkspaceLayout`, `channels` | core only |
| `residuum-skills` | the skill index, `SKILL.md` parsing, skill state | — |
| `residuum-inference` | providers, failover, embeddings, the System One client, tool-call result types | — |
| `residuum-tracing-service` | OTEL trace export, sanitizing, bug reports | — |
| `residuum-memory` | observer and reflector, search, vector and episode stores, the wiki index | inference |
| `residuum-mcp` | the MCP client and server registry | inference |
| `residuum-net` | `tunnel`, `remote_access`, `pairing`, `workbench`, shared request guards | — |
| `residuum-bus` | the topic broker and the payloads it carries | inference, memory |
| `residuum-workspace` | identity files, bootstrapping, team files, the change feed, `inbox` | bus, inference, mcp |
| `residuum-checkpoints` | the workspace checkpoint history | bus, workspace |
| `residuum-protocol` | WebSocket frames and their conversions from bus events | bus, inference, workspace (and not core or config) |
| `residuum-proactivity` | `pulse` (heartbeats) and `actions` (scheduled actions) | bus |
| `residuum-notify` | notification channels (ntfy, webhook, inbox, native macOS/Windows) and their router | bus, workspace |
| `residuum-agent` | the turn loop, context assembly, the `Tool` trait and registry, `subconscious` | bus, inference, memory, mcp, skills, workspace |
| `residuum-background` | agent sessions, agent messaging, the team router, the session tool factory trait | agent and most layers below |
| `residuum-a2a` | the A2A listener, caller keys, agent cards, outbound client, `diagnostics_dispatch` | agent, background, proactivity, net, … |
| `residuum-tools` | the agent's tools, their registration, the main and session registries | a2a, background, agent, … |
| `residuum-interfaces` | the Discord, Telegram and Teams adapters, webhooks, Teams app setup | background, net, … |
| `residuum-hub` | the agent host, the hub's HTTP app and web routes, `launch`, `update`, `features`, `config_watcher` | interfaces, a2a, protocol, … |
| `residuum-gateway` | a running agent's event loop, startup, reload, routes and WebSocket; `GatewayLauncher` | every other member |
| `residuum` (root) | the binary, `commands`, `daemon`, `wiring`, `logging`, `setup_wizard`, `BUILD_INFO`, the facade, the source and manifest scans | every member |

Each crate's `Cargo.toml` is the full, exact list of what it depends on.

### Dependency rules

- **Dependencies point down.** No member depends on the root or a crate above it. Shared code lives in the lowest crate that uses it. When a lower crate needs something from above, it takes a trait or a parameter (inference's `NoticeSink`, which the bus implements).
- **No cycles, dev-dependencies included.** Cargo accepts a dev-dependency cycle silently and compiles the crate twice, so its types stop unifying.
- **`test-support` is enabled only from `[dev-dependencies]`**, never from `[dependencies]`, a target table or a `[features]` entry, so no test helper reaches the release binary. (A crate's own `test-support` may turn on a dependency's, as inference's turns on `residuum-core/test-support`.) A package that depends on a crate with `test-support` turns it on in its `[dev-dependencies]`.
- **`test-support` code compiles two ways**: with the feature in other crates' tests, and with `cfg(test)` in its own. An external crate it needs is declared both as an optional dependency the feature enables and in `[dev-dependencies]` (see config's `tempfile`).
- **Every external dependency is declared once**, in `[workspace.dependencies]`, and named `{ workspace = true }` in each package, adding features where needed. Members are declared with a path and a version (`deny.toml` rejects a path without one). Target-specific dependencies go in each crate's target table.
- **Every member** inherits `version`, `edition`, `license` and `publish`, sets `[lints] workspace = true`, is named `residuum-<dir>` with a lowercase-and-hyphens `<dir>`, and sets no `[lib] name` (the default log filter `residuum=debug` matches library names starting with `residuum_`).
- **A test binary that builds an HTTP client installs the TLS provider**: reqwest panics with "No provider set" otherwise. Such a crate includes `crates/core/src/testing/crypto.rs` by `#[path]` under `#[cfg(test)]`, dev-depends on `ctor` and `rustls`, and lists them in `[package.metadata.cargo-shear] ignored`, because `cargo shear` can't see a use in a file included from another crate.
- **Dependencies are used or removed.** `cargo shear` fails on unused entries and unlinked source files. A use it can't see goes in `[package.metadata.cargo-shear] ignored` with a comment saying why.
- **Files outside a crate.** A file a member embeds from outside its own directory and outside `assets/` (`residuum-net` embeds `web/src/styles/tokens.css`; `residuum-hub` embeds `web/app-shell-csp.txt` and, in a test, `.github/workflows/release.yml`) needs an entry in `RUST_PATHS_RE` in `.github/workflows/ci.yml`, or CI runs no Rust checks for a change to it alone, and a `classify_path` arm in `scripts/affected-packages.sh` naming that member, so the pre-push hook runs its tests. Two `testing::source_scan` tests fail, saying what to add, when either is missing.

### Where the rules are enforced

- `testing::manifest_scan` (a root test) reads every manifest and fails, naming the file and key, on a broken naming, inheritance, `test-support` or cycle rule, or when `scripts/affected-packages.sh --graph` disagrees with the manifests. The pre-push hook runs it when a manifest changes.
- `testing::source_scan` covers `src/`, `tests/` and every member: test waits, executable scripts, bare spawns, build-version reads outside `src/lib.rs`, files embedded from outside a crate, and `testing::source_scan::unused_pub`, which fails on a member's `pub` item nothing outside its own crate's tests names (rustc never warns about those).
- The merge queue's `rust-isolation` job (`scripts/check-crate-isolation.sh`) checks each member alone, and the root without dev-dependency features, so a crate that compiles only because another enables a feature for it fails.

A new member needs: its `Cargo.toml` written to these rules, an entry in `[workspace.dependencies]`, dependency lines in the crates that use it, its alias in `src/lib.rs`, its line in `docs/systems-usage/hub.md` ("Logging"), and the two path entries above for any file it embeds from outside.

### The root facade

`src/lib.rs` gives each member crate, or each module of one the root names on its own, one alias: `pub use residuum_memory as memory;`, `pub use residuum_config::agent_keys;`.

- A name at the root is either a local module or exactly one alias. Aliases name a whole crate or a whole module, never a single item, and nothing glob-imports another crate (a local item silently shadows a glob).
- Root code names members' items through the aliases (`crate::memory::…`, `crate::agent_keys::…`). Core's `testing` has no alias because the root has its own `testing`, so root tests write `residuum_core::testing::…`.
- Member crates name each other by crate (`residuum_config::Config`).
- Every alias is `pub`. A module only tests use gets no alias; tests name its crate directly (`residuum_net::workbench`).

### Public API

1. A crate's API is what its `lib.rs` exports. A module is public only when callers name it as a namespace (`residuum_config::paths`) or the language needs a path (a serde `with` module); inside it, only items used across the boundary are `pub`.
2. Prefer moving code to the crate that uses it over widening an item for one caller.
3. No blanket `pub` to get the build through.
4. Types with invariants keep private fields and offer methods. Plain data types (serde payloads, config, protocol and status types) have `pub` fields.
5. An item that moves crates keeps its visibility and is not re-exported from its old home; callers import it from the new path.
6. A `#[cfg(test)]` item another crate's tests use becomes `#[cfg(any(test, feature = "test-support"))] pub`. Production code is never put behind the feature.
7. A newly public item gets `# Errors`, `# Panics` and `#[must_use]` as the lints ask.

When the compiler says an item is private to the crate that needs it, take the first that applies:

1. **Move the caller's code in** when it only uses the item to reach the crate's internals; it becomes a function of the lower crate and the item stays private.
2. **Make a field `pub`** on a plain data type.
3. **Add a getter** for a field of a type with invariants; writes go through a method that keeps the invariant.
4. **Make the item `pub`** at the narrowest path, re-exported from the crate root or a public namespace module.
5. **A test-only item** follows rule 6 above.

A PR that widens items lists each one in its description: path, kind, which step applied, and its callers outside the crate.

## Releases

Releases use [CalVer](https://calver.org/), `YYYY.0M.0D` (e.g. `v2026.03.02`), with `-2`, `-3` for same-day follow-ups. Pushing a matching tag builds the cross-platform binaries and creates the GitHub release. The `Cargo.toml` version is independent of release tags.

## Architecture

[docs/systems-usage/](docs/systems-usage/) is the authoritative reference for how each subsystem works. [docs/design-philosophy.md](docs/design-philosophy.md) covers the principles behind it, and [docs/](docs/) describes the rest of the documentation. Maintainer operations are in [docs/runbooks/](docs/runbooks/).

## License

By contributing, you agree that your contributions will be licensed under the MIT License.
