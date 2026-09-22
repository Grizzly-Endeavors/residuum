# Contributing to Residuum

Thanks for your interest in contributing. This document covers the workflow and standards for getting changes merged.

## Getting Started

1. Fork the repository and clone your fork
2. Install [Rust 1.85+](https://rustup.rs/) and [Node.js](https://nodejs.org/) (for the web frontend)
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
5. Git hooks are installed automatically — they enforce formatting, linting, and tests on every commit

## Branch & PR Workflow

All changes go through pull requests.

1. Create a feature branch from `main` with a descriptive name
2. Make your changes and commit frequently
3. Push your branch and open a PR against `main`
4. The quality gate is the pre-commit hook, not CI: formatting, clippy, tests, dependency audit, and the web checks all run locally on every commit. A maintainer re-runs them locally before merging a contributor's PR.
5. Cross-platform compile checks (aarch64 Linux, Windows, macOS) are opt-in; see below

### Cross-Platform Compile Checks

The pre-commit hook only builds for your own platform. `.github/workflows/cross-compile.yml` runs `cargo check` for the other release targets, and it runs when:

- the PR carries the `cross-compile` label (adding the label starts a run, and every later push re-runs it)
- the PR changes `Cargo.toml`, `Cargo.lock`, `build.rs`, or `rust-toolchain.toml` (automatic)
- you dispatch it by hand, with or without a PR: `gh workflow run cross-compile.yml --ref <branch>`

Opt in when a change touches anything platform-sensitive: `unsafe` or FFI code, `#[cfg(target_os = ...)]` / `#[cfg(windows)]` / `#[cfg(unix)]` branches, filesystem paths, process spawning, signals, or file permissions. Anything else that slips through gets caught by the release build, which compiles every target.

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
- When `web/src/` files are staged: Prettier, ESLint, `svelte-check`, and the web unit tests

Do not bypass hooks. If a hook fails, fix the issue before committing.

### Style

- **Error messages**: lowercase, no trailing period, include context (`"failed to parse config at {path}"` not `"parse error"`)
- **No silent failures**: every error must be visible to the user — not buried in debug logs
- **Comments**: explain *why*, not *what*
- **Visibility**: private-first — only add `pub(crate)` or `pub` when needed
- **Async-first**: sync only for trivial or CPU-bound work

### Testing

Tests are required for new functionality. Unit tests go in `#[cfg(test)] mod tests` at the bottom of the file. Integration tests go in `tests/`.

Test modules use `#[expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]`.

### Lint Denials

These are denied project-wide and will not be relaxed:
- `unsafe_code`, `unwrap_used`, `expect_used`, `panic`, `todo`, `unimplemented`
- `indexing_slicing`, `string_slice`, `dbg_macro`, `exit`

Any `#[allow]` must be `#[expect]` with a reason string.

## Releases

Releases use [CalVer](https://calver.org/) with the format `YYYY.0M.0D` (e.g., `v2026.03.02`). If multiple releases happen on the same day, a suffix is added: `v2026.03.02-2`.

Releases are automated — pushing a tag matching this format triggers the CI pipeline, which builds cross-platform binaries and creates a GitHub release. Cargo.toml version is not tied to release tags.

## Architecture

See [docs/systems-usage/](docs/systems-usage/) for how each subsystem works — it is the authoritative reference, kept current with the code. [docs/design-philosophy.md](docs/design-philosophy.md) covers the principles behind those choices, and [docs/](docs/) describes the rest of the documentation layout.

## License

By contributing, you agree that your contributions will be licensed under the MIT License.
