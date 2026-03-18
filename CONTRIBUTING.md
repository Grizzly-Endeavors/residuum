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

All changes go through pull requests. Direct pushes to `main` are blocked.

1. Create a feature branch from `main` with a descriptive name
2. Make your changes and commit frequently
3. Push your branch and open a PR against `main`
4. CI must pass before merge (formatting, clippy, tests, dependency audit)
5. PRs require one approving review

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
- `cargo test` — full test suite

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

See [docs/residuum-design.md](docs/residuum-design.md) for system architecture and design decisions. The `docs/` directory covers individual subsystems in detail.

## License

By contributing, you agree that your contributions will be licensed under the MIT License.
