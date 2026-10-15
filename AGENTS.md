# AGENTS.md | Residuum - Personal Agent Framework

`CLAUDE.md` carries these same rules. This paragraph and the title are the only difference. Change them together.

## Key References

- [Design Philosophy](./docs/design-philosophy.md)
- [Systems Usage](./docs/systems-usage/) — authoritative reference for how systems work today
- [Guides](./docs/guides/) — task-oriented walkthroughs
- [Design](./docs/design/) — designs for work that is not built yet
- [Hub](./docs/systems-usage/hub.md) — one process hosts every agent; `~/.residuum/` holds `hub/`, the shared `team/`, and one directory per agent
- [Workspace layout](./CONTRIBUTING.md#workspace-layout) — the root package `residuum` and the member crates under `crates/`: what each holds, the dependency rules, the root facade and the public API policy

Superseded documents live in [`docs/archive/`](./docs/archive/); they record history and do not describe current behavior.

**Web interface:** The Residuum web UI lives in `residuum/web/` (Svelte 5 SPA). See `web/CLAUDE.md` for details. **Do not confuse with `relay/web/`**, the agent-residuum.com site (marketing pages, Cloud sign-in and the account dashboard) — it lives in a separate repository, not this one.

## Build & Quality Gates

### Workspace

The repository is one Cargo workspace. The root `Cargo.toml` is both the workspace manifest and the `residuum` package (the library and the `residuum` binary: gateway, hub, interfaces, bus, agent, tools and everything else from the bus up, plus `build.rs`, the embedded `web/dist` and `tests/`). The lower layers are member crates in `crates/<dir>/`, each named `residuum-<dir>`: `core` (`util`, `time`, the diagnostic types, the shared test helpers), `config` (with `agent_keys` and `WorkspaceLayout`), `skills`, `inference`, `tracing-service`, `memory`, `mcp` and `net` (`tunnel`, `remote_access`, `pairing`, `workbench`). Dependencies point down only: no member depends on the root, and the root depends on every member. Plain `cargo build`, `cargo test` and `cargo clippy` from the root cover every package (`default-members`), and `-p residuum-<dir>` narrows a command to one. Root code names a member's items through the aliases in `src/lib.rs` (`crate::memory::…`, `crate::agent_keys::…`); member crates name each other by crate (`residuum_config::Config`). Only the root has a build script, so member crates build and test without `web/dist`. `CONTRIBUTING.md`, "Workspace layout", has the crates, their dependency rules, the facade's rules and the public API policy; `src/testing/manifest_scan.rs` enforces the manifest rules. A file a member embeds from outside its own directory and outside `assets/` (`web/src/styles/tokens.css`, embedded by `crates/net/src/browser_landing.rs`) must be listed by hand in `RUST_PATHS_RE` in `ci.yml` and in the pre-push hook, in `classify_path` and in the hook's `plan_rust_tests`; `CONTRIBUTING.md`, "Workspace layout", has the details.

### Git Hooks

The git hooks in `.githooks/` (installed with `.githooks/install.sh` or `just hooks`) enforce the quality gates in two stages: the fast checks on every commit, the slow ones once per push.
- **pre-commit**: formats what is staged, re-staging only the paths already staged: `cargo fmt --all` when Rust is staged, Prettier on the staged web files. It refuses a formatter change to a file that also has unstaged edits, so the author stages the whole file or stashes the rest. When a manifest (`*.toml`, `Cargo.lock`) is staged it also runs `cargo deny check`. It blocks three things outright in the staged diff: a `dbg!()` call, an `#[allow(...)]` attribute (use `#[expect(lint, reason = "...")]` instead so stale suppressions warn), and growth of `src/testing/wall_clock_backlog.rs`. When any `scripts/ci-failures*` or `scripts/stress-issues*` file is staged it runs the `just ci-failures` log parser's fixtures and its merge queue batch selection (`scripts/ci-failures-test.sh`) and a dry run of the nightly stress issue filer over them (`scripts/stress-issues-test.sh`). When any `scripts/affected-packages*` file is staged it runs `scripts/affected-packages-test.sh`, which checks the package selection behind the pre-push hook and the CI `changes` job against a scratch workspace.
- **pre-push**: checks each pushed commit as pushed, without merging `main` into it: a conflict with `main` shows on the pull request, and the merge queue checks the merged result. A commit is checked in place only when it is the checkout's HEAD and the working tree has no uncommitted or untracked changes; any other commit is checked in the stable tree at `target/pre-push/tree` (sharing this checkout's `web/node_modules`), or in a temporary worktree of that commit when another push holds the stable tree or it cannot be prepared. The hook fetches `origin/main` to measure a new branch's changes against it. A commit checked in place builds into the checkout's own target directory (`CARGO_TARGET_DIR` when set, else `target/`), reusing the artifacts the checkout's cargo runs left there; a commit checked in a temporary tree builds into `target/pre-push`, which keeps it from replacing the checkout's artifacts; `target/pre-push` keeps its own builds from push to push, and the stable tree keeps the modification times of unchanged files, so Cargo reuses them. When the pushed commits change what the Rust checks run on (the paths for which the `changes` job of `.github/workflows/ci.yml` runs the Rust checks: `src/`, `tests/`, `assets/`, `crates/`, the Cargo, clippy, rustfmt and deny configuration, `build.rs`, the generated web types and the scripts and workflows those checks use), it runs `cargo clippy --all-targets --all-features -- -D warnings` over the packages the changed paths touch and the packages that depend on them (`scripts/affected-packages.sh --affected`; the whole workspace for a change to the root manifests, toolchain, lint or deny configuration, `.cargo/`, `assets/`, `tests/fixtures/`, or a `crates/` path outside a member), the tests for the code the push touches, and `scripts/check-generated-types.sh`, which regenerates the web app's TypeScript protocol types into a scratch directory and fails on any difference from the index. The tests come from the changed paths, and the hook prints what it chose. A change under `src/<module>/` or in `src/<module>.rs` runs that module's root unit tests (one `cargo test --quiet --no-fail-fast -p residuum --lib -- <module>:: ...` run with a filter per touched module, and `--bins` too when `src/main.rs` declares the module); a change to `src/lib.rs` or `src/main.rs` runs every root unit test, lib and bin; a change under `tests/it/` runs the `it` binary, which holds every integration test except `ts_export`; a change to `tests/ts_export.rs` runs that test; a change under `tests/fixtures/` or `assets/` runs every unit test of every package and every root integration test. A change to a member crate under `crates/<dir>/` runs that crate's tests (`--lib --tests`) and those of every member crate that depends on it; a change to a member's `Cargo.toml`, to a root manifest (`Cargo.toml`, `Cargo.lock`), `rust-toolchain.toml`, `clippy.toml`, `rustfmt.toml`, `deny.toml`, `.cargo/`, or a `crates/` path outside a member runs the `testing::manifest_scan` test. Integration tests other than `ts_export` are built only when a `tests/` path changed; the generated-types check builds `ts_export` on every Rust push. The source scans (`testing::source_scan`) run on every push that changes Rust. A change to `build.rs`, the generated web types, or the scripts and workflows the checks use adds no tests of its own. A change to a file the Rust tests read by path (`web/app-shell-csp.txt`, `.github/workflows/release.yml`) runs every root unit test; a change to `web/src/styles/tokens.css`, which `crates/net/src/browser_landing.rs` embeds, runs `residuum-net`'s tests. When the selection script is missing or fails, clippy checks the whole workspace and the member tests cover every member other than `residuum`. CI runs the whole suite, so a test in a module the push did not touch fails there, not in the hook. The `ts_export` integration test writes the generated types to a scratch directory, so a push never rewrites `web/src/lib/generated`. When the pushed commits change `web/` (Markdown under `web/` does not count), the Rust checks rebuild `web/dist` first unless the stable tree already holds a dist built from the same web sources, and the hook runs ESLint and the style lint, `svelte-check`, and the web unit and component tests (Vitest) that import a changed file (`vitest related`); a change to `web/package.json`, the lockfile, the Vite, Vitest, Svelte or TypeScript configuration, the component test setup file, the hang guard (`web/src/test/guard.ts`) or the service worker plugin (`web/build/service-worker.ts`) runs the whole web suite. Every check runs even after one fails, and a summary at the end lists what failed. A push that only deletes a branch, or changes neither Rust nor `web/`, runs no checks.
- **commit-msg**: advisory checks on the subject line — fails only on a subject under 10 characters, and warns on over-72-character subjects, a trailing period, or a missing conventional prefix.

