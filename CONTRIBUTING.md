# Contributing to Residuum

Thanks for your interest in contributing. This document covers the workflow and standards for getting changes merged.

## Getting Started

1. Fork the repository and clone your fork
2. Install [Rust](https://rustup.rs/) (the version pinned in `rust-toolchain.toml`; rustup installs it — see [Rust Toolchain](#rust-toolchain)) and [Node.js](https://nodejs.org/) (for the web frontend; the supported versions are the `engines` field in `web/package.json`, see [web/CONTRIBUTING.md](web/CONTRIBUTING.md#prerequisites))
3. Build the web frontend first — the Rust binary embeds the built assets, so `cargo build` will fail without them (the member crates under `crates/` build without them, so `cargo test --quiet -p residuum-memory` does not need this step):
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
5. Install the git hooks — they format and check every commit, and run clippy, the tests for the code you changed, and the web checks on every push:
   ```bash
   .githooks/install.sh
   ```

If you have [`just`](https://github.com/casey/just) installed, `just setup` installs the web dependencies (step 3) and the git hooks (step 5) in one go, and `just` on its own lists everything else: `just check` runs everything CI runs except the web end-to-end suite and lists every failing check in one run, `just test gateway::` runs one root module's tests, `just test-crate memory` runs one member crate's tests (see [Workspace layout](#workspace-layout)), and `just web-mock` starts the web UI against a mock API with no backend. Every recipe wraps a command documented here, so `just` is optional.

## Branch & PR Workflow

All changes go through pull requests, and `main` changes only through the merge queue.

1. Create a feature branch from `main` with a descriptive name
2. Make your changes and commit frequently
3. Push your branch and open a PR against `main`, with `closes #N` in the PR body for each issue it closes
4. The git hooks are the first quality gate: formatting and the dependency audit run on every commit; clippy, the tests for the code you changed (and the source scans), the generated-types check, the unused-dependency check, and the web checks run on every push, against your branch as pushed. A conflict with `main` shows on the pull request, and the merge queue checks the merged result. CI runs the full suite once, in the merge queue, on the batch merged onto `main`: the Rust checks when the batch changes Rust code and the web checks when it changes `web/`, over the whole workspace, and its `CI gate` check sums them up (see [Quality Gates](#quality-gates)). On the PR itself CI runs only two early warnings that never gate it, the Windows clippy and the visual comparisons, and `CI gate` passes there at once, so an enqueued PR goes straight into the queue. CI runs on self-hosted runners, so a PR from a fork runs no checks of its own: they run on the merge queue batch once a maintainer enqueues it, and a maintainer can run the checks locally before enqueueing a contributor's PR.
5. Clippy for Windows runs on every Rust PR as an early warning and in the merge queue as a gate; the other cross-platform checks (clippy for aarch64 Linux and macOS, plus tests on Windows) run in the merge queue when a batch changes dependencies or the toolchain, and on a PR when asked for; see below
6. The PR merges through the merge queue (see [Merging](#merging))

### Merging

`main` has a ruleset, kept in [`.github/rulesets/main.json`](.github/rulesets/main.json): it refuses a push to `main`, a force push or its deletion, requires a pull request (no approvals needed), and merges a pull request only through the merge queue, as a merge commit, once its `CI gate` check has passed. The queue merges a PR onto the tip of `main` together with any PRs ahead of it (up to five at once), runs CI on that batch, and merges the batch only when every PR in it passes.

- Never push to `main`, and never merge locally and push the result: the push is refused.
- Enqueue a PR with `gh pr merge <n> --auto`, or the "Merge when ready" button. This turns on auto-merge, and GitHub adds the PR to the queue once `CI gate` passes on it, which on a pull request takes only as long as the `changes` job: the PR's early warnings (the Windows clippy and the visual comparisons) do not hold it back, so look at them, and fix a Windows clippy failure, before enqueueing. The merge happens later, not when the command returns.
- Put `closes #N` in the PR body (`gh pr create --body`, or `gh pr edit <n> --body`). The queue writes the merge commit's message, so there is no merge commit message of your own to put it in, and a keyword in a PR comment closes nothing.
- See where a PR stands with `gh pr view <n> --json state,mergeStateStatus,autoMergeRequest,statusCheckRollup` (`autoMergeRequest` is set while it waits to enter the queue, and `state` is `MERGED` once it has merged), its place in the queue with `gh api graphql -f query='query($n:Int!){repository(owner:"Grizzly-Endeavors",name:"residuum"){pullRequest(number:$n){mergeQueueEntry{position state}}}}' -F n=<n>`, and the whole queue at https://github.com/Grizzly-Endeavors/residuum/queue/main.
- A PR ejected from the queue (its batch failed CI, it conflicts with a PR merged ahead of it, or its checks timed out after four hours) is removed, with the reason on its timeline. Fix the cause on the branch, push, and enqueue it again. `just ci-failures <n>` lists the failures of the PR's own runs and of its latest merge queue batch that did not pass.
- A fork's PR shows a passing `CI gate` without having run anything. Enqueueing it runs its code, including any change it makes under `.github/`, on the self-hosted runners, so read the whole diff before enqueueing a fork's PR.
- Repository admins can bypass the ruleset in an emergency, such as CI itself being broken, and only through a PR: `gh pr merge <n> --merge --admin` merges past the queue and its checks. Nobody can push to `main` directly.

To change the ruleset, edit `.github/rulesets/main.json` in a PR, and once it merges, apply it with `gh api --method PUT repos/Grizzly-Endeavors/residuum/rulesets/<id> --input .github/rulesets/main.json`, where `gh api repos/Grizzly-Endeavors/residuum/rulesets --jq '.[] | select(.name == "main") | .id'` prints the id. A repository without the ruleset gets it with `gh api --method POST repos/Grizzly-Endeavors/residuum/rulesets --input .github/rulesets/main.json`, and the repository needs auto-merge allowed and head branches deleted on merge for the flow above: `gh api --method PATCH repos/Grizzly-Endeavors/residuum -F allow_auto_merge=true -F delete_branch_on_merge=true`. The file is the ruleset as it should be: a change made in the repository settings is lost at the next apply, so make it in the file.

### Rust Toolchain

The Rust version is pinned in `rust-toolchain.toml`, and rustup installs exactly that version for every build: your machine, CI, and releases. Everyone lints with the same clippy, so code that passes the git hooks passes CI. Bumping the version is its own PR: change the pin, fix whatever new lints the release brings, and commit.

### Cross-Platform Checks

The git hooks only build for your own platform. Every merge queue batch that runs the Rust checks also runs `cargo clippy --all-targets --all-features --target x86_64-pc-windows-gnu -- -D warnings` over the whole workspace (the `Clippy (x86_64-pc-windows-gnu)` job in `.github/workflows/rust-checks.yml`), so code behind `cfg(windows)`, and test code that only some platforms use, is held to the same lints as your own build before it reaches `main`. Every PR that changes Rust runs the same clippy over its affected packages (the `Windows Clippy` job of `ci.yml`, which calls `rust-checks.yml` with `windows_clippy_only`), as an early warning that does not gate the PR, since your own hooks cannot catch these failures.

`.github/workflows/cross-compile.yml` adds clippy for aarch64 Linux and macOS, and runs the full test suite on a Windows runner, where `cfg(windows)` code actually executes. The merge queue runs it for every batch that changes `Cargo.toml`, `Cargo.lock`, `build.rs`, `rust-toolchain.toml`, a member's `crates/*/Cargo.toml`, or `.github/workflows/cross-compile.yml`, as part of `CI gate`, so a batch that fails it does not merge. On a pull request it reports without gating the merge, and runs when:

- the PR carries the `cross-compile` label (adding the label starts a run, and every later push re-runs it)
- the PR changes `Cargo.toml`, `Cargo.lock`, `build.rs`, `rust-toolchain.toml`, or a member's `crates/*/Cargo.toml` (automatic)
- you dispatch it by hand, with or without a PR: `gh workflow run cross-compile.yml --ref <branch>`

Fix what it reports before you enqueue the PR. Opt in when a change touches anything platform-sensitive: `unsafe` or FFI code, `#[cfg(target_os = ...)]` / `#[cfg(windows)]` / `#[cfg(unix)]` branches, filesystem paths, process spawning, signals, or file permissions. Anything else that slips through gets caught by the release build, which compiles every target.

From Linux, the Windows and aarch64 Linux targets can also be linted locally with `just cross-clippy` (Windows) or `just cross-clippy aarch64-unknown-linux-gnu`, which install the target first; `just cross-check` lints both.

To run Windows-specific code yourself, the local VM harness in `scripts/windows-vm/` runs the test suite and desktop checks (such as toast notifications) on Windows 11; see [docs/runbooks/windows-harness.md](docs/runbooks/windows-harness.md).

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

The git hooks split the checks between commit and push: a commit runs the quick checks on what is staged, and the slow ones run once per push.

On every commit (pre-commit), on what is staged:
- `cargo fmt` — formatting (auto-applied and staged)
- Prettier on the staged web files (auto-applied and staged)
- `cargo deny check` — dependency audit, when a manifest (`*.toml`, `Cargo.lock`) is staged; the commit is refused when `cargo-deny` is not installed (`cargo install --locked cargo-deny`)
- `scripts/ci-failures-test.sh` and `scripts/stress-issues-test.sh` — the `just ci-failures` log parser and merge queue batch selection against their fixtures, and a dry run of the nightly stress issue filer over them, when a `scripts/ci-failures*` or `scripts/stress-issues*` file is staged
- `scripts/affected-packages-test.sh` — the package selection behind the pre-push hook and the CI `changes` job, against a scratch workspace, when a `scripts/affected-packages*` file is staged
- No `dbg!()` or `#[allow(...)]` in the staged diff, and no growth of the wall-clock backlog

On every push (pre-push), against the pushed commits as they are, without merging `main` into them (a conflict with `main` shows on the pull request, and the merge queue checks the merged result). A commit is checked in place when it is the checkout's HEAD with a clean tree, and otherwise in the stable tree at `target/pre-push/tree`, or in a temporary worktree of that commit when another push holds the stable tree or it cannot be prepared:
- When the pushed commits change what the Rust checks run on (the paths for which the `changes` job of `.github/workflows/ci.yml` runs the Rust checks, such as `src/`, `tests/`, `assets/`, `crates/`, the Cargo and lint configuration, and the generated web types in `web/src/lib/generated/`): `cargo clippy` (pedantic linting with strict denials) over the packages the changed paths touch and the packages that depend on them, the tests for the code the push touches, `scripts/check-generated-types.sh`, which checks that the web app's TypeScript protocol types match the Rust types, and `cargo shear --locked --deny-warnings` (see below). The hook picks the packages and tests from the changed paths and prints what it chose:
  - A change under `src/<module>/` or in `src/<module>.rs` runs that module's root unit tests (`cargo test --lib` for `residuum`, filtered to `<module>::`, plus the bin's tests when `src/main.rs` declares the module).
  - A change to `src/lib.rs` or `src/main.rs` runs every root unit test.
  - A changed path runs the `it` modules whose `//! covers:` line names the package it belongs to: a path inside a module selects that module alone, a member's path the modules covering that member, and a root path the modules covering `residuum`. `tests/it/main.rs` and `tests/it/support/` run the whole `it` binary, and a change to `tests/ts_export.rs` runs that test. A change under `tests/fixtures/` or `assets/` runs every unit test of every package and every root integration test. Integration tests other than `ts_export` are built only when a path selects them (a `tests/` path, or a covers line naming a changed package); the generated-types check builds `ts_export` on every Rust push.
  - A change to a member crate under `crates/<dir>/` runs that crate's tests and the `it` modules whose covers line names it, but not the tests of the crates that depend on it (clippy checks those). A change to a member's `Cargo.toml`, to a root manifest (`Cargo.toml`, `Cargo.lock`), `rust-toolchain.toml`, `clippy.toml`, `rustfmt.toml`, `deny.toml`, `.cargo/`, or a `crates/` path outside a member runs the `testing::manifest_scan` test.
  - The source scans (`testing::source_scan`) run on every push that changes Rust: the test-code waits and executable scripts ([Waiting in tests](#waiting-in-tests)), bare spawns in production code, reads of the build version outside `src/lib.rs`, and `pub` items of member crates that nothing outside their own crate's tests uses ([Where the rules are enforced](#where-the-rules-are-enforced)).
  - A change to `build.rs`, the generated web types, or the scripts and workflows the checks use adds no tests of its own. A change to a file the Rust tests read by path runs the member tests of `residuum-hub` when the hub is a member crate with the file that reads it (`web/app-shell-csp.txt`: `crates/hub/src/web/assets.rs`; `.github/workflows/release.yml`: `crates/hub/src/update.rs`), and every root unit test otherwise; a change to `web/src/styles/tokens.css`, which `crates/net/src/browser_landing.rs` embeds, runs `residuum-net`'s tests.
  - When the selection script is missing or fails, clippy checks the whole workspace and the member tests cover every member other than `residuum`.
  - `cargo shear` reports unused dependencies, unused `[workspace.dependencies]` entries and unlinked source files, always across the whole workspace. It reads the manifests and sources and builds nothing, so it runs before the web build and whatever the other checks do. CI pins the `cargo-shear` version in `.github/workflows/rust-checks.yml`. A machine without `cargo-shear` runs the other checks, and the summary at the end says the check did not run and names the install command (`cargo install --locked cargo-shear --version <pinned>`); a different installed version runs, with a note in the summary. `just shear` runs the check and asks for the pinned version when none or another is installed.
- When they change `web/` (Markdown under `web/` does not count, since no build or test reads it): ESLint and the style lint (Stylelint), `svelte-check` (warnings fail it), and the web unit and component tests (Vitest) that import a changed file. A change to `web/package.json`, the lockfile, the Vite, Vitest, Svelte or TypeScript configuration, the component test setup file, the hang guard (`web/src/test/guard.ts`) or the service worker plugin (`web/build/service-worker.ts`) runs the whole web suite.

The hook runs the tests of the code you changed, not the whole suite, and CI runs the whole suite. When a change reaches code that other modules depend on (a shared helper, a trait other modules implement), run those modules' tests with `just test <filter>` (`just test-crate <crate> <filter>` for a member crate's), or the whole suite with `cargo test --quiet`, before you push. A change to a member crate runs the tests of that crate and of the member crates above it, not those of the root modules that use it; clippy still compiles the root against it.

Every push check runs even after one fails, and the hook ends with a summary of what failed. Do not bypass the hooks: `--no-verify` is not allowed on `git commit` or `git push`. If a hook fails, fix the issue and try again.

CI repeats these checks and adds more, once, in the merge queue. `.github/workflows/ci.yml` runs on every pull request and every merge queue batch. Its `changes` job reads the diff. On a merge queue batch it starts the Rust checks (`rust-checks.yml`) when Rust code or what the build embeds changed (`src/`, `tests/`, `assets/`, `crates/`, the Cargo and lint configuration, the generated web types, the package selection and isolation scripts), the web checks (`web-checks.yml`) when `web/` changed (Markdown under `web/` excepted, since no build or test reads it), a `docker build` of `docker/Dockerfile` (the `Docker Source Image` job, on a runner with Docker, without pushing the image) when the batch changes `docker/` or `.dockerignore`, and actionlint (`actionlint.yml`) when a workflow or `.github/actionlint.yaml` changed. The Rust checks cover the whole workspace, and their `rust-isolation` job checks each member crate alone, and the root library and binary without dev-dependency features, with `scripts/check-crate-isolation.sh`. On a pull request it starts only the early warnings, which never gate it: the Windows clippy when Rust code changed, over the packages the diff touches and the packages that depend on them, as `scripts/affected-packages.sh` selects them (the job summary prints the selection), and the visual comparisons when `web/` changed. Its `CI gate` job is the one required status check. On a merge queue batch it fails when a job it needs failed or was cancelled, and passes when each one succeeded or was skipped because its files did not change, so a batch that touches neither Rust nor web code still merges. On a pull request it passes as soon as the `changes` job has finished, so the PR enters the queue without waiting on the early warnings.

| Stage | Runs on | What it runs |
|-------|---------|--------------|
| pre-commit | every commit | Formatting of what is staged, `cargo deny` when a manifest is staged, the `dbg!()` / `#[allow]` / wall-clock-backlog blocks, the CI log parser and stress issue filer fixtures and the package selection test when those are staged |
| pre-push | every push, as pushed (not merged with `main`) | Clippy, the tests of the code the push touches, the generated-types check and `cargo shear` (unused dependencies) when Rust paths change; ESLint, Stylelint, `svelte-check` and Vitest (the tests that import a changed file) when `web/` changes |
| PR lane (`ci.yml`, `pull_request`) | every push to a pull request | Two early warnings, neither of which gates the PR: the Windows clippy (`Windows Clippy`) when Rust paths change, over the affected packages (the packages the change touches and the packages that depend on them, as `scripts/affected-packages.sh` selects them, or the whole workspace when it cannot select them), and the visual comparisons when web paths change, which never fail and report in one PR comment. `CI gate` passes once the `changes` job has read the diff, without waiting for them, so `gh pr merge <n> --auto` puts the PR straight into the queue |
| Merge queue lane (`ci.yml`, `merge_group`) | every batch the merge queue builds, merged onto `main` | The full suite, which runs nowhere else before a change reaches `main`. Rust (when Rust paths change), over the whole workspace: fmt, host clippy, Windows clippy, rustdoc with warnings denied, `cargo deny`, `cargo shear`, `cargo test`, the Pebble tests, generated types, and the crate isolation job (`scripts/check-crate-isolation.sh` checks each member crate alone and the root library and binary without dev-dependency features, so a crate that compiles only because another crate turns on a feature for it, or production code that uses a `test-support` item, fails here). Web (when web paths change): format, lint, types, unit tests on Node 22 and 24, build, the four end-to-end shards; no visual comparisons. The source image (`Docker Source Image`: `docker build` of `docker/Dockerfile`, not pushed) when the batch changes `docker/` or `.dockerignore`. `cross-compile.yml` (aarch64 Linux and macOS clippy, the tests on Windows) when the batch changes `Cargo.toml`, `Cargo.lock`, `build.rs`, `rust-toolchain.toml`, a member's `crates/*/Cargo.toml` or `.github/workflows/cross-compile.yml`. actionlint when a workflow changes. `CI gate` must pass for the batch to merge |
| Nightly (`nightly-stress.yml`) | 07:23 UTC on `main`, or by hand | The Rust suite unloaded, then as two concurrent copies under CPU load for three rounds; every end-to-end spec three times with the CPU throttled. Each failing test becomes an `investigate` issue, and a run that timed out gets one saying it did not finish. It gates nothing |
| Release (`release.yml`) | a `v*` tag | The merge queue lane's Rust and web checks (no visual comparisons, no stress; the Rust checks cover the whole workspace and include the crate isolation job) beside the release builds; nothing is published unless both pass |

The Rust checks run `cargo fmt --check`, clippy with `-D warnings` for your host and for the Windows target (`Clippy (x86_64-pc-windows-gnu)`; see Cross-Platform Checks), `cargo deny check`, the rustdoc build with warnings denied (`cargo doc --workspace --no-deps --all-features --document-private-items` under `RUSTDOCFLAGS=-D warnings`, which `just doc` runs; a broken intra-doc link fails it, in the docs of private items as well as public ones), `cargo shear --locked --deny-warnings` (the `cargo-shear` release archive pinned in `.github/workflows/rust-checks.yml`, with its sha256 checked before it runs), the tests (of the whole workspace), and the generated-types check, which regenerates the web's TypeScript protocol types from the Rust types and fails when `web/src/lib/generated/` differs from what is committed, so after changing an exported Rust type run `just types` and commit the result (`just types-check` runs the same check as CI). The merge queue lane and releases add the Pebble tests (`just pebble`), which issue certificates from the Let's Encrypt test CA in Docker. The web checks run the format, lint, type and unit checks, with web test coverage in the job summary and no threshold, the unit and component tests again on Node 22 and Node 24 (`Web Tests (Node 22)` and `Web Tests (Node 24)`), the build, and the end-to-end and accessibility suite against the mock in four shards, in the Playwright container's browser, so the runners need no Chromium of their own (`just web-e2e` runs it locally together with the visual comparisons; `just web-e2e-fast` leaves out the visual comparisons and runs in your own Chromium, or in the Playwright container when Chromium can't be installed; see [web/CONTRIBUTING.md](web/CONTRIBUTING.md#testing)). On pull requests, and only there, the visual comparisons, a design reference of component screenshots, run in a job of their own that never fails the PR, and report their changed shots in one pull request comment, updated in place, with a link to the diffs. actionlint runs with shellcheck over the `run:` blocks, at the versions pinned in `.github/workflows/actionlint.yml`; `just actionlint` runs it locally and needs `actionlint` and `shellcheck` on `PATH`. A failing check does not stop the others: every test binary runs, and the format, lint, type and test steps each run, so one CI run lists every failure.

When a CI run fails, `just ci-failures` lists every failure of the latest runs for your branch (failing tests, lint, type and format errors, and jobs or steps that ran out of time, with the step and its last lines), grouped by job and step, so one pass can fix them all. It takes a PR number, a branch, a release tag or a commit instead when you want another one. A run still in progress is marked so, with how many of its jobs have finished, and the failures of the jobs that have finished are listed. An earlier run of a workflow on the same commit is listed too when a later run did not rerun a job that failed in it, as Cross-Platform Checks does when it runs again for an unrelated label. For a PR number, the latest merge queue batch of that PR is listed too when it did not pass, since the full suite runs only there: that is how an ejected PR's failing tests show up, beside any early warning that failed on the PR itself. Each failure carries two flags: `touched` when this change edits a file the failure names (the failing test's file or, for a Rust failure, the file its panic was raised in) or, for a test of a `tests/it` module, reaches a package that module's covers line names, and `seen elsewhere` when the same failure was in a failed run on another branch or tag in the last 14 days. A failure that is not `touched` but is `seen elsewhere` is marked `likely flake`: it may be a flake, or a breakage other branches share, so check `main` before treating it as this change's. Every other failure is presumed to come from this change. Failed jobs' logs are cached under `target/ci-failures-cache/`. The log parser and the merge queue batch selection have fixtures: `just ci-failures-test` checks both.

### Style

- **Error messages**: lowercase, no trailing period, include context (`"failed to parse config at {path}"` not `"parse error"`)
- **No silent failures**: every error must be visible to the user — not buried in debug logs
- **Comments**: explain *why*, not *what*
- **Visibility**: private-first — only add `pub(crate)` or `pub` when needed
- **Async-first**: sync only for trivial or CPU-bound work

### Testing

Tests are required for new functionality. Unit tests go in `#[cfg(test)] mod tests` at the bottom of the file. Integration tests are modules of the one test binary in `tests/it/`: add a file there and a `mod` line to `tests/it/main.rs`, and `cargo test --test it gateway::` runs one module. A module's leading doc comment carries exactly one `//! covers: <package>, …` line naming the packages it tests (`residuum` for the root); the pre-push hook runs the modules that cover a changed package. They share a process, so a test doesn't change environment variables, the current directory or the global tracing subscriber. Every file directly in `tests/` is a test binary of its own that links the root library and every member crate; `tests/ts_export.rs`, which writes the web app's generated types and which the type scripts run by name, is the only one. Member crates have no `tests/` directory: their tests are unit tests, and an integration test that needs one goes in `tests/it/`. Tests that need root modules from more than one layer of the crate graph (the hub's host and runtime with the gateway's launcher, the hub's team file routes with the agents' file tools, the A2A server with the tools' session tools) are modules of `src/integration/`, a `#[cfg(test)]` root module: each carries the same covers line and runs as root unit tests (`cargo test --quiet -p residuum --lib integration::hub_host::`).

A test filter is a substring match on a test's path, and a member crate's test paths start below the crate: `residuum-memory`'s search tests are `search::tests::…`, not `memory::search::tests::…`. So `just test memory::` runs the root tests whose path contains `memory::` and none of `residuum-memory`'s. `just test-crate memory search::` (`cargo test --quiet --no-fail-fast -p residuum-memory search::`) runs them, building only that crate and the crates below it, with no web build. `just test` and `just stress` print a note when a filter's first segment names a member crate or a module inside one, and a warning when the run matched no test. For a member crate, `just test` also lists each `tests/it` module whose covers line names that crate, with the `cargo test --quiet --test it <m>::` command that runs it.

A test waits on the event it checks, never on elapsed time: a duration that holds on an idle machine fails when several builds share it. A flaky test is fixed by making it wait on an event the code exposes, adding that signal to the code when there is none, never by widening a timeout, adding a sleep, or rerunning past it. The suites under load run nightly on `main`, in `.github/workflows/nightly-stress.yml`, not on pull requests or in the merge queue. The Rust suite runs once unloaded, then as two concurrent copies with CPU burners alongside, three rounds over (`scripts/stress-tests.sh`); every end-to-end spec runs three times with the browser's CPU throttled, split over four runners (`scripts/web-stress.sh` with `WEB_STRESS_ALL=1`). `scripts/stress-issues.sh` files each test that failed as an `investigate` issue titled `Nightly stress: <test>` (a root test by its path, `gateway::web::sessions::tests::…`; a member crate's with its crate first, `residuum_memory::search::tests::…`), with the run link, the commit and the failure's messages, and adds a comment to the open issue instead when one has that title, so a test that keeps failing collects its runs in one place. A run with more than ten new failing tests files one issue for the run instead, listing them all, so a broken `main` or runner does not flood the tracker. A Rust failure says whether it failed under load only, so a test waiting on time (fix the wait; a rerun that happens to pass hides it), or also in the unloaded run, an ordinary failure to fix as any failing test. A job that fails without naming a test (a broken build, a runner that went away) is filed as one issue for its suite, and a job cancelled by its timeout is filed as one saying the run did not finish. Reproduce a failure with `just stress <filter>` (`just stress -p residuum-<crate> <filter>` for a member crate's test), which runs the tests as two concurrent copies with CPU burners alongside (`STRESS_COPIES`, `STRESS_ROUNDS` and `STRESS_BURN` tune it), or `WEB_STRESS_ALL=1 just web-stress <spec>` for a spec. A test you add or fix must pass `just stress <filter>` before you push (a member crate's test: `just stress -p residuum-<crate> <filter>`); the nightly run is the backstop, not the first check. `gh workflow run nightly-stress.yml` runs the nightly suites on `main` by hand. `just stress-issues-test` checks the issue filer against the log parser's fixtures, without GitHub.

#### Waiting in tests

Test code waits through `residuum_core::testing::{wait, gate, clock}`, and through `residuum_bus::testing` for what the bus publishes, never with `tokio::time::sleep`, `tokio::time::timeout`, `std::thread::sleep` or wiremock's `set_delay`. Core's `testing` is behind the `test-support` feature of `residuum-core`, which every other package turns on in its `[dev-dependencies]`; core's own tests reach it as `crate::testing`, and root tests name it `residuum_core::testing::…`, since the root's own `testing` module takes the name `crate::testing`. In the list below, `wait`, `gate` and `clock` are those modules of core's, and the bus's waits are `residuum_bus::testing`. A source-scan test (`testing::source_scan`, in the root, over `src/`, `tests/` and every member crate's sources) fails on any of those outside core's `testing::wait` and `testing::clock`; the pre-push hook runs it on every push that changes Rust. The waits that predate the rule are counted per file in `src/testing/wall_clock_backlog.rs`, and the counts only go down: converting a file's waits means lowering its entry, and the hook refuses a commit that adds one.

- **Something will arrive on a channel or watch.** `wait::next`, `next_matching` (skips to the first match), `next_where` (everything received up to the match) or `watch_until`; a bus `Subscriber` is waited on with `residuum_bus::testing::next_event`. For state with no channel to wait on, `wait::until` polls a check. Each runs under `wait::HANG_GUARD`, the suite's only deadline, which exists to stop a hung test and names what it was waiting for; never shorten it or add another.
- **A state must last while the test looks at it** (an agent mid-turn, a session queued behind another). Hold the call that would end it at a closed `gate::Gate`: `gate.until_held(1)` proves the call is waiting, the test looks, then `gate.release(1)`. The model has gated versions: inference's `testing::model::GatedModel` (an HTTP model in front of a wiremock server, reached from the root as `crate::inference::testing::model`; the hub test fixture puts one in front of every agent's mock), `GatedProvider` in process (same path), and the scripted stream server's `Step::Hold`.
- **Background work must have happened.** Wait on the signal the code gives for it. When there is none, add one to the code: a test that needs to know something finished is usually a sign users and diagnostics need to know too.
- **Something must not happen.** Only after a positive sync point that proves it would have happened by now: a bus barrier (`residuum_bus::testing::bus_barrier`, which publishes a marker and waits for it; the broker hands a message to every subscriber at once, so once a subscriber has a later message, every subscriber has the earlier one), a sentinel file written after the cause (file notifications arrive in order), a stop (an agent's watcher drains before `stop` returns), or a pending count back at zero. Then check with `wait::drain` or the current state.
- **The code under test is timer-driven and has no real I/O** (no sockets, child processes or file watchers). Run it on a paused clock (`#[tokio::test(start_paused = true)]`) and move time with `clock::elapse`; `clock::within` checks that something does not complete within a window. Both refuse to run on a real clock.
- **A timeout on a child process must fire.** Start on a real clock, wait for the child to show it has started (a marker file it writes), then `tokio::time::pause()` and jump past the timeout with `tokio::time::advance`. Don't leave a paused clock to advance on its own while a child runs: on Windows tokio reads a child's pipes on blocking threads, and a running blocking task holds a paused clock still, so the test waits for the child to exit instead.

A test that needs a fake tool writes it with `residuum_core::testing::script::write_executable`, which writes the script through a child shell so the test process never holds a write descriptor a forked child could inherit; writing a script in the test and executing it fails with "Text file busy" when another test thread forks at the wrong moment, and far more often under load. The same source scan fails on test code that makes a file executable itself (a `"chmod"` command, or an execute-bit mode literal passed to `from_mode`, `set_mode` or `.mode(`), and a directory restored to a mode with its execute bit is listed with its reason in `EXEC_MODE_ALLOWED` in `src/testing/source_scan.rs`.

`clippy.toml` exempts `unwrap_used`, `expect_used`, `panic`, and `dbg_macro` inside `#[cfg(test)]` modules and `#[test]` functions, so unit tests use unwrap, expect, panic, and `dbg!` with no suppression. Do not add `#[expect(clippy::unwrap_used)]` (or `expect_used`, `panic`, `dbg_macro`) on that code: the lint is already exempt, the expectation never fires, and `-D warnings` fails the build. Code behind `#[cfg(any(test, feature = "test-support"))]` is different: clippy does not read that gate as test code, and `cargo clippy --all-features` compiles it with the feature on, as production code. An unwrap, expect or panic in a `test-support` helper therefore carries a scoped `#[expect(clippy::…, reason = "test-support helper: …")]`, which that build fulfills.

`clippy::tests_outside_test_module` is denied and is not controllable from `clippy.toml`. Integration tests under `tests/` need `#![expect(clippy::tests_outside_test_module, reason = "...")]` at the top of the module's file. A helper at the top level of a `tests/it/*.rs` module — not inside `#[cfg(test)]`, and not itself a `#[test]` — still needs its own `#[expect]` when it uses unwrap, expect, panic, or `dbg!`.

### Lint Denials

These are denied in `[workspace.lints.rust]` and `[workspace.lints.clippy]` of the root `Cargo.toml`, which the root package and every member crate inherit through `[lints] workspace = true`, and will not be relaxed without explicit approval:
- `unsafe_code` (`deny`, not `forbid`, so a real FFI boundary can carry `#[expect(unsafe_code, reason = "...")]` on that item)
- `unwrap_used`, `expect_used`, `panic`, `todo`, `unimplemented`
- `missing_errors_doc`, `missing_panics_doc`, `must_use_candidate`, `print_stderr`
- `indexing_slicing`, `string_slice`, `dbg_macro`, `exit`

`unwrap_used`, `expect_used`, `panic`, and `dbg_macro` are exempt inside `#[cfg(test)]` modules and `#[test]` functions via `clippy.toml`; see Testing. Every other denial is in the root `Cargo.toml`'s `[workspace.lints]` tables. `clippy.toml`, `rustfmt.toml` and `deny.toml` sit at the repository root and apply to every crate.

Any `#[allow]` must be `#[expect]` with a reason string.

## Workspace layout

The repository is one Cargo workspace. The root `Cargo.toml` is both the workspace manifest and the `residuum` package: the library, the `residuum` binary, `build.rs`, the embedded `web/dist` and the integration tests in `tests/`. The lower layers are member crates in `crates/<dir>/`, where `<dir>` is the package name without its `residuum-` prefix. `members` and `default-members` both cover `crates/*`, so `cargo build`, `cargo test` and `cargo clippy` from the repository root cover every package, and `-p residuum-<dir>` narrows a command to one.

### The crates

| crate | directory | holds | depends on |
|---|---|---|---|
| `residuum-core` | `crates/core` | `util` (atomic file writes and other filesystem primitives, spawn helpers, frontmatter parsing, backoff, `${VAR}` expansion, secret comparison), `time`, the diagnostic types (`Diagnostic`, `Location`, `Severity`), `build_info` (`BuildInfo`, the build's version and commit), `process` (process-tree management and the shell runner), and `testing` behind `test-support` | — |
| `residuum-config` | `crates/config` | `config` (loading, resolution, patching, the secret store, paths), `agent_keys`, `WorkspaceLayout`, and the plain data types config resolves (`ThinkingConfig`, `RetryConfig`, `SkillDir`, `SharedToolsPath`) | core |
| `residuum-skills` | `crates/skills` | `skills`: the skill index, the `SKILL.md` parser and diagnostics, skill state | core, config |
| `residuum-inference` | `crates/inference` | `inference`: the providers, the provider chain and failover, embeddings, the System One client, the tool-call result types (`ToolError`, `ToolResult`), and `testing::model` behind `test-support` | core, config |
| `residuum-tracing-service` | `crates/tracing-service` | `tracing_service`: trace export to OTEL endpoints, sanitizing, bug-report submission, and `client_context` (the metadata attached to a submission) | core, config |
| `residuum-memory` | `crates/memory` | `memory`: the observer and reflector, search, the vector store, the episode store, the wiki index, and `testing` behind `test-support` | core, config, inference |
| `residuum-mcp` | `crates/mcp` | `mcp`: the MCP client and the server registry | core, config, inference |
| `residuum-net` | `crates/net` | `tunnel`, `remote_access`, `pairing`, `workbench`, and the request helpers they share with the hub (`browser_landing`, `cross_site`, `remote_control_guard`) | core, config |
| `residuum-bus` | `crates/bus` | `bus`: the topic broker and the payloads it carries (the session vocabulary, message origins, attachments, usage, workspace changes, interrupts), and `testing` behind `test-support` | core, config, inference, memory |
| `residuum-workspace` | `crates/workspace` | `workspace`: identity files, bootstrapping, team files, the config, MCP patching, and the change feed (`watch`); `inbox`, the user and agent inboxes and the inbound attachment code; and `testing` behind `test-support` | core, config, bus, inference, mcp |
| `residuum-agent` | `crates/agent` | `agent`: the turn loop, context assembly, the auto mode gate, usage, the `Tool` trait and the `ToolRegistry` container, the `TeamRoster` trait, and `testing` (the test tools) behind `test-support`; `subconscious`, the mid-turn and end-of-turn watch | core, config, bus, inference, memory, mcp, skills, workspace |
| `residuum-background` | `crates/background` | `background`: agent sessions (the session registry, runtime and store, the spawn listener and fork construction, per-run memory, owner sessions), agent messaging, the `team` slice (the team router, `TeamDirectory`, the agent lifecycle types), the `session_tools` factory trait, and `testing` behind `test-support` | core, config, bus, inference, memory, mcp, skills, tracing-service, workspace, checkpoints, agent (dev: notify) |
| `residuum-a2a` | `crates/a2a` | `a2a`: the caller-key store, the agent cards, the hub's A2A listener and its per-request auth, the per-agent server (`agent_a2a_router`), the session executor, the task store, the outbound client (`client`), `public_url`, and `diagnostics_dispatch`; `StaticAgentDirectory` behind `test-support` | core, config, bus, agent, background, workspace, inference, skills, proactivity, net |
| `residuum-tools` | `crates/tools` | `tools`: the agent-invoked tools and their registration functions (`registration`), the main registry builder and its parity test (`main_tools`), the session tool factory (`SessionTools`), and `testing` (`RegistryHarness`, `tools_for`) behind `test-support` | core, config, bus, inference, memory, skills, tracing-service, workspace, checkpoints, proactivity, agent, background, a2a |
| `residuum-checkpoints` | `crates/checkpoints` | `checkpoints`: the workspace checkpoint history (the engine, the repository backend, notices and types), and `testing` behind `test-support` | core, config, bus, workspace |
| `residuum-interfaces` | `crates/interfaces` | `interfaces`: the Discord, Telegram and Teams adapters (their subscribers, handlers and shared chat state), the webhook table, the outbound and boot-retry helpers, `AdapterSenders`, and the Teams app setup (`teams::{atk, atk_runner, setup_job, setup_types}`) | core, config, bus, inference, net, workspace, checkpoints, background (dev: memory) |
| `residuum-protocol` | `crates/protocol` | `protocol`: the WebSocket client and server frames and the payloads they carry (the ts-exported wire types), and `frames`, the conversions from bus session events and workspace changes to frames | bus, inference, workspace (dev: config, memory) |
| `residuum-proactivity` | `crates/proactivity` | `pulse` (the heartbeat pulses: scheduling, scripts, the edit and next-run helpers, and `testing` behind `test-support`) and `actions` (scheduled actions and their store) | core, config, bus (dev: workspace) |
| `residuum-notify` | `crates/notify` | `notify`: the notification channels (ntfy, webhook, the inbox, and the native macOS and Windows channels), the router and the subscriber, and `channel_builder`, whose `build_external_channels` is re-exported at the crate root | core, config, bus, workspace |
| `residuum-hub` | `crates/hub` | `hub`: the agent host, the agent directory, provisioning, the hub's HTTP app and WebSocket (`http`), the web routes it serves and the embedded app's file type (`web`), `file_server`, `launch` (the `AgentLauncher` trait and the types the launcher hands back), push, the overview and the team event log, `update`, `features`, `config_watcher` and `last_known_good`, and `testing` behind `test-support` | core, config, bus, inference, tracing-service, memory, mcp, net, workspace, checkpoints, proactivity, agent, background, a2a, protocol, interfaces |
| `residuum-gateway` | `crates/gateway` | `gateway`: a running agent's event loop, startup, reload and routes, the agent's `web` routes (sessions, scheduled, memory, model, the A2A status routes, the inbox), the WebSocket subscriber (`websocket`), `turn_journal`, and `GatewayLauncher`, which implements the hub's `AgentLauncher` | core, config, bus, inference, memory, net, mcp, a2a, tools, skills, tracing-service, agent, background, interfaces, workspace, protocol, notify, proactivity, checkpoints, hub |
| `residuum` | `.` | everything else: `daemon`, `wiring`, `logging`, `setup_wizard`, the `commands` of the binary, and the facade over the member crates | every member |

```
                 residuum-gateway
                         │
             residuum-hub               residuum-tools
         ┌─────────┴────────┐                 │
residuum-interfaces    residuum-a2a ◄─────────┘
         │                  │
residuum-background ────────┘
        │
residuum-agent    residuum-checkpoints    residuum-protocol    residuum-notify
        │                  │                      │                   │
        └────────┬─────────┴──────────────────────┴───────────────────┘
             residuum-workspace
        │
residuum-bus    residuum-proactivity
        │
residuum-memory     residuum-mcp
       │                  │
       └────────┬─────────┘
       residuum-inference    residuum-skills    residuum-tracing-service    residuum-net
                │                   │                      │                      │
                └────────── residuum-config ───────────────┴──────────────────────┘
                                    │
                              residuum-core
```

The diagram draws the chain of dependencies that sets the layers; each crate's row in the table above lists every crate it depends on, including the ones the diagram does not draw (`residuum-background` also depends on most of the crates below it, `residuum-agent` also depends on `residuum-bus`, `residuum-memory`, `residuum-mcp` and `residuum-skills`, `residuum-interfaces` also depends on `residuum-net`, `residuum-bus` and `residuum-inference`, `residuum-workspace` on `residuum-mcp` and `residuum-inference`, `residuum-protocol` on `residuum-bus` and `residuum-inference`, `residuum-proactivity` on `residuum-bus` alone, `residuum-a2a` also on `residuum-agent`, `residuum-bus`, `residuum-workspace`, `residuum-skills`, `residuum-proactivity`, `residuum-net` and `residuum-inference`, `residuum-hub` also on `residuum-protocol`, `residuum-proactivity`, `residuum-checkpoints`, `residuum-agent`, `residuum-workspace`, `residuum-bus`, `residuum-memory`, `residuum-mcp`, `residuum-inference`, `residuum-tracing-service` and `residuum-net`, `residuum-tools` also on `residuum-background`, `residuum-agent`, `residuum-checkpoints`, `residuum-proactivity`, `residuum-workspace`, `residuum-bus`, `residuum-memory`, `residuum-skills`, `residuum-tracing-service` and `residuum-inference`, and `residuum-bus` on `residuum-inference`).

Only the root has a build script and embeds `web/dist`, so every member crate builds and tests without a web build. The root alone reads the build variables (`RESIDUUM_VERSION` and `RESIDUUM_GIT_COMMIT`, set by `build.rs`), and only into `residuum::BUILD_INFO`; everything else takes a `BuildInfo` (or its `version`) as a parameter, and `testing::source_scan` enforces this. `include_str!` and `CARGO_MANIFEST_DIR` paths in a member reach the repository's `assets/` and `web/` from the crate's directory (`../../../assets/…` from `crates/config/src/`). A file a member embeds from outside its own directory and outside `assets/` is not covered by that: `crates/net/src/browser_landing.rs` embeds `web/src/styles/tokens.css`, and such a path must appear in `RUST_PATHS_RE` in both `.github/workflows/ci.yml` and `.githooks/pre-push`, be classified to its crate in `classify_path` in `scripts/affected-packages.sh`, and have an arm in the pre-push hook's `plan_rust_tests` (for `tokens.css`, `residuum-net`). A file a test reads by path follows the same rule: `web/app-shell-csp.txt` and `.github/workflows/release.yml` belong to `residuum-hub` when it is a member crate with the file that reads each (`crates/hub/src/web/assets.rs`, `crates/hub/src/update.rs`), and to the root otherwise. Without the `RUST_PATHS_RE` entry, CI runs no Rust checks for a change to that file alone, so a breakage can reach `main`.

### Dependency rules

- **Dependencies point down the graph.** No member depends on the root or on a crate above it. Code only the root uses lives in the root; code a lower crate and the root share lives in the lowest crate that uses it. When a lower crate needs something from above, it takes a trait or a parameter: inference's `NoticeSink`, which `residuum-bus` implements for its `Publisher`, carries its failover notices.
- **No cycles, dev-dependencies included.** A crate dev-depends only on crates at or below its normal dependencies. Cargo accepts a dev-dependency cycle without a word and compiles the crate twice, so its types and globals stop unifying.
- **`test-support` is enabled only from `[dev-dependencies]`**, never from `[dependencies]`, a `[target.*.dependencies]` table, or a `[features]` entry such as `default` or a named feature that lists the crate's own `test-support`, `<member>/test-support` or `<member>?/test-support` (a crate's own `test-support` feature may turn on a workspace dependency's, as inference's turns on `residuum-core/test-support`), so no test helper reaches the release binary. A package that depends on a workspace crate with a `test-support` feature turns it on in its `[dev-dependencies]`, so `cargo test -p` and the workspace run build that crate with the same features and share the build.
- **`test-support` code compiles two ways**: with the feature, in another crate's tests, and with `cfg(test)` alone, in the crate's own tests. An external crate such code needs is declared twice: as an optional dependency the feature turns on, and in `[dev-dependencies]` (config's `tempfile` is the pattern). A workspace crate's `test-support` that such code needs is turned on by the feature (`residuum-core/test-support` in inference's, for `testing::model`'s use of core's `gate`); the dev-dependency already has it on, by the rule above.
- **Every external dependency is declared once**, in `[workspace.dependencies]` with its version and features, and packages name it `{ workspace = true }`, adding features where they need them (a crate that names `tokio::net` asks for `features = ["net"]` itself). A crate's `[dependencies]` are the crates its production code names and its `[dev-dependencies]` the crates only its test code names. Workspace members are declared with a path and a version, because `deny.toml` denies wildcard dependencies and a path without a version is one. A target-specific dependency is declared once in `[workspace.dependencies]` and named `{ workspace = true }` in the target table of each crate whose code uses it: `nix` in core's `cfg(unix)` table (for `process`) and in the root's (for `daemon` and the commands); `windows-sys` in the root's (for `daemon`); `objc2`, `objc2-foundation`, `objc2-user-notifications` and `block2` in notify's `cfg(target_os = "macos")` table, and `tauri-winrt-notification` in its `cfg(windows)` table.
- **Every member inherits** `version`, `edition`, `license` and `publish` from `[workspace.package]` and sets `[lints] workspace = true`. Its name starts with `residuum-`, its `<dir>` is lowercase letters and hyphens only, which the package check in `rust-checks.yml` requires, and it sets no `[lib] name`, because the default log filter (`residuum=debug`) enables a crate's logs only through its library name starting with `residuum_` (see `docs/systems-usage/hub.md`, "Logging"). A member names a workspace dependency only as a `residuum-<x> = { workspace = true … }` line, which is the form `scripts/affected-packages.sh` reads.
- **A test binary that can build an HTTP client installs the TLS provider.** reqwest panics with "No provider set" until a crypto provider is installed, so each crate whose code builds HTTP clients includes `crates/core/src/testing/crypto.rs` by `#[path]` under `#[cfg(test)]` and dev-depends on `ctor` and on `rustls`, which a crate whose production code already names `rustls` has in `[dependencies]` and does not repeat. `cargo shear` reads a source file only for the package whose `src/` directory holds it, so it does not see that file's use of the two crates in the including package: the package lists the ones it would otherwise report in `[package.metadata.cargo-shear] ignored`, and `crates/core` lists the file in `ignored-paths`, since no module of core declares it.
- **Dependencies are used or removed.** `cargo shear` fails on an entry in a `[dependencies]` or `[dev-dependencies]` table that no source file of the package names, on a `[workspace.dependencies]` entry no member inherits, and on a source file no module declares. A dependency that is used in a way the scan cannot see (a use in a file the package includes by `#[path]` from another crate's directory, a dev-dependency listed only to turn on a feature) goes in `[package.metadata.cargo-shear] ignored` in the package's manifest, with a comment saying why; `cargo shear` also reports an ignore that suppresses nothing, so the list does not outlive its reason.

### Where the rules are enforced

`src/testing/manifest_scan.rs`, a root test module (`cargo test --quiet -p residuum --lib -- testing::manifest_scan`), reads the root `Cargo.toml` and every `crates/*/Cargo.toml` and fails, naming the file and the key, when a member lacks `[lints] workspace = true`; a member's name does not start with `residuum-` or it sets `[lib] name`; a member does not inherit `version`, `edition`, `license` and `publish`; `test-support` is enabled outside `[dev-dependencies]`, in a dependency table or in a `[features]` entry (any entry that lists the crate's own `test-support`, or an entry other than a crate's own `test-support` feature that lists another member's); the dependency graph, normal and dev dependencies together, has a cycle; a directory under `crates/` is not matched by both `members` and `default-members`; a member's directory is not its package name without the prefix; a package depends on a workspace crate with `test-support` without turning it on in its `[dev-dependencies]`; or the graph `scripts/affected-packages.sh --graph` prints differs from the manifests'. The pre-push hook runs it whenever a manifest changes. The source scans (`testing::source_scan`) cover `src/`, `tests/` and every member's sources, and fail when a crate under `crates/` is not scanned. One of them, `testing::source_scan::unused_pub`, fails on a `pub` item of a member crate (in production or test-support code) whose name appears nowhere in the workspace's code outside its own declaration line, its own crate's `#[cfg(test)]` code and its own crate's `use` lines, since rustc never warns about an unused `pub` item of a library: delete such an item, or move a test helper only its own crate's tests use into `#[cfg(test)]` code. It matches by name, so an unused item whose name something else shares is not reported; its module doc lists its blind spots. The merge queue's `rust-isolation` job runs `scripts/check-crate-isolation.sh`, which checks each member alone (`cargo check -p residuum-<dir>` for its library, for all its targets, and for its library with `test-support`) and the root library and binary without the dev-dependency features, as separate cargo runs, so a crate that compiles only because another package turns on a feature for it fails there.

A new member therefore needs its `crates/<dir>/Cargo.toml` written to these rules, an entry `residuum-<dir> = { path = "crates/<dir>", version = "0.1.0" }` in `[workspace.dependencies]`, the dependency lines in the root and in each member that uses it (with `test-support` in `[dev-dependencies]` when the crate has the feature), its alias in `src/lib.rs`, its line in the crate list of `docs/systems-usage/hub.md` ("Logging"), and, for any file it embeds from outside its own directory and outside `assets/`, the four path entries the `include_str!` paragraph names.

### The root facade

`src/lib.rs` gives each module that lives in a member crate its module name in the root, with one alias each: `pub use residuum_memory as memory;`, `pub use residuum_core::{build_info, diagnostics, process, time, util};`, `pub use residuum_config::agent_keys;`, `pub use residuum_net::{pairing, remote_access};`, `pub(crate) use residuum_net::{tunnel, workbench};`, and so on. `crate::memory::…` in the root and `residuum::memory::…` in `commands/` and `tests/` resolve to the member crate.

- Each name at the root is either a local module or exactly one alias in `src/lib.rs`. An alias names a whole crate, or one whole module of a crate, under its module name; nothing forwards a single item. No local module glob-imports another crate's items into its own, since a local item silently shadows a glob import.
- Root code names a member's items through the aliases: `crate::memory::…`, and `crate::agent_keys::…` rather than `crate::config::agent_keys::…`. The exception is core's `testing`, which has no alias because the root has a `testing` module of its own, so root tests name it `residuum_core::testing::…`.
- Member crates name each other by crate (`residuum_config::Config`), never through a `use` that makes `crate::config` resolve to another crate.
- An alias is `pub` when the binary (`commands/`) or `tests/` names the module through `residuum::…`, and `pub(crate)` otherwise: `tunnel` and `workbench` are `pub(crate)`. An integration test that needs one of their types names the crate directly (`tests/ts_export.rs` imports `residuum_net::workbench::ArtifactSummary`).

### Public API

1. A crate's API is what its `lib.rs` exports. Internal modules are private or `pub(crate)`. A module is public only when it is a namespace callers name, because `module::item` reads better than a root name (`residuum_config::paths`, `residuum_core::util::fs`) or because the language needs a path (a serde `with` module such as `residuum_core::time::minute_format`), and inside it only the items used across the boundary are `pub`.
2. Prefer moving code to the crate that uses it over widening an item for one caller.
3. No blanket `pub`: making a whole module, or every `pub(crate)` in a file, `pub` to get the build through is not allowed.
4. Types with invariants keep their fields private and offer methods. Plain data types (serde payloads, config structs, protocol and status types) have `pub` fields.
5. Public items and modules stay public when their code moves to another crate.
6. An item that moves to another crate or module is not re-exported from its old home. Signatures name it at its new path, and every caller imports it from there.
7. A `#[cfg(test)]` item another crate's tests use becomes `#[cfg(any(test, feature = "test-support"))] pub`. A production item that only another crate's tests call becomes plain `pub`. Production code is never put behind the feature.
8. A newly public item meets the public-item lints: it gets its `# Errors` and `# Panics` sections and `#[must_use]` as `missing_errors_doc`, `missing_panics_doc` and `must_use_candidate` ask.

When the compiler reports an item as private to the crate that needs it, decide it by the first of these that applies:

1. **Move the caller's code in** when the caller uses the item only to reach the crate's internals, that is, the block of caller code that touches it names no root item other than the values it receives and returns. That block becomes a function or method of the lower crate, named for what it does, and the item stays private.
2. **Make a field `pub`** when it belongs to a plain data type.
3. **Add a getter** (`pub fn x(&self) -> &T`) for a field of a type with invariants. A write the caller needs goes through a method that keeps the invariant, never a public field or a plain setter.
4. **Make the item `pub`** otherwise, at the narrowest path that serves it: re-exported from the crate root or from a public namespace module, with the defining module left private.
5. **A test-only item** follows rule 7 above.

A pull request that widens items lists each one in its description: its path, its kind, which step applied, and its callers outside the crate.

### Build caching

Local builds keep Cargo's default incremental dev profile, and CI's lab-runner jobs set `CARGO_INCREMENTAL=0`. This is what sccache does with workspace crates, measured with sccache 0.18.0 on a two-crate workspace built in two directories:

- **It does not cache an incrementally compiled crate.** Under the default dev profile Cargo compiles workspace crates with `-C incremental`, and sccache reports them `CannotCache(incremental)`, so locally sccache serves only registry dependencies. It never caches test harnesses or binaries (`CannotCache(crate-type)`).
- **With `CARGO_INCREMENTAL=0`, it caches a workspace crate's library, but only at the same absolute path.** The same workspace built in a second directory missed on both workspace crates and on the registry crates with build-script output (`serde`, `serde_core`), and hit on the registry crates without one (`proc_macro2`, `quote`, `syn`, `unicode_ident`).
- **`SCCACHE_BASEDIRS` does not change that for Rust.** With both checkouts listed in it, set in the server's environment when it started and shown by `sccache --show-stats` under "Base directories", the hits and misses were the same. sccache hashes the values of the `CARGO_*` environment variables rustc inherits: with the same sources at the same path, changing only `CARGO_TARGET_DIR` misses every crate, registry ones included. `CARGO_MANIFEST_DIR` and `OUT_DIR` are absolute paths in that set, which a base directory does not rewrite; that they cause the misses is inferred, not proved.

What follows from it:

- **CI sets `CARGO_INCREMENTAL=0`** in `rust-checks.yml`, the `rust-stress` job of `nightly-stress.yml` and the lab-runner jobs of `cross-compile.yml`. CI checkouts are fresh, so incremental state would never be reused there, and with it off sccache serves every unchanged member's library from the runners' shared store. The lab runners' `github-runner` image sets `RUSTC_WRAPPER=sccache` and an S3-compatible shared store; that configuration lives with the runner pools in grizzly-platform, not in this repository. Checkouts are at the fixed path `/home/runner/_work/residuum/residuum`, and no job may set `CARGO_TARGET_DIR`: sccache hashes its value, so a per-job target directory would miss every crate. The build summaries of the `rust-lint` and `rust-test` jobs in `rust-checks.yml` print `sccache --show-stats`, which shows the hit rate per run; the `rust-test` summary also has the CPU table from `scripts/ci-cpu-report.sh`. The job summary is not readable through `gh` or the API, so each of these tables is also written to the job log, between `===== BEGIN CI METRICS (<job>) =====` and `===== END CI METRICS (<job>) =====` lines. To read them for a run, run `gh run view <run-id> --log | sed -n '/BEGIN CI METRICS/,/END CI METRICS/p'`, or fetch one job's log with `gh api repos/Grizzly-Endeavors/residuum/actions/jobs/<job-id>/logs`. The CPU tables of the web end-to-end and web stress shards are logged the same way.
- **Local builds keep incremental compilation, with no per-package override.** Every agent worktree has its own path, so sccache could not share member builds between worktrees even with incremental off, and a cached library would carry the first worktree's absolute paths in its debug info and panic locations. Within one checkout, Cargo's fingerprints keep unchanged members from rebuilding, and incremental compilation speeds up the crate being edited. Reuse comes from a checkout at a stable path keeping its own target directory, which is why the pre-push hook checks commits it cannot check in place in one stable tree, `target/pre-push/tree`, building into `target/pre-push`, rather than in a fresh worktree each time. The hooks do not change `CARGO_INCREMENTAL`.

To check again, for example after an sccache upgrade (it uses a private server and leaves the shared one alone):

```bash
(
R="$(mktemp -d)"
# Workspace p1: root package topc depending on crates/lowc by path, lowc depending on serde with derive.
cargo new --quiet --lib "$R/p1" --name topc
cargo new --quiet --lib "$R/p1/crates/lowc"
printf '\n[workspace]\nmembers = ["crates/lowc"]\n' >>"$R/p1/Cargo.toml"
(cd "$R/p1" && cargo add --quiet lowc --path crates/lowc && cd crates/lowc && cargo add --quiet serde --features derive)
(cd "$R/p1" && cargo generate-lockfile --quiet)
cp -a "$R/p1" "$R/p2"

export SCCACHE_SERVER_PORT=4299 SCCACHE_DIR="$R/cache" SCCACHE_ERROR_LOG="$R/server.log" SCCACHE_LOG=debug
export SCCACHE_BASEDIRS="$R/p1:$R/p2" CARGO_INCREMENTAL=0 RUSTC_WRAPPER=sccache
unset CARGO_TARGET_DIR
sccache --start-server
sccache --show-stats | grep -A2 'Base directories'    # must list p1 and p2
(cd "$R/p1" && cargo build --quiet)
sccache --zero-stats
(cd "$R/p2" && cargo build --quiet)
sccache --show-stats                                   # hits and misses for p2
grep 'compile result' "$R/server.log" | grep -E '\[(lowc|topc|serde|serde_core)\]'
sccache --stop-server
)
```

The subshell keeps the variables out of your shell.

`SCCACHE_BASEDIRS` takes absolute paths separated by `:` on Unix, and a running server reads it only when it starts. The check passes when the p2 build shows cache hits for `lowc` and `topc`; with sccache 0.18.0 both miss. The same reading works on a real build: `[residuum_memory]: compile result: cache hit` in the server log. While the check fails, the choices above stand.

## Releases

Releases use [CalVer](https://calver.org/) with the format `YYYY.0M.0D` (e.g., `v2026.03.02`). If multiple releases happen on the same day, a suffix is added: `v2026.03.02-2`.

Releases are automated — pushing a tag matching this format triggers the CI pipeline, which builds cross-platform binaries and creates a GitHub release. Cargo.toml version is not tied to release tags.

## Architecture

See [docs/systems-usage/](docs/systems-usage/) for how each subsystem works — it is the authoritative reference, kept current with the code. [docs/design-philosophy.md](docs/design-philosophy.md) covers the principles behind those choices, and [docs/](docs/) describes the rest of the documentation layout. How the code is split into crates is in [Workspace layout](#workspace-layout).

## License

By contributing, you agree that your contributions will be licensed under the MIT License.
