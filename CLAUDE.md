# CLAUDE.md | Residuum - Personal Agent Framework

`AGENTS.md` carries these same rules for Cursor and other non-Claude agents. This paragraph and the title are the only difference. Change them together.

## Key References

- [Design Philosophy](./docs/design-philosophy.md)
- [Systems Usage](./docs/systems-usage/) — authoritative reference for how systems work today
- [Guides](./docs/guides/) — task-oriented walkthroughs
- [Design](./docs/design/) — designs for work that is not built yet

Superseded documents live in [`docs/archive/`](./docs/archive/); they record history and do not describe current behavior.

**Web interface:** The Residuum web UI lives in `residuum/web/` (Svelte 5 SPA). See `web/CLAUDE.md` for details. **Do not confuse with `relay/web/`**, the marketing landing page — it lives in a separate repository, not this one.

## Build & Quality Gates

### Pre-Commit Hooks

Pre-commit hooks enforce quality gates:
- **pre-commit**: auto-formats with `cargo fmt --all` (re-staging only the paths already staged), runs `cargo clippy --all-targets --all-features -- -D warnings`, runs the tests for the modules touched by the commit, and runs `cargo deny check`. When `web/src/` files are staged it also runs the web formatter, ESLint, `svelte-check`, and the web unit tests (Vitest). Finally it blocks two things outright in the staged diff: a `dbg!()` call, and an `#[allow(...)]` attribute (use `#[expect(lint, reason = "...")]` instead so stale suppressions warn).
- **commit-msg**: advisory checks on the subject line — fails only on a subject under 10 characters, and warns on over-72-character subjects, a trailing period, or a missing conventional prefix.

Bypass is **FORBIDDEN**.

### Cross-Platform Targets

Release builds target Linux x86_64, Linux aarch64, macOS aarch64 (Apple Silicon), and Windows x86_64 (`x86_64-pc-windows-gnu`). Keep platform differences in mind:
- **`c_char`**: `i8` on x86_64, `u8` on aarch64-linux — always use `std::ffi::c_char` in FFI signatures, never hardcode `i8`/`u8`
- **Path separators, endianness, pointer width**: use `std` abstractions, not platform-specific assumptions
- **FFI code**: test against the aarch64 target when touching unsafe/FFI boundaries

CI does not cross-compile by default; when it does, it runs clippy with `-D warnings` for each target and the test suite on a Windows runner. For platform-sensitive changes (unsafe/FFI, `cfg(target_os)`/`cfg(windows)`/`cfg(unix)` branches, paths, process spawning, signals, permissions), add the `cross-compile` label to the PR, or run `gh workflow run cross-compile.yml --ref <branch>`. PRs that change `Cargo.toml`, `Cargo.lock`, `build.rs`, or `rust-toolchain.toml` cross-compile automatically. See CONTRIBUTING.md.

To run on real Windows locally (tests, clippy with the native toolchain, and desktop checks such as toast notifications), use the VM harness in `scripts/windows-vm/`; see [docs/runbooks/windows-harness.md](./docs/runbooks/windows-harness.md).

### Rust Toolchain

The Rust version is pinned in `rust-toolchain.toml` so the pre-commit hook, CI, and release builds all use the same compiler and clippy. Bumping it is its own PR, together with fixes for any new lints.

### Lint Rules

Clippy pedantic is enabled with strict error handling:
- `unsafe_code` - **denied** (not `forbid`, so a genuine FFI boundary can carry a scoped `#[expect(unsafe_code, reason = "...")]` at the item level; `forbid` cannot be overridden that way)
- `unwrap_used`, `expect_used`, `panic`, `todo`, `unimplemented` - **denied**
- `missing_errors_doc`, `missing_panics_doc`, `must_use_candidate` - **denied**
- `print_stderr` - **denied** (residuum runs as a daemon; diagnostics go through `tracing`, not stderr)

The lint set is kept in parity with the `rust-toolkit` reference project. `print_stderr` and the sqlite-vec-specific `unsafe_code` rationale are the only intentional residuum-only deltas. Supply-chain policy lives in `deny.toml`: vulnerabilities and source problems block, unmaintained and yanked crates do not. Every entry in the `advisories.ignore` list carries a comment saying why it is tolerated and what would let it be dropped.