Bypassing a hook with `--no-verify` is **FORBIDDEN**, on `git push` as on `git commit`.

### What Runs Where

| Stage | Runs on | What it runs |
|-------|---------|--------------|
| pre-commit | every commit | Formatting of what is staged, `cargo deny` when a manifest is staged, the `dbg!()` / `#[allow]` / wall-clock-backlog blocks, the CI log parser and stress issue filer fixtures and the package selection test when those are staged |
| pre-push | every push, as pushed (not merged with `main`) | Clippy, the tests of the code the push touches and the generated-types check when Rust paths change; ESLint, Stylelint, `svelte-check` and Vitest (the tests that import a changed file) when `web/` changes |
| PR lane (`ci.yml`, `pull_request`) | every push to a pull request | Rust (when Rust paths change): fmt, host clippy, Windows clippy, `cargo deny`, `cargo test`, generated types. Clippy and the tests cover the affected packages only: the packages the change touches and the packages that depend on them, as `scripts/affected-packages.sh` selects them, or the whole workspace when it cannot select them. Fmt, `cargo deny` and the generated types always cover the whole workspace. Web (when web paths change): format, lint, types, unit tests on Node 22 and 24, build, the four end-to-end shards, and the visual comparisons, which never fail and report in one PR comment. actionlint when a workflow changes. `CI gate` sums them up |
| Merge queue lane (`ci.yml`, `merge_group`) | every batch the merge queue builds, merged onto `main` | The PR lane's checks without the visual comparisons, with clippy and the tests over the whole workspace rather than the affected packages, plus the Pebble tests, plus the crate isolation job (`scripts/check-crate-isolation.sh` checks each member crate alone and the root library and binary without dev-dependency features, so a crate that compiles only because another crate turns on a feature for it, or production code that uses a `test-support` item, fails here), plus `cross-compile.yml` (aarch64 Linux and macOS clippy, the tests on Windows) when the batch changes `Cargo.toml`, `Cargo.lock`, `build.rs`, `rust-toolchain.toml`, a member's `crates/*/Cargo.toml` or `.github/workflows/cross-compile.yml`. `CI gate` must pass for the batch to merge |
| Nightly (`nightly-stress.yml`) | 07:23 UTC on `main`, or by hand | The Rust suite unloaded, then as two concurrent copies under CPU load for three rounds; every end-to-end spec three times with the CPU throttled. Each failing test becomes an `investigate` issue, and a run whose job timed out gets one saying it did not finish. It gates nothing |
| Release (`release.yml`) | a `v*` tag | The merge queue lane's Rust and web checks (no visual comparisons, no stress; the Rust checks cover the whole workspace and include the crate isolation job) beside the release builds; nothing is published unless both pass |

