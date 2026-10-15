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
5. Install the git hooks — they format and check every commit, and run clippy, the tests for the code you changed, and the web checks on every push:
   ```bash
   .githooks/install.sh
   ```

If you have [`just`](https://github.com/casey/just) installed, `just setup` installs the web dependencies (step 3) and the git hooks (step 5) in one go, and `just` on its own lists everything else: `just check` runs everything CI runs except the web end-to-end suite and lists every failing check in one run, `just test memory::` runs one module's tests, and `just web-mock` starts the web UI against a mock API with no backend. Every recipe wraps a command documented here, so `just` is optional.

## Branch & PR Workflow

All changes go through pull requests, and `main` changes only through the merge queue.

1. Create a feature branch from `main` with a descriptive name
2. Make your changes and commit frequently
3. Push your branch and open a PR against `main`, with `closes #N` in the PR body for each issue it closes
4. The git hooks are the first quality gate: formatting and the dependency audit run on every commit; clippy, the tests for the code you changed (and the source scans), the generated-types check, and the web checks run on every push, against your branch as pushed. A conflict with `main` shows on the pull request, and the merge queue checks the merged result. CI then runs the whole test suite, the Rust checks on PRs that change Rust code and the web checks on PRs that change `web/`, and its `CI gate` check sums them up (see [Quality Gates](#quality-gates)). CI runs on self-hosted runners, so a PR from a fork runs no checks of its own: they run on the merge queue batch once a maintainer enqueues it, and a maintainer can run the checks locally before enqueueing a contributor's PR.
5. Clippy for Windows runs on every Rust PR; the other cross-platform checks (clippy for aarch64 Linux and macOS, plus tests on Windows) run in the merge queue when a batch changes dependencies or the toolchain, and on a PR when asked for; see below
6. The PR merges through the merge queue (see [Merging](#merging))

### Merging

`main` has a ruleset, kept in [`.github/rulesets/main.json`](.github/rulesets/main.json): it refuses a push to `main`, a force push or its deletion, requires a pull request (no approvals needed), and merges a pull request only through the merge queue, as a merge commit, once its `CI gate` check has passed. The queue merges a PR onto the tip of `main` together with any PRs ahead of it (up to five at once), runs CI on that batch, and merges the batch only when every PR in it passes.

- Never push to `main`, and never merge locally and push the result: the push is refused.
- Enqueue a PR with `gh pr merge <n> --auto`, or the "Merge when ready" button. While the PR's checks run, this turns on auto-merge; when `CI gate` passes, GitHub adds the PR to the queue. The merge happens later, not when the command returns.
- Put `closes #N` in the PR body (`gh pr create --body`, or `gh pr edit <n> --body`). The queue writes the merge commit's message, so there is no merge commit message of your own to put it in, and a keyword in a PR comment closes nothing.
- See where a PR stands with `gh pr view <n> --json state,mergeStateStatus,autoMergeRequest,statusCheckRollup` (`autoMergeRequest` is set while it waits to enter the queue, and `state` is `MERGED` once it has merged), its place in the queue with `gh api graphql -f query='query($n:Int!){repository(owner:"Grizzly-Endeavors",name:"residuum"){pullRequest(number:$n){mergeQueueEntry{position state}}}}' -F n=<n>`, and the whole queue at https://github.com/Grizzly-Endeavors/residuum/queue/main.
- A PR ejected from the queue (its batch failed CI, it conflicts with a PR merged ahead of it, or its checks timed out after four hours) is removed, with the reason on its timeline. Fix the cause on the branch, push, and enqueue it again. `just ci-failures <n>` lists the failing tests, including those of the merge queue batch that failed when the PR's own runs are green.
- A fork's PR shows a passing `CI gate` without having run anything. Enqueueing it runs its code, including any change it makes under `.github/`, on the self-hosted runners, so read the whole diff before enqueueing a fork's PR.
- Repository admins can bypass the ruleset in an emergency, such as CI itself being broken, and only through a PR: `gh pr merge <n> --merge --admin` merges past the queue and its checks. Nobody can push to `main` directly.

To change the ruleset, edit `.github/rulesets/main.json` in a PR, and once it merges, apply it with `gh api --method PUT repos/Grizzly-Endeavors/residuum/rulesets/<id> --input .github/rulesets/main.json`, where `gh api repos/Grizzly-Endeavors/residuum/rulesets --jq '.[] | select(.name == "main") | .id'` prints the id. A repository without the ruleset gets it with `gh api --method POST repos/Grizzly-Endeavors/residuum/rulesets --input .github/rulesets/main.json`, and the repository needs auto-merge allowed and head branches deleted on merge for the flow above: `gh api --method PATCH repos/Grizzly-Endeavors/residuum -F allow_auto_merge=true -F delete_branch_on_merge=true`. The file is the ruleset as it should be: a change made in the repository settings is lost at the next apply, so make it in the file.

### Rust Toolchain

The Rust version is pinned in `rust-toolchain.toml`, and rustup installs exactly that version for every build: your machine, CI, and releases. Everyone lints with the same clippy, so code that passes the git hooks passes CI. Bumping the version is its own PR: change the pin, fix whatever new lints the release brings, and commit.

### Cross-Platform Checks

The git hooks only build for your own platform. Every PR that runs the Rust checks also runs `cargo clippy --all-targets --all-features --target x86_64-pc-windows-gnu -- -D warnings` (the `Clippy (x86_64-pc-windows-gnu)` job in `.github/workflows/rust-checks.yml`), so code behind `cfg(windows)`, and test code that only some platforms use, is held to the same lints as your own build before it reaches `main`.

`.github/workflows/cross-compile.yml` adds clippy for aarch64 Linux and macOS, and runs the full test suite on a Windows runner, where `cfg(windows)` code actually executes. The merge queue runs it for every batch that changes `Cargo.toml`, `Cargo.lock`, `build.rs`, `rust-toolchain.toml`, or `.github/workflows/cross-compile.yml`, as part of `CI gate`, so a batch that fails it does not merge. On a pull request it reports without gating the merge, and runs when:

- the PR carries the `cross-compile` label (adding the label starts a run, and every later push re-runs it)
- the PR changes `Cargo.toml`, `Cargo.lock`, `build.rs`, or `rust-toolchain.toml` (automatic)
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
- `cargo deny check` — dependency audit, when a manifest (`*.toml`, `Cargo.lock`) is staged
- `scripts/ci-failures-test.sh` and `scripts/stress-issues-test.sh` — the `just ci-failures` log parser and merge queue batch selection against their fixtures, and a dry run of the nightly stress issue filer over them, when a `scripts/ci-failures*` or `scripts/stress-issues*` file is staged
- No `dbg!()` or `#[allow(...)]` in the staged diff, and no growth of the wall-clock backlog

On every push (pre-push), against the pushed commits as they are, without merging `main` into them (a conflict with `main` shows on the pull request, and the merge queue checks the merged result). A commit is checked in place when it is the checkout's HEAD with a clean tree, and otherwise in a temporary worktree of that commit:
- When the pushed commits change what the Rust checks run on (the paths for which the `changes` job of `.github/workflows/ci.yml` runs the Rust checks, such as `src/`, `tests/`, `assets/`, the Cargo and lint configuration, and the generated web types in `web/src/lib/generated/`): `cargo clippy` (pedantic linting with strict denials), the tests for the code the push touches, and `scripts/check-generated-types.sh`, which checks that the web app's TypeScript protocol types match the Rust types. The hook picks the tests from the changed paths and prints what it chose:
  - A change under `src/<module>/` or in `src/<module>.rs` runs that module's unit tests (`cargo test --lib` filtered to `<module>::`, plus the bin's tests when `src/main.rs` declares the module).
  - A change to `src/lib.rs` or `src/main.rs` runs every unit test.
  - A change under `tests/it/` runs the `it` binary, which holds every integration test except `ts_export`, and a change to `tests/ts_export.rs` runs that test. A change under `tests/fixtures/` or `assets/` runs every unit test and every integration test. Integration tests other than `ts_export` are built only when a `tests/` path changed; the generated-types check builds `ts_export` on every Rust push.
  - The source scans (`testing::source_scan`, see [Waiting in tests](#waiting-in-tests)) run on every push that changes Rust.
  - A change to a manifest, `build.rs`, the toolchain or lint configuration, the generated web types, or the scripts and workflows the checks use adds no tests of its own. A change to any other file the Rust tests read by path (`web/src/styles/tokens.css`, `web/app-shell-csp.txt`, `.github/workflows/release.yml`) runs every unit test.
- When they change `web/` (Markdown under `web/` does not count, since no build or test reads it): ESLint and the style lint (Stylelint), `svelte-check` (warnings fail it), and the web unit and component tests (Vitest) that import a changed file. A change to `web/package.json`, the lockfile, the Vite, Vitest, Svelte or TypeScript configuration, the component test setup file, the hang guard (`web/src/test/guard.ts`) or the service worker plugin (`web/build/service-worker.ts`) runs the whole web suite.

The hook runs the tests of the code you changed, not the whole suite, and CI runs the whole suite. When a change reaches code that other modules depend on (a shared helper, a trait other modules implement), run those modules' tests with `just test <filter>`, or the whole suite with `cargo test --quiet`, before you push.

Every push check runs even after one fails, and the hook ends with a summary of what failed. Do not bypass the hooks: `--no-verify` is not allowed on `git commit` or `git push`. If a hook fails, fix the issue and try again.

CI repeats these checks and adds more. `.github/workflows/ci.yml` runs on every pull request and every merge queue batch. Its `changes` job reads the diff and starts the Rust checks (`rust-checks.yml`) when Rust code or what the build embeds changed (`src/`, `tests/`, `assets/`, the Cargo and lint configuration, the generated web types), the web checks (`web-checks.yml`) when `web/` changed (Markdown under `web/` excepted, since no build or test reads it), and actionlint (`actionlint.yml`) when a workflow or `.github/actionlint.yaml` changed. Its `CI gate` job is the one required status check: it fails when a job it needs failed or was cancelled, and passes when each one succeeded or was skipped, so a PR that touches neither Rust nor web code still merges.

| Stage | Runs on | What it runs |
|-------|---------|--------------|
| pre-commit | every commit | Formatting of what is staged, `cargo deny` when a manifest is staged, the `dbg!()` / `#[allow]` / wall-clock-backlog blocks, the CI log parser and stress issue filer fixtures when those are staged |
| pre-push | every push, as pushed (not merged with `main`) | Clippy, the tests of the code the push touches and the generated-types check when Rust paths change; ESLint, Stylelint, `svelte-check` and Vitest (the tests that import a changed file) when `web/` changes |
| PR lane (`ci.yml`, `pull_request`) | every push to a pull request | Rust (when Rust paths change): fmt, host clippy, Windows clippy, `cargo deny`, `cargo test`, generated types. Web (when web paths change): format, lint, types, unit tests on Node 22 and 24, build, the four end-to-end shards, and the visual comparisons, which never fail and report in one PR comment. actionlint when a workflow changes. `CI gate` sums them up |
| Merge queue lane (`ci.yml`, `merge_group`) | every batch the merge queue builds, merged onto `main` | The PR lane's checks without the visual comparisons, plus the Pebble tests, plus `cross-compile.yml` (aarch64 Linux and macOS clippy, the tests on Windows) when the batch changes `Cargo.toml`, `Cargo.lock`, `build.rs`, `rust-toolchain.toml` or `.github/workflows/cross-compile.yml`. `CI gate` must pass for the batch to merge |
| Nightly (`nightly-stress.yml`) | 07:23 UTC on `main`, or by hand | The Rust suite unloaded, then as two concurrent copies under CPU load for three rounds; every end-to-end spec three times with the CPU throttled. Each failing test becomes an `investigate` issue, and a run that timed out gets one saying it did not finish. It gates nothing |
| Release (`release.yml`) | a `v*` tag | The merge queue lane's Rust and web checks (no visual comparisons, no stress) beside the release builds; nothing is published unless both pass |

The Rust checks run `cargo fmt --check`, clippy with `-D warnings` for your host and for the Windows target (`Clippy (x86_64-pc-windows-gnu)`; see Cross-Platform Checks), `cargo deny check`, the whole test suite, and the generated-types check, which regenerates the web's TypeScript protocol types from the Rust types and fails when `web/src/lib/generated/` differs from what is committed, so after changing an exported Rust type run `just types` and commit the result (`just types-check` runs the same check as CI). The merge queue lane and releases add the Pebble tests (`just pebble`), which issue certificates from the Let's Encrypt test CA in Docker. The web checks run the format, lint, type and unit checks, with web test coverage in the job summary and no threshold, the unit and component tests again on Node 22 and Node 24 (`Web Tests (Node 22)` and `Web Tests (Node 24)`), the build, and the end-to-end and accessibility suite against the mock in four shards, in the Playwright container's browser, so the runners need no Chromium of their own (`just web-e2e` runs it locally together with the visual comparisons; `just web-e2e-fast` leaves out the visual comparisons and runs in your own Chromium, or in the Playwright container when Chromium can't be installed; see [web/CONTRIBUTING.md](web/CONTRIBUTING.md#testing)). On pull requests the visual comparisons, a design reference of component screenshots, run in a job of their own that never fails the PR, and report their changed shots in one pull request comment, updated in place, with a link to the diffs. actionlint runs with shellcheck over the `run:` blocks, at the versions pinned in `.github/workflows/actionlint.yml`; `just actionlint` runs it locally and needs `actionlint` and `shellcheck` on `PATH`. A failing check does not stop the others: every test binary runs, and the format, lint, type and test steps each run, so one CI run lists every failure.

When a CI run fails, `just ci-failures` lists every failing test of the latest runs for your branch, grouped by job and step, so one pass can fix them all. It takes a PR number, a branch, a release tag or a commit instead when you want another one. For a PR number whose own runs are green, the latest merge queue batch of that PR is listed when it failed, which is how an ejected PR's failing tests show up. Each failure carries two flags: `touched` when this change edits a file the failure names (the failing test's file or, for a Rust failure, the file its panic was raised in), and `seen elsewhere` when the same failure was in a failed run on another branch or tag in the last 14 days. A failure that is not `touched` but is `seen elsewhere` is marked `likely flake`: it may be a flake, or a breakage other branches share, so check `main` before treating it as this change's. Every other failure is presumed to come from this change. Failed jobs' logs are cached under `target/ci-failures-cache/`. The log parser and the merge queue batch selection have fixtures: `just ci-failures-test` checks both.

### Style

- **Error messages**: lowercase, no trailing period, include context (`"failed to parse config at {path}"` not `"parse error"`)
- **No silent failures**: every error must be visible to the user — not buried in debug logs
- **Comments**: explain *why*, not *what*
- **Visibility**: private-first — only add `pub(crate)` or `pub` when needed
- **Async-first**: sync only for trivial or CPU-bound work

### Testing

Tests are required for new functionality. Unit tests go in `#[cfg(test)] mod tests` at the bottom of the file. Integration tests are modules of the one test binary in `tests/it/`: add a file there and a `mod` line to `tests/it/main.rs`, and `cargo test --test it gateway::` runs one module. They share a process, so a test doesn't change environment variables, the current directory or the global tracing subscriber. Every file directly in `tests/` links the whole crate as a binary of its own; `tests/ts_export.rs`, which writes the web app's generated types and which the type scripts run by name, is the only one.

A test waits on the event it checks, never on elapsed time: a duration that holds on an idle machine fails when several builds share it. A flaky test is fixed by making it wait on an event the code exposes, adding that signal to the code when there is none, never by widening a timeout, adding a sleep, or rerunning past it. The suites under load run nightly on `main`, in `.github/workflows/nightly-stress.yml`, not on pull requests or in the merge queue. The Rust suite runs once unloaded, then as two concurrent copies with CPU burners alongside, three rounds over (`scripts/stress-tests.sh`); every end-to-end spec runs three times with the browser's CPU throttled, split over four runners (`scripts/web-stress.sh` with `WEB_STRESS_ALL=1`). `scripts/stress-issues.sh` files each test that failed as an `investigate` issue titled `Nightly stress: <test>`, with the run link, the commit and the failure's messages, and adds a comment to the open issue instead when one has that title, so a test that keeps failing collects its runs in one place. A run with more than ten new failing tests files one issue for the run instead, listing them all, so a broken `main` or runner does not flood the tracker. A Rust failure says whether it failed under load only, so a test waiting on time (fix the wait; a rerun that happens to pass hides it), or also in the unloaded run, an ordinary failure to fix as any failing test. A job that fails without naming a test (a broken build, a runner that went away) is filed as one issue for its suite, and a job cancelled by its timeout is filed as one saying the run did not finish. Reproduce a failure with `just stress <filter>`, which runs the tests as two concurrent copies with CPU burners alongside (`STRESS_COPIES`, `STRESS_ROUNDS` and `STRESS_BURN` tune it), or `WEB_STRESS_ALL=1 just web-stress <spec>` for a spec. A test you add or fix must pass `just stress <filter>` before you push; the nightly run is the backstop, not the first check. `gh workflow run nightly-stress.yml` runs the nightly suites on `main` by hand. `just stress-issues-test` checks the issue filer against the log parser's fixtures, without GitHub.

#### Waiting in tests

Test code waits through `crate::testing`, never with `tokio::time::sleep`, `tokio::time::timeout`, `std::thread::sleep` or wiremock's `set_delay`. A source-scan test (`testing::source_scan`) fails on any of those outside `src/testing/`; the pre-push hook runs it on every push that changes Rust. The waits that predate the rule are counted per file in `src/testing/wall_clock_backlog.rs`, and the counts only go down: converting a file's waits means lowering its entry, and the hook refuses a commit that adds one.

- **Something will arrive on a channel or watch.** `testing::wait::next`, `next_matching` (skips to the first match), `next_where` (everything received up to the match) or `watch_until`; a bus `Subscriber` is waited on with `bus_wait::next_event`. For state with no channel to wait on, `wait::until` polls a check. Each runs under `wait::HANG_GUARD`, the suite's only deadline, which exists to stop a hung test and names what it was waiting for; never shorten it or add another.
- **A state must last while the test looks at it** (an agent mid-turn, a session queued behind another). Hold the call that would end it at a closed `testing::gate::Gate`: `gate.until_held(1)` proves the call is waiting, the test looks, then `gate.release(1)`. The model has gated versions: `testing::model::GatedModel` (an HTTP model, either in front of a wiremock server or with a fixed reply; the hub test fixture puts one in front of every agent's mock), `GatedProvider` in process, and the scripted stream server's `Step::Hold`.
- **Background work must have happened.** Wait on the signal the code gives for it. When there is none, add one to the code: a test that needs to know something finished is usually a sign users and diagnostics need to know too.
- **Something must not happen.** Only after a positive sync point that proves it would have happened by now: a bus barrier (`bus_wait::bus_barrier`, which publishes a marker and waits for it; the broker hands a message to every subscriber at once, so once a subscriber has a later message, every subscriber has the earlier one), a sentinel file written after the cause (file notifications arrive in order), a stop (an agent's watcher drains before `stop` returns), or a pending count back at zero. Then check with `wait::drain` or the current state.
- **The code under test is timer-driven and has no real I/O** (no sockets, child processes or file watchers). Run it on a paused clock (`#[tokio::test(start_paused = true)]`) and move time with `testing::clock::elapse`; `clock::within` checks that something does not complete within a window. Both refuse to run on a real clock.
- **A timeout on a child process must fire.** Start on a real clock, wait for the child to show it has started (a marker file it writes), then `tokio::time::pause()` and jump past the timeout with `tokio::time::advance`. Don't leave a paused clock to advance on its own while a child runs: on Windows tokio reads a child's pipes on blocking threads, and a running blocking task holds a paused clock still, so the test waits for the child to exit instead.

A test that needs a fake tool writes it with `crate::testing::script::write_executable`, which writes the script through a child shell so the test process never holds a write descriptor a forked child could inherit; writing a script in the test and executing it fails with "Text file busy" when another test thread forks at the wrong moment, and far more often under load. The same source scan fails on test code that makes a file executable itself (a `"chmod"` command, or an execute-bit mode literal passed to `from_mode`, `set_mode` or `.mode(`), and a directory restored to a mode with its execute bit is listed with its reason in `EXEC_MODE_ALLOWED` in `src/testing/source_scan.rs`.

`clippy.toml` exempts `unwrap_used`, `expect_used`, `panic`, and `dbg_macro` inside `#[cfg(test)]` modules and `#[test]` functions, so unit tests use unwrap, expect, panic, and `dbg!` with no suppression. Do not add `#[expect(clippy::unwrap_used)]` (or `expect_used`, `panic`, `dbg_macro`) on that code: the lint is already exempt, the expectation never fires, and `-D warnings` fails the build.

`clippy::tests_outside_test_module` is denied and is not controllable from `clippy.toml`. Integration tests under `tests/` need `#![expect(clippy::tests_outside_test_module, reason = "...")]` at the top of the module's file. A helper at the top level of a `tests/it/*.rs` module — not inside `#[cfg(test)]`, and not itself a `#[test]` — still needs its own `#[expect]` when it uses unwrap, expect, panic, or `dbg!`.

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
