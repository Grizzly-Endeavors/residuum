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

**Every failure must be visible to the user.** This is non-negotiable.

- If an operation fails, it must either return an error or print a warning to stderr
- Debug/trace logging is NOT sufficient - users don't run with `RUST_LOG=debug` by default
- Partial failures (e.g., reading 2 of 3 files) must be reported, not silently ignored
- Empty input that causes unexpected behavior must warn the user
- "Graceful degradation" that hides errors is not acceptable - fail explicitly instead

### Comments
- Explain **why**, never **what** — the code shows what, comments explain non-obvious reasoning
- Doc comments: one-line `///` summary for public items; expand only for complex behavior

### Module Organization
- Group related types in one file (e.g., `Message`, `Role`, `ToolCall` together in `llm/mod.rs`)
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