`CI gate` is the only required status check. It passes when every job it needs succeeded or was skipped because its files did not change, so a pull request that touches neither Rust nor web code still merges. `cross-compile.yml` also runs on a pull request with the `cross-compile` label or a dependency or toolchain change, where it reports without gating (see Cross-Platform Targets).

### Local Tasks

The `justfile` wraps the commands above; `just` lists the recipes. `just check` runs everything CI runs except the web end-to-end suite (web checks, then fmt, clippy, tests, and `cargo deny`, then actionlint over the workflows; a failing check does not stop the others, and the failures are listed at the end; it runs the web tests on your own Node version only and does not run the Windows clippy, which `just cross-clippy` does), `just test <filter>` runs `cargo test --quiet --no-fail-fast <filter>` and prints a note on stderr when a filter names a member crate or a module inside one, or a warning when no test matched, `just test-crate <crate> <filter>` runs one member crate's tests (`cargo test --quiet --no-fail-fast -p residuum-<crate> <filter>`; `just test-crate memory search::` runs `residuum-memory`'s search tests), `just web-mock` runs the web UI against the mock API, and `just web-e2e` runs the web end-to-end, accessibility and visual suite (the visual comparisons render in the Playwright container, so it needs Docker; `just web-e2e-fast` leaves them out). In CI the visual comparisons run in a job of their own that never fails a pull request or a release, and report changed shots in one pull request comment. Every cargo recipe that builds the root package builds `web/dist` first when it is missing, because `build.rs` refuses to compile without it; `test-crate` and `pebble` build only member crates and skip it.

### Cross-Platform Targets

Release builds target Linux x86_64, Linux aarch64, macOS aarch64 (Apple Silicon), and Windows x86_64 (`x86_64-pc-windows-gnu`). Keep platform differences in mind:
- **`c_char`**: `i8` on x86_64, `u8` on aarch64-linux — always use `std::ffi::c_char` in FFI signatures, never hardcode `i8`/`u8`
- **Path separators, endianness, pointer width**: use `std` abstractions, not platform-specific assumptions
- **FFI code**: test against the aarch64 target when touching unsafe/FFI boundaries