The root `clippy.toml` exempts `unwrap_used`, `expect_used`, `panic`, and `dbg_macro` for code inside `#[cfg(test)]` modules and `#[test]`-attributed functions, so test code may freely use unwrap/expect/panic/dbg for readability with no suppression header needed. Adding an `#[expect(clippy::unwrap_used, ...)]` (or `expect_used`/`panic`/`dbg_macro`) to a test file is an error, not belt-and-braces — the lint is already exempt there, so the expectation never fires and becomes an unfulfilled-expectation hard error under `-D warnings`. `clippy::tests_outside_test_module` is the one exception: it is not controllable from `clippy.toml` and stays denied project-wide, so integration tests under `tests/` (which live outside a `#[cfg(test)]` module) still need an explicit `#[expect(clippy::tests_outside_test_module, reason = "...")]`. Plain helper functions at the top level of a `tests/*.rs` file (not wrapped in `#[cfg(test)]` and not themselves `#[test]`-attributed) are also not covered by the `clippy.toml` exemption and still need their own suppression if they use unwrap/expect/panic/dbg.

DO NOT, under any circumstance, change this config without explicit approval from the user.

### Testing

Testing is a first-class operation — NEVER skip test implementation.
- Always run `cargo test --quiet` — never plain `cargo test`. The `--quiet` flag suppresses per-test noise and only shows failures and the summary.
- Unit tests: `#[cfg(test)] mod tests` at file bottom
- Integration tests: `tests/` directory

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
- This governs product behavior, not code correctness. Lints, pre-commit hooks, and `deny` rules guard against an observed, recurring failure (agents taking shortcuts) and stay as the floor.

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
- Log level is configured in `config.toml` under `[tracing]`: `log_level = "info" | "debug" | "trace"` (default: `debug`)
- `residuum logs` — view saved log files; `residuum logs --watch` to tail live; `residuum logs --level warn` to filter at read time
- `residuum tracing status` — show current tracing config and streaming state
- `residuum tracing otel add <url>` — add an OTEL endpoint for trace export
- `residuum tracing dump` — one-shot export of buffered traces to configured OTEL endpoints
- `residuum tracing stream start|stop` — live trace streaming to OTEL endpoints
- `residuum tracing sanitize on|off` — toggle content redaction in trace exports (default: on)
- `residuum tracing error-reporting on|off` — toggle auto error reporting (default: off). The switch is stored for the running daemon and shows up in `residuum tracing status`. No report is sent: `TracingService::on_error` logs and returns, and nothing calls it ([#101](https://github.com/Grizzly-Endeavors/residuum/issues/101)). `residuum bug-report` is the path that sends one.
- `residuum bug-report -m "description"` — send a sanitized trace dump to the developer via the feedback-ingest service
- `RUST_LOG` env var overrides the configured log level when set

## Git Workflow

Single-branch model: all work lands on `main`.

### Day-to-Day Work

1. **Create a feature branch from `main`** with a descriptive name (e.g., `feat/add-telegram-retry`, `fix/memory-search-ranking`)
2. **Commit frequently** — pre-commit hooks enforce fmt, clippy, and tests. Commit especially often during large multi-phase tasks.
3. **Wrapup** Check for anything unfinished, ensure documents and guides are updated, create migration guides for breaking changes.
4. **Push the branch** and merge into `main`

All changes must be committed before giving the user a completion summary. **Never** use `git -C` — the shell is already in the project root; use plain `git` commands.

### Branch Naming

Use prefixed branch names:
- `feat/` — new features
- `fix/` — bug fixes
- `refactor/` — code restructuring without behavior changes
- `docs/` — documentation only
- `ci/` — CI/CD changes
- `chore/` — maintenance, dependency updates

### Releases

Releases use **CalVer** (`YYYY.0M.0D`), not SemVer. Tags like `v2026.03.02`, with `-N` suffix for same-day follow-ups (`v2026.03.02-2`). Cargo.toml version is independent and not tied to release tags. The release workflow runs full CI checks before building artifacts.
