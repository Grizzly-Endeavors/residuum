# CLAUDE.md | Residuum - Personal Agent Framework

**Important docs:**
- [Design Philosophy](./docs/design-philosophy.md)
- [Residuum Design](./docs/residuum-design.md)
- [Projects Context](./docs/projects-context-design.md)
- [Personal Agent](./docs/personal-agent-design.md)
- [Background Tasks](./docs/background-tasks-design.md)
- [Memory Search](./docs/memory-search-design.md)
- [Notification Routing](./docs/notification-routing-design.md)
- [Systems Usage](./docs/systems-usage/) (authoritative reference for how systems are intended to work)

## Commit Requirements, Linting, and Formatting

### Git Hooks

Pre-commit hooks enforce quality gates:
- **pre-commit**: auto-formats with `cargo fmt` (auto-stages changes), runs `cargo clippy`, runs `cargo test`
- **commit-msg**: validates message format

Bypass is **FORBIDDEN**.

### Cross-Platform Targets

Release builds target Linux x86_64, Linux aarch64, and macOS aarch64 (Apple Silicon). Keep platform differences in mind:
- **`c_char`**: `i8` on x86_64, `u8` on aarch64-linux — always use `std::ffi::c_char` in FFI signatures, never hardcode `i8`/`u8`
- **Path separators, endianness, pointer width**: use `std` abstractions, not platform-specific assumptions
- **FFI code**: test against the aarch64 target when touching unsafe/FFI boundaries

### Lint Rules

Clippy pedantic is enabled with strict error handling:
- `unsafe_code` - forbidden
- `unwrap_used`, `expect_used`, `panic`, `todo`, `unimplemented` - **denied**
- `missing_errors_doc`, `missing_panics_doc`, `must_use_candidate` - warnings

Test modules have `#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]` for readability.

DO NOT, under any circumstance, change this config without explicit approval from the user.

## Style Guidelines

### Naming
- **Domain-specific names**: Prefer descriptive names that match the domain (`send_chat_completion` over generic `run`)
- **Common abbreviations OK**: `cfg`, `dir`, `msg`, `ctx`, `cmd` are fine; avoid obscure ones

### Error Messages
- Always include context: `"failed to parse config at {path}"` not just `"parse error"`
- Lowercase, no trailing period (Unix style, chains well with `anyhow` context)

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
- Use appropriate log levels (error/warn/info/debug/trace per the logging guidelines above)
- Transient failures (retries, timeouts) should log at `warn` with attempt count and backoff details

#### Avoid log spam
- Do not log every retry attempt individually — log once at `warn` when retries start, and once when they resolve or exhaust
- Do not log routine successful operations ("connection still alive", "heartbeat ok") — absence of errors is the signal that things work
- Periodic health-check style output belongs at `trace` level at most, never `info` or above
- If a log line would fire on every loop iteration or timer tick under normal conditions, it's too noisy

### Comments
- Explain **why**, never **what** — the code shows what, comments explain non-obvious reasoning
- Doc comments: one-line `///` summary for public items; expand only for complex behavior

### Module Organization
- `mod.rs` files should be reserved for declarations and re-exports, not logic.
- Group related types in one file (e.g., `Message`, `Role`, `ToolCall` together in `llm/types.rs`)
- Tests: unit tests in `#[cfg(test)] mod tests` at file bottom; integration tests in `tests/`

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

### Logging (tracing)
- **error**: failures that stop an operation
- **warn**: recoverable issues, degraded behavior
- **info**: major operations (LLM calls, chunked processing)
- **debug**: internal details, state transitions
- **trace**: verbose diagnostics (full payloads, timing)
- Use structured fields: `info!(chunks = count, "starting chunked review")` not string interpolation

### Debugging
- `residuum serve --debug` — debug logging for residuum crates (default mode)
- `residuum serve --debug=all` — debug logging for all crates including dependencies
- `residuum serve --debug=trace` — trace-level logging for residuum crates
- `residuum logs` — view saved log files; `residuum logs --watch` to tail live
- `RUST_LOG` env var overrides `--debug` when set

## Agent Usage

When spawning sub-agents for parallel or delegated work, always include these instructions in the agent prompt:

> **Do NOT run tests, linting, or formatting checks.** Do NOT attempt to commit changes. Focus only on implementing the requested changes. Verification (tests, clippy, fmt) will be run after all agents complete.

This prevents agents from:
- Wasting cycles on verification that will be done centrally
- Creating conflicting commits from parallel work
- Blocking on test failures that may depend on other agents' changes

The orchestrating agent is responsible for running `cargo fmt`, `cargo clippy`, and `cargo test` after all sub-agent work is complete, then creating a single commit.

## Git Workflow

Single-branch model: all work lands on `main`.

### Day-to-Day Work

1. **Create a feature branch from `main`** with a descriptive name (e.g., `feat/add-telegram-retry`, `fix/memory-search-ranking`)
2. **Commit frequently** — pre-commit hooks enforce fmt, clippy, and tests
3. **Push the branch** and merge into `main`

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

## Misc Notes
- Testing is a first class operation, NEVER skip test implementation.
- Commits should be made frequently, especially for large multi-phase tasks.
- All changes must be committed before giving the user a completion summary.
- **Never** use `git -C` — the shell is already in the project root; use plain `git` commands.
- Always run `cargo test --quiet` — never plain `cargo test`. The `--quiet` flag suppresses per-test noise and only shows failures and the summary.