Every Rust PR runs clippy with `-D warnings` for `x86_64-pc-windows-gnu` alongside the host clippy (the `Clippy (x86_64-pc-windows-gnu)` job), so Windows-only lint failures, including test imports only some platforms use, fail the PR. The other targets (aarch64 Linux and macOS clippy, and the test suite on a Windows runner) are in `cross-compile.yml`. The merge queue runs it, as part of `CI gate`, for every batch that changes `Cargo.toml`, `Cargo.lock`, `build.rs`, `rust-toolchain.toml`, a member's `crates/*/Cargo.toml`, or `.github/workflows/cross-compile.yml`. On a pull request it runs, without gating the merge, when the PR changes one of the manifests, `build.rs` or `rust-toolchain.toml`, carries the `cross-compile` label, or is dispatched with `gh workflow run cross-compile.yml --ref <branch>`. Add the label for platform-sensitive changes (unsafe/FFI, `cfg(target_os)`/`cfg(unix)` branches, paths, process spawning, signals, permissions), and fix what it reports before enqueueing the PR. `just cross-clippy` lints a cross target locally. See CONTRIBUTING.md.

To run on real Windows locally (tests, clippy with the native toolchain, and desktop checks such as toast notifications), use the VM harness in `scripts/windows-vm/`; see [docs/runbooks/windows-harness.md](./docs/runbooks/windows-harness.md).

### Rust Toolchain

The Rust version is pinned in `rust-toolchain.toml` so the git hooks, CI, and release builds all use the same compiler and clippy. Bumping it is its own PR, together with fixes for any new lints.

### Lint Rules

Clippy pedantic is enabled with strict error handling:
- `unsafe_code` - **denied** (not `forbid`, so a genuine FFI boundary can carry a scoped `#[expect(unsafe_code, reason = "...")]` at the item level; `forbid` cannot be overridden that way)
- `unwrap_used`, `expect_used`, `panic`, `todo`, `unimplemented` - **denied**
- `missing_errors_doc`, `missing_panics_doc`, `must_use_candidate` - **denied**
- `print_stderr` - **denied** (residuum runs as a daemon; diagnostics go through `tracing`, not stderr)

The lint table is `[workspace.lints.rust]` and `[workspace.lints.clippy]` in the root `Cargo.toml`. The root package and every member crate opt in with `[lints] workspace = true`, and `testing::manifest_scan` fails a member that does not. `clippy.toml`, `rustfmt.toml` and `deny.toml` sit at the repository root and apply to every crate.

The lint set is kept in parity with the `rust-toolkit` reference project. `print_stderr` and the sqlite-vec-specific `unsafe_code` rationale are the only intentional residuum-only deltas. Supply-chain policy lives in `deny.toml`: vulnerabilities and source problems block, unmaintained and yanked crates do not. Every entry in the `advisories.ignore` list carries a comment saying why it is tolerated and what would let it be dropped.

The root `clippy.toml` exempts `unwrap_used`, `expect_used`, `panic`, and `dbg_macro` for code inside `#[cfg(test)]` modules and `#[test]`-attributed functions, so test code may freely use unwrap/expect/panic/dbg for readability with no suppression header needed. Adding an `#[expect(clippy::unwrap_used, ...)]` (or `expect_used`/`panic`/`dbg_macro`) to a test file is an error, not belt-and-braces — the lint is already exempt there, so the expectation never fires and becomes an unfulfilled-expectation hard error under `-D warnings`. `clippy::tests_outside_test_module` is the one exception: it is not controllable from `clippy.toml` and stays denied project-wide, so integration tests under `tests/` (which live outside a `#[cfg(test)]` module) still need an explicit `#![expect(clippy::tests_outside_test_module, reason = "...")]` at the top of the module's file. Plain helper functions at the top level of a `tests/it/*.rs` module (not wrapped in `#[cfg(test)]` and not themselves `#[test]`-attributed) are also not covered by the `clippy.toml` exemption and still need their own suppression if they use unwrap/expect/panic/dbg. Code behind `#[cfg(any(test, feature = "test-support"))]` is not test code to clippy either: `cargo clippy --all-features` compiles it with the feature on, as production code, so an unwrap, expect or panic there carries a scoped `#[expect(clippy::…, reason = "test-support helper: …")]`, which that build fulfills.

