# AGENTS.md | Residuum - Personal Agent Framework

Rules for coding agents working in this repository. `CLAUDE.md` imports this file, so Claude Code and every other agent read the same rules. [CONTRIBUTING.md](./CONTRIBUTING.md) has the details behind them.

## Key References

- [Design Philosophy](./docs/design-philosophy.md)
- [Systems Usage](./docs/systems-usage/) — authoritative reference for how systems work today
- [Guides](./docs/guides/) — task-oriented walkthroughs
- [Design](./docs/design/) — designs for work that is not built yet
- [Hub](./docs/systems-usage/hub.md) — one process hosts every agent; `~/.residuum/` holds `hub/`, the shared `team/`, and one directory per agent
- [Workspace layout](./CONTRIBUTING.md#workspace-layout) — the crates, the dependency rules, the root facade and the public API policy
- [Runbooks](./docs/runbooks/) — maintainer operations: build dirs, the `main` ruleset, CI metrics, the Windows VM

Superseded documents live in [`docs/archive/`](./docs/archive/); they record history and do not describe current behavior.

**Web interface:** the Residuum web UI lives in `web/` (Svelte 5 SPA); see `web/CLAUDE.md`. Do not confuse it with `relay/web/`, the agent-residuum.com site, which lives in a separate repository.

## Build & Quality Gates

### Workspace

One Cargo workspace. The root `Cargo.toml` is the workspace manifest and the `residuum` package (the binary, the CLI, the daemon, and the embedded `web/dist`). Everything else is a member crate in `crates/<dir>/`, named `residuum-<dir>`. Dependencies point down only: no member depends on the root or on a crate above it. `CONTRIBUTING.md`, "The crates", lists what each crate holds.

- `cargo build`, `cargo test --quiet` and `cargo clippy` from the root cover every package; `-p residuum-<dir>` narrows to one.
- The root package needs `web/dist` to build (`just web-build`); member crates don't.
- Root code names members through the aliases in `src/lib.rs` (`crate::memory::…`); members name each other by crate (`residuum_config::Config`).

### Commands

`just` lists every recipe. The ones you'll use most:

| Command | What it does |
|---|---|
| `just check` | Everything CI runs except the web end-to-end suite; lists every failure |
| `just test <filter>` | Root and workspace tests (`cargo test --quiet --no-fail-fast`) |
| `just test-crate <crate> <filter>` | One member crate's tests, no web build |
| `just stress <filter>` | Tests as two concurrent copies under CPU load (`-p residuum-<crate>` for a member) |
| `just ci-failures [pr\|branch]` | Every failure in the latest CI runs, flagged `touched` / `seen elsewhere` |
| `just types` | Regenerate the web's TypeScript protocol types after changing an exported Rust type |
| `just cross-clippy` | Clippy for the Windows target from Linux |

Builds lease a warm target dir from a shared pool (`docs/runbooks/build-dirs.md`); use `just clean`, not `cargo clean`, in the main checkout.

### Git hooks

Install with `just hooks`. **Bypassing a hook with `--no-verify` is forbidden**, on `git commit` and `git push`. If a hook fails, fix the cause.

- **pre-commit** formats what is staged, runs `cargo deny` when a manifest is staged, and blocks `dbg!()`, `#[allow(...)]` and growth of `src/testing/wall_clock_backlog.rs`.
- **pre-push** runs clippy over the affected packages, the tests of the code you changed, the generated-types check and `cargo shear` when Rust changes, and the web lint, type and related unit tests when `web/` changes. It prints which tests it picked. It does not run the whole suite: when you change code other modules depend on (a shared helper, a trait), run their tests or `cargo test --quiet` before pushing.
- The full suite runs in the merge queue. `CONTRIBUTING.md`, "Quality Gates", has what runs where.

### Cross-platform

Releases target Linux x86_64 and aarch64, macOS aarch64, and Windows x86_64 (`x86_64-pc-windows-gnu`).

- Use `std::ffi::c_char` in FFI signatures, never `i8`/`u8`. Use `std` abstractions for paths, endianness and pointer width.
- Every Rust PR gets a Windows clippy early warning; fix it before enqueueing.
- Add the `cross-compile` label to a PR that touches unsafe/FFI, `cfg(target_os)`/`cfg(unix)` branches, paths, process spawning, signals or permissions, and fix what it reports.
- Real Windows testing: [docs/runbooks/windows-harness.md](./docs/runbooks/windows-harness.md).

### Toolchain and lints

The Rust version is pinned in `rust-toolchain.toml`; bumping it is its own PR, with fixes for new lints.

The lint table is `[workspace.lints]` in the root `Cargo.toml`: clippy pedantic plus a deny list that includes `unsafe_code`, `unwrap_used`, `expect_used`, `panic`, `indexing_slicing`, `todo`, `missing_errors_doc`, `missing_panics_doc`, `must_use_candidate`, `print_stderr` (residuum is a daemon; diagnostics go through `tracing`) and `allow_attributes`. Every member sets `[lints] workspace = true`.

- Suppress with a scoped `#[expect(lint, reason = "...")]`, never `#[allow]`. `unsafe_code` is `deny`, not `forbid`, so a real FFI boundary can carry one.
- `unwrap`, `expect`, `panic` and `dbg!` are already allowed inside `#[cfg(test)]` and `#[test]` code by `clippy.toml`. Adding an `#[expect]` for them there is an error (the expectation is never fulfilled).
- Code behind `#[cfg(any(test, feature = "test-support"))]` is production code to clippy and needs its own `#[expect(..., reason = "test-support helper: ...")]`.
- Integration test files under `tests/it/` need `#![expect(clippy::tests_outside_test_module, reason = "...")]`, and their top-level helpers need their own suppressions.

Changing a lint, a lint level, or a clippy or deny setting needs explicit approval from the user.

### Testing

Testing is a first-class operation — never skip test implementation.

- Always `cargo test --quiet` (or `just test`), never plain `cargo test`.
- Unit tests go in `#[cfg(test)] mod tests` at the bottom of the file. Integration tests are modules of the one test binary in `tests/it/`, each with exactly one `//! covers: <package>, …` line naming the packages it tests. They share a process, so a test never changes environment variables, the current directory or the global tracing subscriber.
- A filter matches a test's path, and a member crate's paths start below the crate: `residuum-memory`'s search tests are `search::tests::…`. Run them with `just test-crate memory search::`.
- Shared test helpers live in the lowest crate that can hold them, behind its `test-support` feature, which a package turns on only from `[dev-dependencies]`. Look for `testing` modules under `crates/*/src/` before writing a new helper.
- **A test waits on the event it checks, never on elapsed time.** Fix a flaky test by making it wait on an event the code exposes (adding the signal when there is none), never by widening a timeout, adding a sleep, or rerunning. `CONTRIBUTING.md`, "Waiting in tests", says which helper fits; a source scan enforces it.
- A test you add or fix must pass `just stress <filter>` before you push. Nightly stress runs file each failure as a `Nightly stress: <test>` issue.
- After a red CI run, run `just ci-failures` and fix every failure, `touched` or not. A `likely flake` (not touched, seen elsewhere) may be shared breakage: check `main`, and file or add to an `investigate` issue instead of chasing it in this PR.

## Code Style

### Naming
- **Domain-specific names**: prefer names that match the domain (`send_chat_completion` over `run`)
- **Common abbreviations OK**: `cfg`, `dir`, `msg`, `ctx`, `cmd`; avoid obscure ones
- **Semantics matter**: a struct, enum or function's name says what it does. If the logic doesn't match the name, refactor.

### Error Messages
- Include context: `"failed to parse config at {path}"`, not `"parse error"`
- Lowercase, no trailing period (chains well with `anyhow` context)

### Comments
- Explain **why**, never **what**
- Doc comments: one-line `///` summary for public items; expand only for complex behavior

### Module Organization
- `mod.rs` holds declarations and re-exports, plus module-level coordination logic when it belongs there
- Group related types in one file

### Visibility
- Private-first: add `pub(crate)` or `pub` only when needed
- `pub` is a commitment. Before widening an item across crates, follow `CONTRIBUTING.md`, "Public API".

### Function Signatures
- **Strings**: `&str` for read-only, `impl Into<String>` when storing, owned `String` when the caller gives up ownership
- **Async**: async-first; sync only for trivial or CPU-bound work
- **Generics**: concrete types by default; generify at public API boundaries when flexibility is needed

### Construction
- `new()` with required args + `Default` for optional configuration
- No builder pattern unless a struct has many optional fields

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
- Assume users are non-technical — say what went wrong and what to do next, not internals
- Plain language: `"Couldn't connect to the server. Check your internet connection and try again."` not `"TCP connection refused on port 443"`
- Partial failures (syncing 2 of 3 items) say what succeeded, what failed, and whether the user needs to act
- Never show raw error types, stack traces, or module paths to the user
- A failure with no user impact still needs a log

#### Developer-facing: rich, structured diagnostics
- Every error path logs enough context to diagnose without reproducing
- Structured fields: `error!(error = %e, path = %path, "failed to read config")`
- Chain context with `anyhow`: `.context("failed to load user settings")`
- Transient failures (retries, timeouts) log at `warn` with attempt count and backoff

#### Avoid log spam
- Log once at `warn` when retries start, and once when they resolve or exhaust — not every attempt
- Don't log routine success ("heartbeat ok"); the absence of errors is the signal
- Periodic health-check output is `trace` at most
- A line that fires on every loop iteration or timer tick under normal conditions is too noisy

### Logging (tracing)
- **error**: failures that stop an operation
- **warn**: recoverable issues, degraded behavior
- **info**: major operations (LLM calls, chunked processing)
- **debug**: internal details, state transitions
- **trace**: verbose diagnostics (full payloads, timing)

### Debugging
- Log level: `hub/config.toml`, `[tracing] log_level` (default `debug`). `RUST_LOG` overrides it.
- `RUST_LOG` targets start with the crate's library name: root modules are `residuum::<module>` (`residuum::daemon=trace`), members are `residuum_<crate>` (`residuum_memory=trace`). `docs/systems-usage/hub.md`, "Logging", maps modules to crates.
- `residuum logs [--watch] [--level warn] [--agent <name>] [--module <m>]` reads the saved logs.
- `residuum tracing status | otel add <url> | dump | stream start|stop | sanitize on|off | error-reporting on|off` manages trace export.
- `residuum bug-report -m "description"` sends a sanitized trace dump to the developer.

## Git Workflow

All work lands on `main`, and only through the merge queue.

1. Branch from `main` with a prefixed name: `feat/`, `fix/`, `refactor/`, `docs/`, `ci/`, `chore/`.
2. Commit often; the hooks keep each commit and push honest.
3. Before wrapping up, check for unfinished work, update docs and guides, and write migration guides for breaking changes.
4. Push and open a PR against `main`, with `closes #N` in the PR **body** for each issue it closes (the queue writes the merge commit, and a keyword in a comment closes nothing).
5. Enqueue with `gh pr merge <n> --auto`. It returns at once; don't report the work merged until `gh pr view <n> --json state` says `MERGED`.
6. After it merges, switch to `main`, pull, and delete the local branch.

All changes are committed before you give the user a completion summary. Never use `git -C`.

### Merging rules

- **Never push to `main` or merge locally.** The ruleset refuses it.
- A PR ejected from the queue stays out until re-enqueued: fix the cause (`just ci-failures <n>`; merge `main` for a conflict), push, and run `gh pr merge <n> --auto` again.
- A fork's PR shows a passing `CI gate` without running anything, and enqueueing it runs its code on the self-hosted runners. Read the whole diff first.
- `gh pr merge <n> --merge --admin` bypasses the queue. Agent sessions run as an admin, so the command works for you: never use it unless the owner asks for that specific merge.

### Releases

CalVer (`YYYY.0M.0D`) tags like `v2026.03.02`, with `-N` for same-day follow-ups. The Cargo.toml version is independent of release tags.