Changing a lint, a lint level, or a clippy or deny setting needs explicit approval from the user. Editing a comment in these files to keep it accurate does not.

### Testing

Testing is a first-class operation — NEVER skip test implementation.
- Always run `cargo test --quiet` — never plain `cargo test`. The `--quiet` flag suppresses per-test noise and only shows failures and the summary.
- Unit tests: `#[cfg(test)] mod tests` at file bottom
- Integration tests: modules of the one test binary in `tests/it/` (a file there plus a `mod` line in `tests/it/main.rs`; `cargo test --test it gateway::` runs one module). They share a process, so a test doesn't change environment variables, the current directory or the global tracing subscriber. `tests/ts_export.rs` is the only other file directly in `tests/`; every such file is a test binary of its own that links the root library and every member crate. Member crates have no `tests/` directory: their tests are unit tests, and integration tests that need one live in the root's `tests/it/`
- Test filters are substring matches on a test's path, and a member crate's test paths start below the crate: `residuum-memory`'s search tests are `search::tests::…`, not `memory::search::tests::…`. So `cargo test --quiet memory::` (or `just test memory::`) runs the root tests whose path contains `memory::` and none of `residuum-memory`'s. Run a member's tests with `just test-crate <crate> <filter>` (`cargo test --quiet -p residuum-<crate> <filter>`), which builds only that crate and the crates below it and needs no `web/dist`; `just test -p residuum-<crate> <filter>` runs the same tests after the web build every `just test` does. `just test` and `just stress` print a note when a filter's first segment names a member crate or a module inside one, and a warning when no test matched
- Test helpers: a crate's own tests use its `#[cfg(test)]` code. Helpers other crates' tests use live in the lowest crate that can hold them, behind that crate's `test-support` feature, which a package turns on only from `[dev-dependencies]`: `residuum_core::testing` (`wait`, `gate`, `clock`, `script`, `log`, `ports`), `residuum_config::testing` and `WorkspaceLayout::with_tempdir_guard`, `residuum_inference::testing::model`, `residuum_memory::testing`, and `residuum-net`'s pairing hooks (`advance_clock`, `pair_device_for_tests`). The root's `crate::testing` (`src/testing/`, `#[cfg(test)]`) holds what only root tests use: `bus_wait` (waits on the bus and the hub), the source and manifest scans and the wall-clock backlog. Root tests name core's helpers as `residuum_core::testing::…`, since the root's own `testing` module takes that name, and the other crates' through the facade (`crate::inference::testing::model`, `crate::memory::testing`)
- The pre-push hook runs the tests of the code a push touches (see Git Hooks), and CI runs the whole suite. When a change reaches code that other modules depend on, such as a shared helper or a trait other modules implement, run those modules' tests (`just test <filter>`) or the whole suite (`cargo test --quiet`) before pushing.
- A test waits on the event it checks, never on elapsed time. A flaky test is fixed by making it wait on an event the code exposes (adding the signal to the code when there is none), never by widening a timeout, adding a sleep, or rerunning past it. The suites under load run nightly on `main` (`nightly-stress.yml`), not on pull requests or in the merge queue: the Rust suite as two concurrent copies with every CPU busy, and every end-to-end spec repeated with the browser's CPU throttled. Each test that fails there arrives as an `investigate` issue titled `Nightly stress: <test>` (a member crate's test arrives as `Nightly stress: residuum_<crate>::<test>`), with the run link and the failure's messages; a test that fails again adds a comment to its open issue. A test that failed under load only waits on time: fix it by making it wait on the event it checks, never by widening a timeout, adding a sleep, or rerunning past it. One that also failed in the unloaded Tests step is an ordinary failure. Reproduce one locally with `just stress <filter>` (two concurrent copies under CPU load, which is how tests run when several agents share a machine) or `WEB_STRESS_ALL=1 just web-stress <spec>`; `just web-stress` on its own repeats the spec files a branch adds or changes (`web/CONTRIBUTING.md`, "CI"). A test you add or fix must pass `just stress <filter>` before you push; the nightly run is the backstop, not the first check. Tests wait through `residuum_core::testing` (waits under one hang guard, gates, a paused-clock helper) and the root's `crate::testing::bus_wait`; `CONTRIBUTING.md`, "Waiting in tests", says which fits where, and a source-scan test enforces it.
- After a red CI run, run `just ci-failures` (or `just ci-failures <pr-number|branch>`) and fix every listed failure before pushing again, except one marked `likely flake` (not `touched`, but `seen elsewhere`). That one may be a flake or a breakage other branches share: check whether `main` fails the same way, and file or add to an `investigate` issue rather than chasing it in this PR. Every other failure, `touched` or not, is this change's to fix until you show otherwise.

## Code Style

### Naming
- **Domain-specific names**: Prefer descriptive names that match the domain (`send_chat_completion` over generic `run`)
- **Common abbreviations OK**: `cfg`, `dir`, `msg`, `ctx`, `cmd` are fine; avoid obscure ones
- **Semantics matter**: Structs, enums, and functions should have names that make it abundantly clear what they do. Avoid vague names and catch-alls. If the logic doesn't match the semantics a refactor is needed.

### Error Messages
- Always include context: `"failed to parse config at {path}"` not just `"parse error"`
- Lowercase, no trailing period (Unix style, chains well with `anyhow` context)

### Comments
- Explain **why**, never **what** — the code shows what, comments explain non-obvious reasoning
- Doc comments: one-line `///` summary for public items; expand only for complex behavior

### Module Organization
- `mod.rs` files should primarily contain declarations and re-exports, but module-level coordination logic is fine when it belongs there.
- Group related types in one file (e.g., `Message`, `Role`, `ToolCall` together in `llm/types.rs`)

### Visibility
- Private-first: start with no visibility modifier, add `pub(crate)` or `pub` only when needed
- Treat `pub` as a commitment — once public, it's API

### Function Signatures
- **Strings**: `&str` for read-only, `impl Into<String>` when storing, owned `String` when caller must give up ownership
- **Async**: async-first; only use sync for trivial or CPU-bound operations
- **Generics**: default to concrete types, generify at public API boundaries when flexibility is needed

### Construction
- Prefer `new()` with required args + `Default` trait for optional configuration
- Avoid builder pattern unless struct has many optional fields

## Designing Behavior: Make Failure Safe

Residuum does long-running, autonomous work, so things will go wrong mid-run. Design every behavior, agent-facing or not, so that when something goes wrong it is seen, contained, and recoverable. Build the capability first, and build its safety in this order:

1. **Visibility.** The user and the agent can see what is happening and what went wrong: how long a run has been going, how many tool calls it has made, how many tokens it has spent, which input is unreasonable and why. Flag unreasonable input instead of refusing it, judging "unreasonable" by cost and benefit to the user rather than by what seems typical: a skill with an oversized description still loads, and raises a notice that it costs tokens on every turn. The mechanics are in "No Silent Failures" below.
2. **Degradation, recovery, and intervention.** When something breaks, the system degrades instead of halting. Destructive actions come with checkpoints, rollback, or undo. The user can step in: any long-running or open-ended behavior ships with a way to see it *and* a way to stop it.
3. **Prevention, last.** A hard limit, block, or rejection is reserved for a failure that has been observed and can be described concretely: a model repeating byte-identical tool calls hundreds of times, or a remote shutdown of the gateway that leaves no way to bring it back. Scope the guard as narrowly as that failure, and describe the observed failure in the guard's comment. Approval gates, blocklists, fail-closed checks, and hold-for-review flows are prevention too, so the agent and the user are trusted to act by default. Users who want more oversight get opt-in guards they choose, not defaults Residuum decides for them.

**Tiers 1 and 2 are part of the feature, and they are gated twice:**

- **Before implementation**, state them explicitly, to the user or in the plan or design doc: what the user and the agent will see, how the feature degrades, and how it is stopped, rolled back, or undone.
- **Before the feature ships**, review the completed implementation for gaps in both tiers that weren't anticipated by the plan. A feature missing either tier is not done.

### Chesterton's Ghosts

The failure this section exists to prevent is a guard built around a failure nobody has observed: a phantom risk, or behavior someone assumed was unwanted. Coding agents add these by instinct, often with an authoritative comment explaining why the code must be written this way. They ripple into product decisions. A hard cap on tool calls per turn works directly against long-running work, where a visible turn, tool-call, and token count plus a stop control addresses the same concern without blocking anything.

- The urge to add a guard usually means a higher tier is missing. Ask what the user would need to see the problem and to stop it, and build that.
- A comment justifying a guard records what an agent believed when it wrote it; it is not an authority. If it names no observed failure, it is a ghost: replace it with visibility and intervention rather than preserving it.
- A gap is missing visibility or intervention, not behavior you would have designed differently. If a behavior is visible and the user can stop or undo it, how it behaves is a product decision: leave it, and raise it only when it carries genuine risk or cost.
- Document product behavior in `docs/systems-usage/` as a plain description of how the system works. Only the user decides what is intentional; agents describe what is. Skip "by design", "intentional", and "do not change": the doc records current behavior, not a case for keeping it.
- This governs product behavior, not code correctness. Lints, git hooks, and `deny` rules guard against an observed, recurring failure (agents taking shortcuts) and stay as the floor.

## Error Handling & Observability

### No Silent Failures

**Every failure must be visible.** This is non-negotiable.

#### User-facing: clear, actionable messages
- Assume users are non-technical — error messages must explain what went wrong and what to do next, not expose internals
- Use plain language: `"Couldn't connect to the server. Check your internet connection and try again."` not `"TCP connection refused on port 443"`
- Partial failures (e.g., syncing 2 of 3 items) must tell the user what succeeded, what failed, and whether they need to act
- Never show raw error types, stack traces, or module paths to the user
- If an operation fails silently with no user impact, it still needs a log (see below)

#### Developer-facing: rich, structured diagnostics
- Every error path must produce a log entry with enough context to diagnose without reproducing
- Include structured fields: `error!(error = %e, path = %path, "failed to read config")` — not just the message
- Chain error context with `anyhow`: `.context("failed to load user settings")` so logs show the full causal chain
- Use appropriate log levels (error/warn/info/debug/trace per the logging guidelines below)
- Transient failures (retries, timeouts) should log at `warn` with attempt count and backoff details

#### Avoid log spam
- Do not log every retry attempt individually — log once at `warn` when retries start, and once when they resolve or exhaust
- Do not log routine successful operations ("connection still alive", "heartbeat ok") — absence of errors is the signal that things work
- Periodic health-check style output belongs at `trace` level at most, never `info` or above
- If a log line would fire on every loop iteration or timer tick under normal conditions, it's too noisy

### Logging (tracing)
- **error**: failures that stop an operation
- **warn**: recoverable issues, degraded behavior
- **info**: major operations (LLM calls, chunked processing)
- **debug**: internal details, state transitions
- **trace**: verbose diagnostics (full payloads, timing)
- Use structured fields: `info!(chunks = count, "starting chunked review")` not string interpolation

### Debugging & Tracing
- Log level is configured in `hub/config.toml` under `[tracing]`: `log_level = "info" | "debug" | "trace"` (default: `debug`)
- `residuum logs` — view saved log files; `residuum logs --watch` to tail live; `residuum logs --level warn` to filter at read time; `residuum logs --agent <name>` to show one agent's lines
- `residuum tracing status` — show current tracing config and streaming state
- `residuum tracing otel add <url>` — add an OTEL endpoint for trace export
- `residuum tracing dump` — one-shot export of buffered traces to configured OTEL endpoints
- `residuum tracing stream start|stop` — live trace streaming to OTEL endpoints
- `residuum tracing sanitize on|off` — toggle content redaction in trace exports (default: on)
- `residuum tracing error-reporting on|off` — toggle auto error reporting (default: off). The switch is stored for the running daemon and shows up in `residuum tracing status`. When on, `TracingService::on_error` submits a synthetic `broken`-severity bug report (with the current span buffer) through the same path as a manual report, rate limited to roughly one submission per minute per error context and five per hour overall; suppressed submissions log at debug. It's called from every unexpected turn failure — the main agent's turns and background/session/pulse turns alike, once retries are exhausted — and from the event loop's tunnel-respawn and adapter-exit supervisory paths; expected outcomes (a user-cancelled turn, a validation error, a retry that later succeeds) never call it. `residuum bug-report` is the other path that sends one, for a manual report.
- `residuum bug-report -m "description"` — send a sanitized trace dump to the developer via the feedback-ingest service
- `RUST_LOG` env var overrides the configured log level when set. A directive matches log targets by prefix, and a target starts with the crate's library name: the root's modules log as `residuum::<module>` (`RUST_LOG=residuum::hub=trace`), a member crate's as `residuum_<crate>::…` (`RUST_LOG=residuum_memory=trace`, `RUST_LOG=residuum_net::tunnel=trace`, `RUST_LOG=residuum_config::agent_keys=trace`), so `residuum::memory=trace` matches nothing. The "Logging" section of `docs/systems-usage/hub.md` lists which module lives in which crate. `residuum logs --module <module>` matches either form

## Git Workflow

Single-branch model: all work lands on `main`, and only through the merge queue (see Merging).

### Day-to-Day Work

1. **Create a feature branch from `main`** with a descriptive name (e.g., `feat/add-telegram-retry`, `fix/memory-search-ranking`)
2. **Commit frequently** — the pre-commit hook formats and audits what is staged; clippy, the tests of the code you changed, the generated-types check and the web checks run once per push. Commit especially often during large multi-phase tasks.
3. **Wrapup** Check for anything unfinished, ensure documents and guides are updated, create migration guides for breaking changes.
4. **Push the branch and open a PR** against `main`, with `closes #N` in the PR body for each issue it closes.
5. **Enqueue it**: `gh pr merge <n> --auto`. The merge queue merges it once its checks pass (see Merging).
6. **After it merges**, switch back to `main`, pull, and delete the local branch. GitHub deletes the remote branch on merge.

All changes must be committed before giving the user a completion summary. **Never** use `git -C` — the shell is already in the project root; use plain `git` commands.

### Merging

`main` changes only through the GitHub merge queue. The ruleset in `.github/rulesets/main.json` refuses a push to `main`, a force push or its deletion, and merges a pull request only through the queue, as a merge commit, once its `CI gate` check has passed. The flow needs auto-merge allowed and head branches deleted on merge, which the repository settings hold (`CONTRIBUTING.md`, Merging).

- **Never push to `main`, and never merge locally** (no `git merge` into `main` followed by a push). The push is refused. Every change, however small, goes through a PR.
- **Enqueue with `gh pr merge <n> --auto`.** While the PR's checks run, this turns on auto-merge; when `CI gate` passes, GitHub adds the PR to the queue. The queue merges it onto the tip of `main` together with any PRs ahead of it, runs CI on that batch (the merge queue lane), and merges the batch when it passes. The command returns at once, and the merge happens later: don't wait on it in the foreground, and don't report the work as merged until `state` is `MERGED`.
- **Put `closes #N` in the PR body** (`gh pr create --body`, or `gh pr edit <n> --body`). The queue writes the merge commit's message, so there is no merge commit message of your own to put it in, and a keyword in a PR comment closes nothing.
- **See where a PR stands** with `gh pr view <n> --json state,mergeStateStatus,autoMergeRequest,statusCheckRollup`: `autoMergeRequest` is set while the PR waits for its checks to enter the queue, and `state` is `MERGED` once it has merged. Its place in the queue: `gh api graphql -f query='query($n:Int!){repository(owner:"Grizzly-Endeavors",name:"residuum"){pullRequest(number:$n){mergeQueueEntry{position state}}}}' -F n=<n>` (`null` when it is not in the queue). The whole queue is at https://github.com/Grizzly-Endeavors/residuum/queue/main.
- **A PR ejected from the queue** (its batch failed CI, it conflicts with a PR merged ahead of it, or its checks timed out) is removed from the queue, with the reason on the PR's timeline, and stays out until it is enqueued again. Fix the cause on the branch (`just ci-failures <n>` lists the failing tests, including those of the merge queue batch that failed when the PR's own runs are green; merge `main` into the branch to resolve a conflict), push, and run `gh pr merge <n> --auto` again.
- **A fork's PR** shows a passing `CI gate` without having run anything. Enqueueing it runs its code, including any change it makes under `.github/`, on the self-hosted runners, so read the whole diff before enqueueing a fork's PR.
- **Emergency bypass is admin-only.** A repository admin can merge a PR past the queue and its checks with `gh pr merge <n> --merge --admin`; the bypass works only through a PR, so even an admin cannot push to `main`. It is for an emergency the owner declares, such as CI itself being broken. Agent sessions run as an admin account, so the command works for them too: an agent never uses it unless the owner asks for that specific merge.

### Branch Naming

Use prefixed branch names:
- `feat/` — new features
- `fix/` — bug fixes
- `refactor/` — code restructuring without behavior changes
- `docs/` — documentation only
- `ci/` — CI/CD changes
- `chore/` — maintenance, dependency updates

### Releases

Releases use **CalVer** (`YYYY.0M.0D`), not SemVer. Tags like `v2026.03.02`, with `-N` suffix for same-day follow-ups (`v2026.03.02-2`). Cargo.toml version is independent and not tied to release tags. The release workflow runs the merge queue lane's Rust and web checks alongside the artifact builds and publishes nothing unless both pass.
