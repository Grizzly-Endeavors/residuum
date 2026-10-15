# Workspace split, round 1: the lower layers as their own crates

**Status:** designed, not started. The starting point is `main` once PR #691 (`perf/build-speed`) has merged; §1 says what that tree looks like. Implementation follows the chunk plan in §14; its first two chunks (A0, M0) record the baseline in §15. When the last chunk lands, the resulting layout is described in a "Workspace layout" section of `CONTRIBUTING.md` (written by chunk F2; it is a developer concern, not product behavior, so it does not go in `docs/systems-usage/`) and this document moves to `docs/archive/`.

## 1. Summary

Residuum is one crate of about 260k lines (145k production, 115k tests). Every edit recompiles all of it, and every test run builds one unit-test harness of all of it. Round 1 turns the repository into a Cargo workspace and extracts the lower layers, at the crate boundaries they keep, into eight member crates:

| crate | directory | holds |
|---|---|---|
| `residuum-core` | `crates/core` | `util` (minus what only the top uses), `time`, the diagnostic types, the shared test helpers (`test-support` feature) |
| `residuum-config` | `crates/config` | `config` (minus the setup wizard), `agent_keys`, the workspace path layout, and the plain data types `config` needs |
| `residuum-skills` | `crates/skills` | `skills` |
| `residuum-inference` | `crates/inference` | `inference`, plus the tool-call result types and the scripted model server (`test-support`) |
| `residuum-tracing-service` | `crates/tracing-service` | `tracing_service` minus the build-info gathering |
| `residuum-memory` | `crates/memory` | `memory` |
| `residuum-mcp` | `crates/mcp` | `mcp` |
| `residuum-net` | `crates/net` | `tunnel`, `remote_access`, `pairing`, `workbench`, and the request helpers they share with the gateway (`browser_landing`, `cross_site`, `remote_control_guard`) |

Everything else stays in the root package `residuum` (lib + the `residuum` binary): gateway, hub, interfaces, bus, agent, tools, background, a2a, workspace, notify, inbox, checkpoints, pulse, subconscious, actions, daemon, update, features, commands. That is about 184k lines, 70% of today's crate. The root keeps `build.rs`, `web/dist` embedding, `tests/`, and the version constant.

What it buys: an edit in the root no longer recompiles the lower crates or rebuilds their test harnesses; work on a lower crate builds and tests only its own dependency chain (`cargo test -p residuum-memory` never builds the gateway or needs `web/dist`); cold builds compile the lower crates in parallel with each other; the pre-push hook lints only the packages a push touches and the packages above them, and runs the tests of the member crates a push touches and of the members above them, plus the root's tests for the root modules it touches; CI pull requests lint, build and test only the packages a change can affect (format, `cargo deny` and the generated-types check stay whole-workspace, and the merge queue runs everything). §15 says how this is measured and what the targets are.

**The starting tree.** PR #691 (`perf/build-speed`, head 63b1cebe when this was written) already did three things this design builds on:

- The integration tests are one binary, `tests/it/` (`tests/it/main.rs` declares one module per former test file), plus `tests/ts_export.rs`, the only other file directly in `tests/`. So the root has four test targets: the lib's unit tests, the bin's unit tests, `it`, and `ts_export`.
- `[profile.dev] debug = "line-tables-only"` and `[profile.dev.package."*"] debug = false`.
- reqwest is built with `rustls-no-provider`, so aws-lc is gone and ring is the only crypto backend. Production installs ring in `commands::run`; each test binary that can build an HTTP client installs it before `main` through a `ctor` in `src/testing/crypto.rs`, which `src/testing/mod.rs`, `src/main.rs` (`#[path]`) and `tests/it/support/mod.rs` (`#[path]`) include. §7 says how the member crates' test binaries get it.

It also includes current `main`: the pre-push hook that checks each pushed commit as pushed and selects tests by changed module (`.githooks/pre-push`, PR #682), and the merge-queue CI (`ci.yml` with its `changes` job and `CI gate`, the reusable `rust-checks.yml`, `nightly-stress.yml`), described in `CLAUDE.md` ("Git Hooks", "What Runs Where", "Merging").

Round 2 (moving the bus's embedded types down, extracting `interfaces`, splitting hub and gateway) is out of scope. §16 describes what round 1 leaves in place, so round 2 can be planned from measurements of round 1.

Evidence: the dependency data comes from a scan of `main` at c83ddf78, which counted every production `crate::<module>` reference with file and line, and from the checks recorded in this document (tracing-subscriber's filter matching, sccache's and Cargo's behavior with workspace crates, Cargo's handling of `default-members`). Line numbers below are from c83ddf78. On the starting tree, `src/` differs from c83ddf78 only in `src/testing/{crypto,mod,source_scan,wall_clock_backlog}.rs`, `src/main.rs`, `src/commands/mod.rs`, `src/a2a/client/e2e_tests.rs` and `src/remote_access/acme.rs`, and no line reference below points into those files, so the numbers hold; chunks still re-locate code by the quoted names, since `main` moves on.

## 2. Goals and constraints

- **One real split.** Each lower crate is extracted once, at its final boundary. There is no interim "foundation" crate to split again.
- **Deliberate public APIs.** Each crate's API is chosen per item (§6). Nothing is widened to `pub` wholesale to make the build pass, and moved items are not forwarded from their old paths.
- **The lint regime does not change in substance.** The lint table moves from `[lints]` to `[workspace.lints]` verbatim, and every member opts in. `clippy.toml`, `rustfmt.toml`, `rust-toolchain.toml` and `deny.toml` keep applying unchanged.
- **Behavior does not change**, except where §8 says so (how `residuum logs` displays crate targets, which extra values `--module` accepts, and what a `RUST_LOG` directive must name).
- **The release binary, its name, and the release artifacts are unchanged.**
- **Every chunk leaves `main` fully working.** There is no half-split state that needs the next chunk to build.

## 3. Crate graph

```
L3   residuum-memory     residuum-mcp
            │                  │
            └────────┬─────────┘
L2          residuum-inference    residuum-skills    residuum-tracing-service    residuum-net
                     │                   │                      │                      │
L1                   └────────── residuum-config ───────────────┴──────────────────────┘
                                         │
L0                                 residuum-core

residuum (root package: lib + bin) depends on every crate above.
Every crate depends on core directly, and every crate but core on config;
memory and mcp also depend on inference.
```

| crate | depends on (workspace) | ≈ lines total | ≈ production | 30-day commits touching it |
|---|---|---|---|---|
| `residuum-core` | — | 3.1k (+0.8k `test-support`) | 1.7k | 32 |
| `residuum-config` | core | 13.3k | 8.8k | 69 |
| `residuum-skills` | core, config | 1.75k | 0.95k | 12 |
| `residuum-inference` | core, config | 21.1k | 10.5k | 56 |
| `residuum-tracing-service` | core, config | 1.5k | 1.1k | 8 |
| `residuum-memory` | core, config, inference | 12.1k | 7.1k | 41 |
| `residuum-mcp` | core, config, inference | 2.0k | 1.3k | 12 |
| `residuum-net` | core, config | 21.3k | 12.4k | 55 |
| `residuum` (root) | all of the above | 184k | 101k | 567 |

The commit column counts, of the 630 non-merge commits that touched `src/` in the 30 days before c83ddf78, those that touch a module the crate will hold. 399 of the 630 (63%) touch only modules that stay in the root, so they would rebuild no lower crate.

Line counts are the scan's totals adjusted for the moves in §5. Tests live inside the crate they test; the lower crates' share of the 115k test lines moves with them.

### Why each boundary is where it is

- **core = util + time + diagnostic types.** These have no production dependency on any other module once the moves listed for core in §5 are made. Today's `util` holds pieces only the root uses: `tracing_init` (it reads `config::LogLevel` and `HubPaths`), `log_format` (only `commands/logs.rs` reads it), and `process` (only `interfaces`, `pulse` and `tools` use it). Those move to the root instead of making core depend on config. The diagnostics dispatcher (`src/diagnostics/mod.rs:147-296`) calls into config, workspace, a2a, pulse and skills, and its `DiagnosticsPaths` is used only by tools, agent and gateway, so the dispatcher stays in the root and only `Diagnostic`, `Location` and `Severity` go to core.
- **config holds agent_keys.** `agent_keys` depends on config only through `config::secrets::{decrypt, encrypt, load_key, load_or_create_key}` (`src/agent_keys/store.rs:23`), which nothing else outside config uses. In one crate those four stay `pub(crate)`; as two crates they would become public API of the secret store for one caller.
- **config is below inference, not above it.** config uses inference for three plain data types and one builder (8 references: `ThinkingConfig`, `ThinkingLevel`, `RetryConfig`, and `completion_options_for_role` building `CompletionOptions`). inference uses config for `ProviderKind`, `ProviderSpec`, `ImageInput` and `SystemOneConfig`, which are deserialized config. Moving the three data types down and the builder up is smaller and keeps config types in config.
- **skills is above config, not below it.** config's only use of skills is the `SkillDir` value it resolves (`src/config/types.rs:12`, `src/config/resolve/agent.rs:6`). With `SkillDir` and `SkillSource` moved into config, skills depends on core and config and nothing in the lower crates depends on skills. With skills below config instead, each of skills' 12 commits a month would rebuild config and everything above it; this way each of config's 69 rebuilds skills, which is 1.75k lines.
- **inference stands alone at L2.** Its one upward reference is the bus `Publisher` used for failover notices (`src/inference/failover.rs:14`, `factory.rs:227,261`), replaced by a trait (§5). The tool-call result types `ToolError` and `ToolResult` move into it next to `ToolDefinition`, which is what lets mcp leave the root.
- **memory and mcp sit on inference.** memory has no bus dependency at all; its only upward edge is `WorkspaceLayout`, which moves to config. mcp's only upward edge is `ToolError`/`ToolResult`/`SharedToolsPath` from tools.
- **tracing_service stands alone at L2.** Once `ThinkingConfig` is in config, its only dependencies are core and config. The one part that reads the build version (`client_context.rs`, all of whose callers are in the root) moves to the root (§9).
- **net is one crate.** tunnel and remote_access reference each other (3 and 3), pairing and workbench reference each other (2 and 10), and remote_access uses pairing; splitting them would mean traits across every one of those pairs. Their remaining upward references (12) are moves of code that only net and the gateway share. Churn is low (55 commits in 30 days touched any of its modules, against 567 for the root), so net is up to date most of the time.
- **skills, mcp and tracing-service are crates of their own** although each is 1.5k to 2k lines: their edges are clean, "final boundaries" means round 2 does not extract them again, and each isolates heavy dependencies (rmcp; opentelemetry and prost).
- **Everything above stays in the root for round 1.** The bus's event vocabulary embeds types from background, interfaces, agent, workspace, a2a and notify, and almost everything publishes on the bus, so nothing between the bus and the top can leave until round 2 moves those types.

## 4. Workspace layout and manifests

The root stays a package. It is not turned into a virtual manifest: `src/`, `tests/`, `build.rs`, the `web/dist` embedding, `include_str!` paths in the root, `RESIDUUM_TS_EXPORT_DIR`'s relative default, the Dockerfiles and the release workflow's `target/<triple>/release/residuum` all keep working unmoved. Members live in `crates/<dir>/`, where `<dir>` is the package name without its `residuum-` prefix (`crates/tracing-service` holds `residuum-tracing-service`); scripts rely on that (§11), and `manifest_scan` rule 7 enforces it.

Root `Cargo.toml` (shape; A1 and the extractions fill it in):

```toml
[package]
name = "residuum"
version.workspace = true
edition.workspace = true
license.workspace = true

[workspace]
resolver = "3"
members = ["crates/*"]
default-members = [".", "crates/*"]
exclude = [".claude", "target"]

[workspace.package]
version = "0.1.0"
edition = "2024"
license = "MIT"
publish = false

[workspace.dependencies]
residuum-core = { path = "crates/core", version = "0.1.0" }
residuum-config = { path = "crates/config", version = "0.1.0" }
# ... one entry per member, then every external dependency of today's [dependencies] and
# [dev-dependencies], with today's version and features (target-specific ones excepted, below)
tokio = { version = "1", features = ["rt-multi-thread", "macros", "process", "fs", "signal", "io-std", "io-util", "sync", "time"] }

[dependencies]
residuum-core = { workspace = true }
tokio = { workspace = true }
# ... what the root's own sources name

[dev-dependencies]
residuum-core = { workspace = true, features = ["test-support"] }
tokio = { workspace = true, features = ["test-util"] }

[workspace.lints.rust]
# today's [lints.rust] table, verbatim, with its comments
[workspace.lints.clippy]
# today's [lints.clippy] table, verbatim, with its comments

[lints]
workspace = true

[profile.dev]
debug = "line-tables-only"

[profile.dev.package."*"]
debug = false
```

A member (`crates/memory/Cargo.toml`; §7 gives every member's `[features]` and workspace `[dev-dependencies]`):

```toml
[package]
name = "residuum-memory"
version.workspace = true
edition.workspace = true
license.workspace = true
publish.workspace = true

[lints]
workspace = true

[features]
test-support = []

[dependencies]
residuum-core = { workspace = true }
residuum-config = { workspace = true }
residuum-inference = { workspace = true }
tantivy = { workspace = true }
# ... every external crate the moved sources name (rule below)

[dev-dependencies]
residuum-core = { workspace = true, features = ["test-support"] }
residuum-config = { workspace = true, features = ["test-support"] }
residuum-inference = { workspace = true, features = ["test-support"] }
ctor = { workspace = true }
rustls = { workspace = true }
tempfile = { workspace = true }
# ... every other external crate the moved test code names
```

Decisions behind the shape:

- **`default-members = [".", "crates/*"]`.** Plain `cargo test`, `cargo clippy`, `cargo build` and `cargo run` from the root keep covering the whole workspace, so every documented command (`cargo test --quiet`, `just check`, the CI steps) keeps its meaning. Checked: `default-members` accepts globs, plain `cargo build` builds every member, plain `cargo test` runs every member's tests, `cargo test --test <name>` still finds the root's integration test, and `cargo run` still finds the one binary.
- **`exclude = [".claude", "target"]` keeps nested checkouts building.** Other checkouts of this repository live inside the checkout: agent worktrees under `.claude/worktrees/`, the verification trees under `.claude/verify*`, and the pre-push hook's stable tree at `target/pre-push/tree` (§11). When Cargo runs in a package whose manifest has no `[workspace]` table, it walks up the parent directories looking for a workspace root, and when it finds one that does not list the package it fails (`error: current package believes it's in a workspace when it's not`). So once the main checkout's `Cargo.toml` has a `[workspace]` table (from A1 on, with or without `members`), every nested checkout of a commit before A1, and the stable tree checking such a commit, would fail every cargo command (reproduced with cargo 1.98.1: a parent with `[workspace]` and a package nested under `.claude/worktrees/agent-x` fails `cargo metadata`; adding the directory to `exclude` fixes it). `exclude` stops the walk from claiming them. A nested checkout of a commit after A1 has its own `[workspace]` table and is its own root, so it needs no entry. `.claude` and `target` hold no member crate, so excluding them whole is the narrowest entry that also covers future worktree and verification directories.
- **`members = ["crates/*"]` is added by the first extraction (E1), not by the scaffold.** Checked: a `members` glob that matches no directory is an error ("failed to load manifest for workspace member"), so the scaffold chunk declares `[workspace]` without members.
- **Every external dependency is declared once in `[workspace.dependencies]`**, carrying the version and features today's root manifest gives it. Members name it with `{ workspace = true }`. A member inherits every feature of the workspace entry and can add features but not remove them, so `tokio = { workspace = true }` gives each member all nine of today's tokio features. That costs nothing: Cargo unifies features across the build anyway, so today's single crate already compiles tokio once with all of them.
- **Target-specific dependencies stay in the root.** `nix` (`[target.'cfg(unix)'.dependencies]`), `windows-sys` and `tauri-winrt-notification` (`cfg(windows)`), `objc2`, `objc2-foundation`, `objc2-user-notifications` and `block2` (`cfg(target_os = "macos")`) are used only by `util/process.rs` (which moves to the root, §5), `daemon.rs` and `notify/**`. They stay in the root's target tables with today's versions and are not added to `[workspace.dependencies]`.
- **Each crate's dependencies are what its sources name.** A member's `[dependencies]` are exactly the external crates its production code names, and its `[dev-dependencies]` the crates only its test code names (`#[cfg(test)]` modules), plus the fixed entries §7 requires (the workspace crates' `test-support`, `ctor` and `rustls` for the TLS-client crates). The extraction finds them with one check per candidate, run from the repository root, where `<ident>` is the dependency's key in the manifest with `-` replaced by `_` (the key, not the package: `a2a`, not `a2a-lf`): `grep -rnE '(^|[^:_[:alnum:]])<ident>::|use <ident>\b|extern crate <ident>\b' crates/<dir>/src` (the first alternative leaves out `crate::<ident>::`, so `notify::` does not match `crate::notify::`). A crate with no hit is not added. A crate whose every hit is in test code (a file declared through `#[cfg(test)] mod`, a `*tests.rs` or `test_*.rs` file, or a line after the file's first `#[cfg(test)]`) goes in `[dev-dependencies]` only; any other hit puts it in `[dependencies]`. The compiler reports a missing one; a production dependency wrongly put in `[dev-dependencies]` fails the isolation check's `--lib` build (§11); a test-only dependency wrongly put in `[dependencies]` builds into production and nothing detects it. After each extraction, the same check over `src/` and `tests/` finds root dependencies the root no longer names, and the chunk removes them (a removal the root still needs fails the build). Nothing in CI detects an unused dependency; this check at extraction is the only one, and a dependency that becomes unused later stays until someone notices. Two rules the check cannot see: a crate that names `tokio::net` lists `features = ["net"]` itself (the workspace `tokio` entry has no `net`; today it arrives through axum and hyper-util, which a crate must not rely on), and §7 lists the dev-dependencies the `test-support` code needs.
- **Internal dependencies carry a version (`{ path, version = "0.1.0" }`).** `deny.toml` sets `wildcards = "deny"`, and cargo-deny treats a path dependency without a version as a wildcard. With the version, `deny.toml` does not change. `publish = false` puts the members under the existing `[licenses.private] ignore = true`.
- **`version.workspace = true` everywhere.** `env!("CARGO_PKG_VERSION")` (`src/a2a/card.rs:191`, `src/tracing_service/otel.rs:65`) keeps reading `0.1.0` in whichever crate it ends up.
- **Profiles stay in the root manifest**, which is where Cargo reads them for the whole workspace. #691's dev-profile debug-info settings apply to every member unchanged. No per-package profile overrides are added (§12 says why).
- **Lower crates have no build script.** Only the root's `build.rs` asserts `web/dist` and sets `RESIDUUM_VERSION`/`RESIDUUM_GIT_COMMIT`; anything that builds only lower crates needs neither.
- **No `doctest = false`.** There are no Rust doctests today (every fenced block in a doc comment is `text`, `toml` or similar), and turning doctests off would make a future doctest silently never run. Each member's library gets a `rustdoc --test` pass with nothing to run, so a full `cargo test` runs nine of those passes instead of one; §15 row 7 measures the whole test run, which includes them.
- **`Cargo.lock` conflicts between parallel chunks** are resolved by taking `main`'s file and letting Cargo add the branch's members, never by regenerating it: after merging `main` into the branch, `git checkout origin/main -- Cargo.lock`, then `cargo metadata --format-version 1 >/dev/null` (Cargo adds the missing workspace entries and changes nothing else), then `git diff origin/main -- Cargo.lock` must show only `[[package]]` entries named `residuum` or `residuum-*` and their `dependencies` lists. `cargo update` and `cargo generate-lockfile` are not used, since they move external versions.

### Rules the root's tests enforce on the manifests

A new test module, `src/testing/manifest_scan.rs` (root crate, `#[cfg(test)]`, next to `source_scan`), reads the root `Cargo.toml` and every `crates/*/Cargo.toml` with the `toml` crate and fails, naming the offending file and key, when:

1. a member lacks `[lints] workspace = true` (otherwise it silently gets rustc's and clippy's defaults);
2. a member's `package.name` does not start with `residuum-`, or it sets `[lib] name` (the log filter in §8 relies on every library target starting with `residuum_`);
3. a member does not inherit `version`, `edition`, `license` and `publish` from the workspace;
4. `test-support` (any member's feature of that name) is enabled from a `[dependencies]` or `[target.*.dependencies]` table rather than `[dev-dependencies]`;
5. the workspace dependency graph, counting normal and dev dependencies, has a cycle (Cargo accepts a dev-dependency cycle and compiles the crate twice, so types and globals stop unifying, with no error);
6. a directory under `crates/` is not matched by both `members` and `default-members`;
7. a member's directory is not its package name without the `residuum-` prefix;
8. a member (or the root) depends on a workspace crate that has a `test-support` feature without turning that feature on in its `[dev-dependencies]` (§7, "Feature sets": otherwise `cargo test -p` and the workspace run build that crate with different features, and each builds it separately).

A4 (which depends on A1) adds one more test there: the dependency graph `scripts/affected-packages.sh --graph` prints equals the one read from the manifests (§11).

These are guards against the shortcuts the repo's lint regime exists for (a member that skips the lints, test code reaching production) and against silent rebuild and selection errors, so they fail the build.

## 5. Upward edges and their resolutions

Every production reference from a module that moves down to a module above its new crate, plus the test-only ones. "Move" means the code moves to the crate named and every caller is rewritten to its new path; nothing is forwarded from the old path (§6, rule 7).

### core

| reference | what | resolution |
|---|---|---|
| `src/util/fs.rs:58` | `config::secrets::set_file_mode_600` | move `set_file_mode_600` into `util::fs`; `config::secrets` calls it there |
| `src/util/tracing_init.rs:9,103,108` | `config::{LogLevel, default_hub_dir, HubPaths}` | move `tracing_init.rs` to the root as `crate::logging` (callers: `src/commands/mod.rs` ×10, `src/commands/serve/daemon.rs:42`, `src/commands/update_watchdog.rs:44`, `src/hub/runtime.rs:888`) |
| `src/util/log_format.rs` (whole file) | only `src/commands/logs.rs:7` uses it | move to the root as `crate::logging::format` |
| `src/util/process.rs` (whole file) | only interfaces, pulse and tools use it; it is also core's only `nix` user | move to the root as `crate::process` |
| `src/util/mod.rs` `MIGRATION_GUIDE_URL`, `HEARTBEATS_REFERENCE_URL` | used only by actions and pulse | move to `crate::pulse` |
| `src/diagnostics/mod.rs:203,244,251,254,261,274-278` | the dispatcher calls `workspace::layout`, `config::{HubConfig, Config}::diagnose_*`, `workspace::config::diagnose_*`, `a2a::client::config::diagnose_a2a_json`, `pulse::types::diagnose_heartbeat`, `skills::diagnose_skill_md` | split the file: the dispatcher (`DiagnosticsPaths`, `FileKind`, `recognize`, `is_recognized`, `diagnose`, `paths_match`) moves to a new root module `crate::diagnostics_dispatch` (A3), and its callers are rewritten to it; `Severity`, `Location`, `Diagnostic` (lines 19-145) stay in `src/diagnostics/mod.rs`, which E1 moves to `residuum_core::diagnostics` |
| `src/testing/wait.rs:14-15,142,193,271` (test) | `bus::{BusHandle, Subscriber, WorkbenchEvent, topics}`, `hub::agent_watch::AgentChangeReceiver` | the generic waits (`guarded`, `until`, `until_reporting`, `until_true`, `watch_until`, `Recv` and its impls for tokio receivers, `next`, `next_matching`, `next_where`, `drain`, `HANG_GUARD`) go to core's `testing::wait`; `next_event`, `bus_barrier` and `impl Recv for AgentChangeReceiver` stay in the root as `crate::testing::bus_wait` (the impl is legal there: the trait is foreign, the type local) |

### config

| reference | what | resolution |
|---|---|---|
| `src/config/resolve/mod.rs:22`, `src/config/types.rs:835,1042` | `inference::{ThinkingConfig, ThinkingLevel}` (`src/inference/types.rs:586-600`) | move both enums to config; `CompletionOptions` names them at `config::` paths, and every caller imports them from config |
| `src/config/resolve/agent.rs:5`, `src/config/types.rs:11` | `inference::retry::RetryConfig` (`src/inference/retry.rs:9-44`: struct, `Default`, `no_retry`) | move to config; `with_retry` stays in inference and takes `&config::RetryConfig`; the `retry` module becomes private; callers of `inference::retry::RetryConfig` (background, gateway, hub) import `config::RetryConfig` |
| `src/config/load.rs:283-291` | `Config::completion_options_for_role` builds `inference::CompletionOptions` | move to inference as `pub fn completion_options_for_role(cfg: &Config, role: &str) -> CompletionOptions`; caller `src/gateway/startup/providers.rs:99`; its tests (`src/config/resolve/models.rs:1166-1229`) move with it |
| `src/config/resolve/agent.rs:23` (and `src/memory/merge_writer.rs:49`, `src/memory/observer/mod.rs:23`, `src/memory/reflector/mod.rs:21`, `src/diagnostics/mod.rs:203`) | `workspace::layout::WorkspaceLayout` | move `src/workspace/layout.rs` (558 lines; its only dependency is `config::paths`) into config as a private `layout` module, `WorkspaceLayout` re-exported at config's root; every caller is rewritten to `crate::config::WorkspaceLayout` (the compiler finds them; about 340 references in 66 files on the starting tree, tests included). Its `#[cfg(test)] tempdir_guard` field and `#[cfg(test)] pub(crate) fn with_tempdir_guard` are test hooks root tests use (§7) |
| `src/config/wizard.rs:282-285` | `workspace::bootstrap::ensure_workspace_labeled`, `workspace::team_files::TeamWriteCoordinator` | move `wizard.rs` to the root as `crate::setup_wizard` (its only caller is `src/commands/setup.rs:50-99`; everything it needs from config is already public) |
| `src/config/resolve/channels.rs:17`, `src/config/types.rs:543` | `interfaces::types::WEB_UI_ENDPOINT` | move the const to `config::constants`, re-exported at config's root; its other references are rewritten |
| `src/config/types.rs:12`, `src/config/resolve/agent.rs:6` | `skills::SkillDir` | move `SkillDir` and `SkillSource` (`src/skills/types.rs`) into config; skills' own signatures (`SkillIndex::scan(dirs: &[SkillDir])`, `SkillIndexEntry::source`) name them at `config::` paths, and every caller imports them from config |

### inference

| reference | what | resolution |
|---|---|---|
| `src/inference/failover.rs:14,44-121`, `src/inference/factory.rs:227,261` | `bus::{Publisher, NoticeEvent, NotifyName, SYSTEM_CHANNEL, topics}` for fallback and recovery notices | inference defines `#[async_trait] pub trait NoticeSink: Send + Sync { async fn notice(&self, message: String); }`; `FailoverNotices` holds an `Arc<dyn NoticeSink>`, and `build_provider_chain_with_notices` / `_with_shared_notices` take one. The root implements `NoticeSink for bus::Publisher` in the bus module, publishing exactly what `notice_on_transition` publishes today and keeping its `warn!` when the publish fails. Callers: `src/background/spawn_context.rs:350`, `src/gateway/startup/providers.rs:42`, `src/gateway/startup/memory.rs:54,108`, `src/subconscious/mod.rs:213` |
| failover tests (`src/inference/failover.rs:483-1316`, 9 × `bus::spawn_broker`) | assert the notices through a bus subscriber | assert them through a recording `NoticeSink` in the test module; one root test checks that the `Publisher` impl delivers a notice on the system channel |
| `src/inference/providers/anthropic.rs:2229,2270,2301` (test) | `hub::test_support::EventLog` | `EventLog` and `LoggedEvent` (`src/hub/test_support.rs:40-92`, a generic tracing capture layer) move to core's `testing::log`; hub's `mount_reply` and `write_agent` stay |
| `src/testing/model.rs:22` (test helper) | depends on inference | moves into inference as `testing::model` behind inference's `test-support` |

### mcp

| reference | what | resolution |
|---|---|---|
| `src/mcp/client.rs:26`, `src/mcp/registry.rs:15` | `tools::{ToolError, ToolResult}` (`src/tools/mod.rs:79-107,148-210`) | move both types and their impls into inference (`tool` module, re-exported at inference's root); the `Tool` trait's signatures name them at `inference::` paths, and every caller (tools, the tool implementations, mcp) imports them from inference |
| `src/mcp/registry.rs:15` | `tools::SharedToolsPath` and `apply_tools_path` (`src/tools/mod.rs:61-78`) | move both to config, next to `ToolsConfig::effective_path` that fills the value |
| `src/a2a/client/config.rs:193` → `src/mcp/client.rs:303` | a2a calls mcp's `expand_env_vars` | move `expand_env_vars` and its tests into `residuum_core::util` (a generic `${VAR}` expander used by two config readers) |

### memory

| reference | what | resolution |
|---|---|---|
| `src/memory/{merge_writer.rs:49, observer/mod.rs:23, reflector/mod.rs:21}` | `WorkspaceLayout` | moves to config (above) |
| `src/memory/mod.rs:29` `strip_code_fences` | used by memory, subconscious and `gateway/web/model.rs` | move to inference (it strips the fences a model wraps JSON in) |
| `src/memory/{merged_run_log.rs:10, recent_messages.rs:112, observer/mod.rs:104}` | intra-doc links to `crate::background::…`, `crate::gateway::…` | rewrite as plain text naming the root module; CI runs no `cargo doc`, so a broken link would not be reported |
| `#[serde(with = "crate::time::…")]` ×3 | string path the scan does not count | becomes `residuum_core::time::…` (the compiler checks it) |

### tracing_service

| reference | what | resolution |
|---|---|---|
| `src/tracing_service/client_context.rs:12` | `inference::{ThinkingConfig, ThinkingLevel}` | they move to config (above) |
| `src/tracing_service/client_context.rs:35,36,66,67,155` | `env!("RESIDUUM_VERSION")`, `option_env!("RESIDUUM_GIT_COMMIT")` | move `client_context.rs` to the root as `crate::client_context` (§9); `ClientContext` and `FeedbackClient` stay in the crate as data |

### net

| reference | what | resolution |
|---|---|---|
| `src/pairing/gate.rs:26`, `src/remote_access/engine.rs:38`, `src/remote_access/proxy.rs:22`, `src/workbench/forward.rs:37`, `src/workbench/server.rs:36` | `gateway::browser_landing` (512 lines; its only crate dependency is `pairing::remote::is_navigation`) | move to net as `browser_landing` |
| `src/workbench/server.rs:168` | `gateway::cross_site` | move to net as `cross_site` |
| `src/remote_access/engine_tests.rs:85` (test) | `gateway::remote_control_guard` (112 lines; depends only on `pairing::remote`) | move to net as `remote_control_guard`; the gateway and hub routes keep applying it |
| `src/workbench/mod.rs:23` | `gateway::protocol::ArtifactSummary` (`src/gateway/protocol.rs:395`, `#[ts(export)]`) | move into `workbench`, in E6 rather than B4: until net is a crate, `workbench` is a `pub(crate)` module, so `tests/ts_export.rs` could not reach the type there. After E6, `gateway/web/workbench.rs` imports `crate::workbench::ArtifactSummary` and `tests/ts_export.rs` names `residuum_net::workbench::ArtifactSummary` directly (the facade's `workbench` alias is `pub(crate)`; integration tests may use the root's normal dependencies). Its public path `residuum::gateway::protocol::ArtifactSummary` goes away: rule 6 keeps visibility, and a moved item changes path under rule 7 |
| `src/workbench/mod.rs:22,24,386-387` | `features::FEATURES`, `update::CURRENT_VERSION` | pass them in: `workbench::SdkContext { version: &'static str, features: &'static [&'static str] }` is a parameter of `workbench::server::start` and is threaded to `inject_sdk`; the hub builds it from `update::CURRENT_VERSION` and `features::FEATURES` |
| `src/workbench/watcher.rs:17` (and its test at `:256`) | `bus::{BusHandle, Publisher, WorkbenchEvent, WorkspaceEvent, topics}` | move `watcher.rs` with its tests to the root as `crate::hub::workbench_watcher` (callers `src/hub/services.rs:80`, `src/gateway/event_loop/run_loop.rs:608`). The watcher calls `super::discover_artifacts` and uses `DiscoveredArtifact` (`src/workbench/watcher.rs:120`), which B4 makes `pub(crate)` in `workbench` and E6 decides by §6's rule (listed under net in §6) |
| `src/remote_access/proxy.rs:253`, `src/remote_access/engine_tests.rs:484` | `interfaces::teams::MESSAGES_PATH` (`src/interfaces/teams/mod.rs:47`) | move to net as `remote_access::TEAMS_MESSAGES_PATH`; the Teams adapter imports it from there |
| `src/remote_access/proxy.rs:405` | `a2a::public_url::AGENTS_PATH_PREFIX` (`src/a2a/public_url.rs:10`) | move to net as `remote_access::A2A_AGENTS_PATH_PREFIX`; a2a imports it from there (the proxy forwards these routes, so it owns their paths) |

### skills

| reference | what | resolution |
|---|---|---|
| none upward | skills uses `util` (frontmatter, `xml_escape`), the diagnostic types and, after the move above, `SkillDir`/`SkillSource` | — |

After these moves the graph in §3 is acyclic for production and test code. A scan of every moving module for inherent or trait impls that the split would turn into orphan-rule violations found none.

## 6. Public API and visibility

### Policy

1. **Each crate's API is what its root exports.** `lib.rs` re-exports the items other crates use. Internal modules are private or `pub(crate)`.
2. **A module may itself be public when it is a namespace callers name**, because `module::item` reads better than a root name or because the language needs a path (serde `with = "…"` modules). The allowed public namespace modules are listed per crate below; inside them only the items used across the boundary are `pub`.
3. **Prefer moving code to the crate that uses it over widening it for one caller.** §5 is the result: code used only by the root moves up (tracing_init, log_format, process, the diagnostics dispatcher, client_context, the wizard, the workbench watcher); code used by a lower crate and the root moves to the lowest user (browser_landing, the path constants, `strip_code_fences`, `expand_env_vars`).
4. **No blanket `pub`.** Making a whole module `pub`, or turning every `pub(crate)` in a file into `pub`, to make the build pass is not allowed. Each widened item has a caller in another crate and is decided by the rule below.
5. **Fields stay private on types with invariants**; such types get methods. Plain data types (serde payloads, config structs, protocol and status types) keep or gain `pub` fields.
6. **Existing `pub` stays `pub`.** Items, and modules, that are already public API of the `residuum` library (for `commands` and `tests/`) stay public; this work does not narrow them.
7. **Moved items are never re-exported from their old home.** Where a public signature in the old home names a moved item (`tools::Tool` returning `ToolResult`, `inference::with_retry` taking `RetryConfig`, `CompletionOptions`' `ThinkingConfig` field, `SkillIndex::scan` taking `&[SkillDir]`, `workspace::identity::load` taking `&WorkspaceLayout`, the dispatcher returning `Diagnostic`), the signature names the item at its new path, and every caller imports it from there. This always works: an item moves only into a crate that each of its old home's callers already depends on. The root depends on every crate; every lower crate depends on core and config, where every moved type lands except `ToolError`/`ToolResult`, whose callers outside the root (mcp) depend on inference.
8. **Test-only cross-crate items** (`#[cfg(test)]` today, used by another crate's tests) are `#[cfg(any(test, feature = "test-support"))] pub` (§7). Production items that only another crate's tests call become plain `pub`; production code is never put behind the feature.
9. **Newly public items meet the public-item lints**: `missing_errors_doc`, `missing_panics_doc` and `must_use_candidate` apply to them, so each gets its `# Errors` section and `#[must_use]` as the lints ask.

### Deciding each widening

The per-crate lists below are the scan's path-visible items. During each extraction the compiler also reports methods and fields that another crate reaches through a value; for net the scan estimated up to 56 methods and 42 fields used from hub and gateway. Every item the compiler reports as private to another crate is decided by this rule, in order, by the implementer, without a separate review:

1. **Move the caller's code in** when the caller uses the item only to reach the crate's internals: the block of caller code that touches the item names no root item other than the values it receives and returns. That block becomes a method or function of the lower crate, named for what it does, and the item stays private. (Example shape: a hub function that reads three `RemoteAccess` fields to build a status line moves into `remote_access` as one method.)
2. **Make a field `pub`** when it belongs to a plain data type (rule 5).
3. **Add a getter** (`pub fn x(&self) -> &T`) for a field of a type with invariants; a write the caller needs goes through a method that keeps the invariant, never a public field or a plain setter.
4. **Make the item `pub`** otherwise (an operation the caller invokes), at the narrowest path that serves it: re-exported from the crate root or from a listed namespace module, with the defining module left private.
5. **A test-only item** follows rule 8: `#[cfg(any(test, feature = "test-support"))] pub` if it is `#[cfg(test)]` today, plain `pub` if it is production code.

The chunk's pull request description lists every item the chunk made `pub` (path, kind, which step of this rule applied, and its callers outside the crate) and every public namespace module, against the lists below. The list is the record; the PR merges through the queue like any other.

### The root crate's facade

Each moved module keeps its name in the root: `src/lib.rs` replaces its `pub mod x;` with one alias, so `crate::x::…` inside the root and `residuum::x::…` in `commands/` and `tests/` resolve to the crate that now holds it. An alias names a whole crate, or one whole module of a crate, under the module name it had; every item under it has the one path it had before, so the alias does not forward any item from somewhere else. Items that moved between modules (§5, rule 7) are not reachable at their old module path. After E8, the moved and new lines of `src/lib.rs` read:

```rust
pub use residuum_core::{diagnostics, time, util};
pub use residuum_config as config;
pub use residuum_config::agent_keys;
pub use residuum_skills as skills;
pub use residuum_inference as inference;
pub use residuum_tracing_service as tracing_service;
pub use residuum_memory as memory;
pub use residuum_mcp as mcp;
pub use residuum_net::{pairing, remote_access};
pub(crate) use residuum_net::{browser_landing, cross_site, remote_control_guard, tunnel, workbench};

pub mod client_context;
pub mod diagnostics_dispatch;
pub mod logging;
pub mod process;
pub mod setup_wizard;
#[cfg(test)]
pub(crate) mod testing;
```

The visibility of each name follows rule 6 and who uses it: `pairing` and `remote_access` are `pub` today and stay so; `tunnel` and `workbench` are `pub(crate)` today and stay so, and the three helpers were `pub(crate)` modules of `gateway`. `logging` (today's `pub mod util::tracing_init` and `pub mod util::log_format`, as `logging` and `logging::format`) and `setup_wizard` (today's `pub mod config::wizard`) are `pub` because the binary reaches them as `residuum::logging` (`commands/mod.rs`, `commands/serve/daemon.rs`, `commands/update_watchdog.rs`, `commands/logs.rs`) and `residuum::setup_wizard` (`commands/setup.rs`). `client_context` (today's `pub mod tracing_service::client_context`), `diagnostics_dispatch` (today's `pub mod diagnostics`) and `process` (today's `pub mod util::process`) have only library callers but are `pub` today, so they stay `pub`. `hub::workbench_watcher` is declared `pub(crate) mod workbench_watcher;` in `src/hub/mod.rs`, as `workbench::watcher` is today.

Rules for it: each name in `crate::` is either a local module or exactly one alias declared here. No local module merges another crate's items with its own through a glob (a local item silently shadows a glob import). Root code names a lower crate's items through these aliases (`crate::memory::…`, and `crate::agent_keys::…` rather than `crate::config::agent_keys::…`); the one exception is core's `testing` module, which has no alias because the root keeps a local `testing` module, so root tests name it `residuum_core::testing::…`. Lower crates name each other by crate (`residuum_config::Config`), never through a root-level `use` that makes `crate::config` resolve to another crate.

### Per crate

Items marked "new" are `pub(crate)` today and are used from another crate after the split (from the scan's path-visible list; methods and fields reached through values are decided during extraction by the rule above).

**`residuum-core`**
- Public namespaces: `util`, `util::fs`, `util::telemetry`, `util::frontmatter`, `util::backoff_tracker`, `util::secret_compare`, `time` (and `time::minute_format`, `time::minute_format_opt` for serde), `diagnostics`, `testing` (feature `test-support`).
- `util` root re-exports as today (`FatalError`, `spawn_in_span`, `spawn_blocking_in_span`, `spawn_monitored`, `panic_message`, `BackoffTracker`, `NoticeAction`, `RetryGate`, `parse_frontmatter_md`, `validate_kebab_name`, `secrets_match`, `xml_escape`), plus `expand_env_vars` (moved in).
- `util::fs`, new: `atomic_write`, `atomic_write_owner_only`, `is_atomic_write_temp`, `is_held_open`, `rename_dir_when_released`, and `set_file_mode_600` (moved in). `is_held_open` and `rename_dir_when_released` have one outside caller (`src/hub/provision.rs`) but stay here: `is_held_open` is shared with `atomic_write`'s retry, and both are Windows-aware filesystem primitives.
- `diagnostics`: `Diagnostic`, `Location`, `Severity`.
- Moves instead of widening: `util::process::{Capture, RunOutcome, run_with_timeout_and_cancel, shell_command}` (root `process`), `MIGRATION_GUIDE_URL`, `HEARTBEATS_REFERENCE_URL` (root `pulse`), `tracing_init`, `log_format` (root `logging`).

**`residuum-config`**
- Root re-exports as today (`src/config/mod.rs:26-53`), plus, new: `DEFAULT_A2A_PORT`, `DEFAULT_CLOUD_RELAY_URL`, `DEFAULT_GATEWAY_PORT`, `DEFAULT_OBSERVER_COOLDOWN_SECS`, `DEFAULT_OBSERVER_FORCE_THRESHOLD`, `DEFAULT_OBSERVER_THRESHOLD`, `DEFAULT_REFLECTOR_THRESHOLD`, `DEFAULT_SUBCONSCIOUS_EVERY_N_ITERATIONS`, `DEFAULT_SUBCONSCIOUS_MAX_TRANSCRIPT_TOKENS`, `DEFAULT_TEAMS_PORT`; `AgentConfigFile`, `HubConfigFile`, `ProvidersFile` (from the private `deserialize`); `from_file_and_env`, `resolve_hub_config`, `removed_agent_env_override_notices` (from the private `resolve`); `display_name_in_toml`.
- Moved in and re-exported at the root: `WorkspaceLayout`, `ThinkingConfig`, `ThinkingLevel`, `RetryConfig`, `WEB_UI_ENDPOINT`, `SkillDir`, `SkillSource`, `SharedToolsPath`, `apply_tools_path`. Behind `test-support`: `WorkspaceLayout::with_tempdir_guard` (§7).
- Public namespaces: `paths` (already public), `patch` (new; its edit functions read as `patch::…`), `secrets` (new; `SecretStore`, `is_reference`, `ENCRYPTED_FILE`), `agent_keys` (its API as today).
- Stays crate-private: `secrets::{decrypt, encrypt, load_key, load_or_create_key}` (only `agent_keys` uses them, inside the crate).
- Moves instead: the wizard (root `setup_wizard`), `completion_options_for_role` (inference).

**`residuum-skills`**: its root API as today, minus `SkillDir` and `SkillSource` (moved to config; callers import them from there), plus `diagnose_skill_md` (new; the root's dispatcher calls it).

**`residuum-inference`**
- Root re-exports as today, minus `ThinkingConfig` and `ThinkingLevel` (moved to config), plus, new: `DroppedFallback`, `build_embedding_provider`, `build_provider_chain`, `build_provider_chain_with_notices`, `build_provider_chain_with_shared_notices`, `NullProvider`.
- New namespace re-export `anthropic` (`pub use providers::anthropic;`) with `OAUTH_BETA`, `OAUTH_USER_AGENT`, `is_oauth_key` made `pub`; the rest of `providers` stays private.
- Moved in: `ToolError`, `ToolResult`, `strip_code_fences`, `completion_options_for_role`; new: `NoticeSink`.
- The `retry` module becomes private (`with_retry` re-exported at the root); `RetryConfig` is config's.
- `testing::model` behind `test-support`.

**`residuum-tracing-service`**: its root API as today (`TracingService`, the status, export, report and feedback types, `redact_agent_key_values`, `sanitize_spans`), without `client_context`.

**`residuum-memory`**
- Public namespace `episode_store` (new): `find_episode_path`, `read_episode_lines`, `format_message_line`, `older_main_episode_id`, `transcripts_newest_first`, `DEFAULT_LINES`, `MAX_LINES`, with the already-public `read_episode_jsonl` and `EpisodeMeta`; also `write_episode_transcript_tagged` and `episode_jsonl_path`, which are production code that root tests call. Behind `test-support`: `write_episode_transcript` and `latest_episode_id`, which are `#[cfg(test)]` today.
- Root, new: `read_idx_jsonl` (from `chunk_extractor`), `parse_obs_file` (from `search`).
- `testing` (today's `test_helpers`) behind `test-support`.
- Moves instead: `strip_code_fences` (inference).

**`residuum-mcp`**: its root API as today. Moves instead: `expand_env_vars` (core). The registry's `#[cfg(test)] pub fn mark_stopped/mark_failed/stop_all` have no caller outside mcp and stay `#[cfg(test)]`.

**`residuum-net`**
- Public namespaces: `tunnel`, `remote_access`, `pairing`, `workbench`, `browser_landing`, `cross_site`, `remote_control_guard`. Each namespace's `mod.rs` re-exports what crosses the boundary; deeper modules stay private.
- `pairing`, new: `AuthenticatedDevice`, `GateState`, `Issued`, `PAIRING_PAGE_PATH`, `PollOutcome`, `Surface`, `cookie_header`, `device_gate`, `RemoteContext`, `RemoteTransport`, `remote_context`, `is_navigation` (re-exported from the private `remote` module, so `pairing::remote::…` callers use `pairing::…`).
- `remote_access`, new: `EngineRouters`, `Notifier`, `RemoteAccess`, `RemoteAccessInputs`, `SecureDiscovery` (re-exported from the private `engine`, `manager` and `siblings::discovery`); moved in: `TEAMS_MESSAGES_PATH`, `A2A_AGENTS_PATH_PREFIX`. Already-public `status` and `types` keep their paths for `tests/ts_export.rs`.
- `tunnel`, new: `start_tunnel`, `SessionHandler`, `AgentInfo`, `web_origin` (re-exported from the private `v2` and `v2::frames`).
- `workbench`, new: `ArtifactDeleteError`, `ArtifactsOrigin`, `HubApi`, `is_valid_artifact_name`, `start` (from the private `server`); moved in: `ArtifactSummary`, `SdkContext`; added by B4's watcher move (not in the scan's list): `discover_artifacts`, `DiscoveredArtifact`.
- `browser_landing`, `cross_site`, `remote_control_guard`: their current `pub(crate)` items used by the gateway and hub become `pub`.
- Test hooks used by root tests (`tunnel::v2::test_relay`, `pairing` service's `advance_clock` and `pair_device_for_tests`, `remote_access::siblings::client::in_process` and `plain_http_for_tests`, `remote_access::jws::test_key`) move behind `test-support`.
- The methods and fields hub and gateway reach through values are decided by "Deciding each widening" above and listed in E6's pull request.

## 7. Test support

- **Each crate's own tests use `#[cfg(test)]`, as today.**
- **Helpers other crates' tests use** live in the lowest crate that can hold them, behind a `test-support` feature, enabled only through `[dev-dependencies]` (`manifest_scan` rule 4):
  - `residuum-core`: `testing::{wait, gate, clock, script, log, ports}`: today's `src/testing/{wait,gate,clock,script}.rs` (the generic part of `wait`, §5), `EventLog`/`LoggedEvent` from `src/hub/test_support.rs`, and `src/util/test_ports.rs`.
  - `residuum-config`: `WorkspaceLayout`'s `tempdir_guard` field and `with_tempdir_guard` (`src/workspace/layout.rs:18,40`, used by root tests in `background/`, `checkpoints/` and `a2a/`).
  - `residuum-inference`: `testing::model` (today's `src/testing/model.rs`).
  - `residuum-memory`: `testing` (today's `src/memory/test_helpers.rs`) and the two transcript writers.
  - `residuum-net`: the hooks listed in §6.
  - `residuum-skills`, `residuum-tracing-service`, `residuum-mcp`: no feature; no root test uses a test-only item of theirs.
- **Every crate's own tests build on their own.** `cargo test -p residuum-<x>` must pass for every member, from a checkout without `web/dist`, with no other package in the build. Every E-chunk's gate runs it for the crate it extracts, and the isolation check (§11) compiles every member's tests on their own (`cargo check -p <pkg> --all-targets`) in each merge queue run. Two rules make the table below complete:
  - A `test-support` gate is `#[cfg(any(test, feature = "test-support"))]` on the item, and on a struct field the same gate goes on the field and on every initializer of it.
  - Code under that gate is compiled in two ways: with the feature (another crate's test build) and with `cfg(test)` alone (the crate's own tests, where the feature is off, since a crate cannot turn on its own feature without dev-depending on itself, which `manifest_scan` rule 5 forbids). So every external crate or external feature such code needs is declared twice: as an optional dependency (or a feature of a dependency) that `test-support` turns on, and in `[dev-dependencies]`. config's `tempfile` is the pattern.

  | crate | `[features] test-support = …` | normal dependencies the feature needs | workspace `[dev-dependencies]` (each with `features = ["test-support"]` where the crate has it) | external `[dev-dependencies]` |
  |---|---|---|---|---|
  | core | `["tokio/test-util", "tokio/net"]` | `tokio` (already a dependency): `testing::clock` calls `tokio::time::advance` and `pause` (`test-util`), `testing::ports` converts its listener with `tokio::net::TcpListener::from_std` (`net`) | none (core's tests use its own `testing` directly) | `tokio = { workspace = true, features = ["test-util", "net"] }`, `tempfile` |
  | config | `["dep:tempfile"]` | `tempfile = { workspace = true, optional = true }`: the guard field holds an `Arc<tempfile::TempDir>` | core | `tempfile` |
  | skills | — | | core, config | `tempfile` |
  | inference | `["dep:axum", "residuum-core/test-support"]` | `axum = { workspace = true, optional = true }`: `testing::model` serves through axum and holds requests with core's `testing::gate` | core, config | `axum`, `wiremock`, `ctor`, `rustls` |
  | tracing-service | — | | core, config | `tempfile`, `ctor`, `rustls` |
  | memory | `[]` | none: its helpers use only memory's and inference's production items | core, config, inference | `tempfile`, `ctor`, `rustls` |
  | mcp | — | | core, config, inference | `tempfile`, `ctor`, `rustls` |
  | net | `["residuum-core/test-support"]` | none: `tunnel::v2::test_relay` waits through core's `testing::wait::guarded`, and `remote_access::pebble_support` reserves ports through `testing::ports` | core, config | `tempfile`, `http-body-util`, `hyper = { workspace = true, features = ["client"] }`, `ctor`, `rustls` |
  | root | (no feature) | | core, config, inference, memory, net (the five with the feature); skills, tracing-service and mcp only as normal dependencies | as today: `ctor`, `tempfile`, `tokio` with `test-util`, `wiremock`, `http-body-util`, `hyper` with `client`, `jsonschema` |

  Each external entry is `{ workspace = true }` unless the table gives features. net's normal `tokio` dependency also lists `features = ["net"]`, because `tunnel/v2/session.rs` and `workbench/server.rs` name `tokio::net` in production code (§4). `tracing-subscriber`, used by `testing::log`, is already a normal dependency of core (`util::telemetry`). Crates other than core get tokio's `test-util` in their tests (for `#[tokio::test(start_paused = true)]`, which mcp's and net's tests use) through their dev-dependency on core's `test-support`, not through an entry of their own. The table was derived from the starting tree by the check in §4 plus the uses of `tokio::time::{pause, advance}`, `start_paused` and `tokio::net`; if an E-chunk's `cargo test -p` or isolation check shows an entry missing, the chunk adds it to the manifest and to this table in the same pull request.
- **Feature sets, and what `cargo test -p` shares with the workspace run.** Cargo builds a crate once per distinct feature set, and keeps each variant in the target directory under its own hash, so different sets cost a first build each but never evict each other. `manifest_scan` rule 8 makes every package turn on `test-support` for each workspace dependency that has it, so in any test build, `cargo test -p residuum-memory` or the workspace run, core, config and inference have the same features (and core's turns on tokio's `test-util` in both). Two kinds of difference remain, and both are expected: a plain `cargo build` builds the workspace crates without `test-support`, as today's `cargo build` builds them without `cfg(test)`; and an external crate gets a dev-only feature only in the builds whose dev-dependencies ask for it (for example `hyper`'s `client`, which the root's and net's tests use), so `cargo test -p residuum-memory` builds hyper, and what depends on it, once more without that feature. §15 row 3b is timed after one untimed `-p residuum-memory` build, so it measures the rebuild and not these first builds.
- **The TLS provider in every test binary that can build an HTTP client.** reqwest panics in `Client::new()` until a process-default `CryptoProvider` is installed, and a ctor in a library would only run if the linker keeps its object file, so each test binary includes the installer by path, as the root does today. E1 moves `src/testing/crypto.rs` to `crates/core/src/testing/crypto.rs`. It is not a module of core (core builds no HTTP client): `crates/core/src/testing/mod.rs`'s doc says so. Each crate whose code builds HTTP clients declares, in its `lib.rs`, `#[cfg(test)] #[path = "../../core/src/testing/crypto.rs"] mod crypto;` and has `ctor` and `rustls` (workspace entries, ring) in its `[dev-dependencies]`: inference (E4), tracing-service (E5), net (E6), memory (its tests build providers through inference, E7) and mcp (E8). The root's lib (`src/testing/mod.rs`, `#[path = "../../crates/core/src/testing/crypto.rs"]`), its bin (`src/main.rs`) and `tests/it/support/mod.rs` change their `#[path]` in E1. A crate that misses it fails its own tests with reqwest's "No provider set" panic.
- **Who rewrites the `crate::testing` callers.** E1 moves `wait`, `gate`, `clock`, `script`, `log` and `ports` to core and rewrites every root caller to `residuum_core::testing::…` (§6 explains the missing alias); the compiler finds them (on the starting tree, 144 references to `crate::testing::{wait, gate, clock, script, model}` in 76 files, plus the `EventLog` and `test_ports` users). `crate::testing::bus_wait` callers keep their path. `src/hub/test_support.rs:39` (`pub(crate) use crate::util::test_ports::reserve_port;`) is a single-item forwarding re-export, which rule 7 does not allow: B6 deletes it and rewrites its callers (`src/hub/runtime.rs:1122`, `src/hub/host/tests.rs:21`) and the direct `crate::util::test_ports` users (`src/remote_access/pebble_support.rs:34`, `src/hub/runtime.rs:1470,1502`) to `crate::testing::ports`, which E1 then rewrites to `residuum_core::testing::ports` with the rest. E4 rewrites `crate::testing::model` callers to `crate::inference::testing::model`. Callers inside a lower crate that moved before E1 do not exist (E1 is the first extraction).
- **No dev-dependency cycles.** A crate dev-depends only on crates at or below its normal dependencies. core's own tests use its `testing` module directly (it is in the same crate, compiled under `cfg(test)` with the feature off, which is why core's `tokio` dev-dependency carries `test-util` and `net`), so core never dev-depends on anything in the workspace, itself included. `manifest_scan` rule 5 enforces this.
- **The root keeps** `src/testing/` with `bus_wait` (the bus and hub waits), `source_scan`, `manifest_scan`, `wall_clock_backlog` and the crypto include; `hub::test_support` keeps `mount_reply` and `write_agent`. `src/testing/mod.rs`'s doc is rewritten to point at `residuum_core::testing` for the waits, gates and clock.
- **Clippy treats `test-support` code as production code.** `clippy.toml`'s `allow-unwrap-in-tests`, `allow-expect-in-tests` and `allow-panic-in-tests` cover items under `#[cfg(test)]` or `#[test]`, and clippy does not read `#[cfg(any(test, feature = "test-support"))]` as test code. `cargo clippy --all-features` turns the feature on, so `EventLog`'s `.lock().unwrap()` and the hang guard's `panic!` are denied there. Each such site gets a scoped `#[expect(clippy::…, reason = "test-support helper: …")]`. Here the expectation is fulfilled, unlike the forbidden `#[expect]` on `#[cfg(test)]` code that `CLAUDE.md` describes; E1 confirms this with the first helper it moves.
- **The source scan treats `test-support` code as test code** (chunk A2), so a `sleep` in a `test-support` helper outside the exempt wait and clock modules fails the scan as it does today.
- **Integration tests stay in the root's `tests/it/` binary.** Every lower crate's change also tests the root (it depends on all of them), so they keep running for every change they cover. Members get no `tests/` directory in round 1. `tests/it/support/{until,guarded}.rs`, which duplicate two waits because `crate::testing` is invisible to integration tests today, are replaced by `residuum_core::testing::wait` (chunk F1).

## 8. Logging targets

`tracing` targets default to `module_path!()`, so code in `residuum-memory` logs as `residuum_memory::search` where it logged as `residuum::memory::search`.

**The default filter keeps every crate's logs.** `LogLevel::filter_str` returns `"residuum=debug,warn"` / `"residuum=trace,warn"` (`src/config/types.rs:856-857`). tracing-subscriber matches a directive's target as a string prefix (`Directive::cares_about_target` and `StaticDirective::cares_about` use `starts_with`, tracing-subscriber 0.3.23 `src/filter/directive.rs:182,250`, `src/filter/env/directive.rs:246`), so `residuum=debug` enables `residuum_memory::search` too. The filter string does not change. What makes this hold for every crate is the naming rule (every library target starts with `residuum_`, enforced by `manifest_scan` rule 2) plus a test, added by chunk A6, that reads the workspace member list from the manifests and, for each member's library name and for `residuum`, builds a debug-level `Metadata` with that target and asserts the subscriber built from `LogLevel::Debug.filter_str()` enables it, while a `hyper::proto` debug event stays off. If tracing-subscriber ever moves to segment-aware matching, or a crate is named outside the rule, that test fails.

**`residuum logs` displays and matches crate targets.** Today `expand_module_filter` turns a value without `::` into the prefix `residuum::<value>` and passes a value containing `::` through unchanged (`src/util/log_format.rs:60-66`); an entry matches when its target starts with that prefix, as a plain string prefix (`src/commands/logs.rs:90-91`), so `--module agent` also matches `residuum::agent_keys::…`; the display strips `residuum::` (`:127`). After chunk A6, for an entry with target `T`:

- **Display form** `D(T)`: `T` with its first segment rewritten: `residuum::` is removed, and `residuum_<x>` becomes `<x>`. **Module form** `M(T)`: `T` with its first segment removed. For a root target the two are the same.

  | target | display |
  |---|---|
  | `residuum::hub::runtime` | `hub::runtime` |
  | `residuum_memory::search` | `memory::search` |
  | `residuum_inference::factory` | `inference::factory` |
  | `residuum_tracing_service::submit` | `tracing_service::submit` |
  | `residuum_config::load` | `config::load` |
  | `residuum_config::agent_keys::store` | `config::agent_keys::store` |
  | `residuum_core::util::fs` | `core::util::fs` |
  | `residuum_net::tunnel::v2` | `net::tunnel::v2` |

  Crates that hold one of today's modules display as today; the ones that bundle several (core, config's `agent_keys`, net) show the crate name too. The rule needs no per-crate table.
- **Matching.** Let `F` be the `--module` value and `F'` be `F` with one leading `residuum::` removed. The entry matches when `F` is a prefix of `T`, or `F'` is a prefix of `D(T)`, or `F'` is a prefix of `M(T)`. Every prefix test is a plain string prefix, as today, not a whole-segment one. A value can match targets in several crates, and the result is their union. For a module that keeps its name, every value that matches its entries today matches the same entries after the split, because its old path below `residuum::` is either its `D` (single-module crates) or its `M` (modules bundled into core, config and net). Modules that change name or home in A3 and the B chunks do not keep their old values: `util::tracing_init` and `util::log_format` (now `logging` and `logging::format`), `util::process` (`process`), `config::wizard` (`setup_wizard`), `diagnostics`' dispatcher (`diagnostics_dispatch`, which `diagnostics` still matches as a prefix), `workspace::layout` (`config::layout`), `gateway::browser_landing`, `gateway::cross_site` and `gateway::remote_control_guard` (`browser_landing`, `cross_site`, `remote_control_guard`, in net after E6), `workbench::watcher` (`hub::workbench_watcher`) and `tracing_service::client_context` (`client_context`). Their entries match their new names.

  | `--module` | target | matches | through |
  |---|---|---|---|
  | `agent` | `residuum::agent::core` | yes | `D` |
  | `agent` | `residuum_config::agent_keys::store` | yes | `M`, a plain prefix, as `agent` matches `residuum::agent_keys` today |
  | `memory` | `residuum_memory::search` | yes | `D` |
  | `tunnel` | `residuum_net::tunnel::v2` | yes | `M` |
  | `net` | `residuum_net::pairing::gate` | yes | `D` (new: a crate's short name) |
  | `config` | `residuum_config::agent_keys::store` | yes | `D` |
  | `hub` | `residuum::hub::runtime` | yes | `D` |
  | `residuum::tunnel::v2` | `residuum_net::tunnel::v2::link` | yes | `M`, after removing `residuum::` |
  | `residuum::mcp::client` | `residuum_mcp::client` | yes | `D`, after removing `residuum::` |
  | `residuum_net::tunnel` | `residuum_net::tunnel::v2` | yes | `T` (new: a crate target written out) |
  | `diagnostics` | `residuum_core::diagnostics` | yes | `M` |
  | `diagnostics` | `residuum::diagnostics_dispatch` | yes | `D`; with the row above, `diagnostics` matches both crates |
  | `search` | `residuum_memory::search` | yes | `M` (new: a module below a crate matches by its own name) |
  | `tools` | `residuum_net::tunnel::v2` | no | — |
- When a non-watch `residuum logs` run with any of `--module`, `--level` or `--agent` prints no entries, it prints one line to stderr, after reading the whole file: `No log entries matched (--module <value>, --level <value>, --agent <value>).`, naming only the filters given, so a filter that names nothing is not mistaken for a quiet log. The exit status stays 0. A6 tests it (a log file with entries, a `--module` value that matches none, the line on stderr and nothing on stdout).
- The test list at `src/hub/host/tests.rs:1322-1325` (`is_hub_level`) changes `residuum::tunnel` to `residuum_net::tunnel` in E6. If it is missed, that test fails, because tunnel's logs then look like agent logs without an agent.
- A6 describes the display and matching rules in the "Logging" section of `docs/systems-usage/hub.md` (they hold from A6 on, with or without member crates).

**`RUST_LOG` directives name crates.** `RUST_LOG=residuum::memory=trace` stops matching memory's logs once memory is a crate; the directive becomes `residuum_memory=trace` (or `residuum_net::tunnel=trace` for a module inside a bundle crate). `RUST_LOG` is a developer override (users set `log_level` in `hub/config.toml`, which is unaffected), so this is documented, not detected: the "Logging" section of `docs/systems-usage/hub.md` lists which modules live in which crate and how to write a directive for them, and each extraction chunk adds its crate to that list in the same pull request, so the doc never names a crate that does not exist or misses one that does. F2 adds the same to the debugging section of `CLAUDE.md`/`AGENTS.md`.

## 9. Build info, embedded assets, include paths, generated types

- **The version stays in the root.** `pub const CURRENT_VERSION: &str = env!("RESIDUUM_VERSION")` (`src/update.rs:30`) and `option_env!("RESIDUUM_GIT_COMMIT")` are read only by root code: `update`, `hub::http::lifecycle`, `gateway::web::config`, `commands::bug_report`, and, after §5, `crate::client_context` (moved from `tracing_service`) and the hub building `workbench::SdkContext`. `rustc-env` from a build script applies only to its own package, and the value changes with every commit, so a lower crate reading it would need its own build script and would recompile, with everything above it, on every commit. Nothing in a lower crate may use `env!("RESIDUUM_VERSION")`; one that needs the version takes it as a parameter. (Locally the value goes stale across commits today, because `build.rs` reruns only when `web/dist` changes; that is #679 and independent of the split.)
- **`build.rs` and `rust-embed` stay with the root.** `build.rs` keeps asserting `web/dist/index.html` and setting the version; `#[derive(Embed)] #[folder = "web/dist/"]` (`src/gateway/web/mod.rs:40-53`) stays in the gateway. No separate web-assets crate is needed: only the root embeds `web/dist`, and the lower crates build and test without it, which `cargo test -p <lower crate>`, the isolation check and the `just test-crate` recipe in §11 use.
- **`include_str!`/`include_bytes!` and `CARGO_MANIFEST_DIR` paths keep pointing at the repository-root files**; assets do not move. The depth changes with the move and each one is rewritten by the chunk that moves the file: `src/config/bootstrap.rs:14-20` (`../../assets/…` becomes `../../../assets/…` from `crates/config/src/bootstrap.rs`), `src/memory/{observer,reflector}/prompt.rs` (`../../../assets/…` becomes `../../../../assets/…`), `src/workbench/mod.rs:36` (`../../assets/workbench/sdk.js` becomes `../../../../assets/workbench/sdk.js`), `src/gateway/browser_landing.rs:272` (`../../web/src/styles/tokens.css` becomes `../../../web/src/styles/tokens.css` from `crates/net/src/browser_landing.rs`), and `src/skills/parser.rs:404` (`CARGO_MANIFEST_DIR` joined with `assets/bundled-skills` becomes `../../assets/bundled-skills`). A wrong `include_str!` path fails the build; a wrong `CARGO_MANIFEST_DIR` join fails the skills test that uses it. `src/remote_access/pebble_support.rs:42` includes a sibling file and moves with it.
- **ts-rs export stays in the root.** `tests/ts_export.rs` (its own test target) imports every exported type and writes to `RESIDUUM_TS_EXPORT_DIR`, defaulting to `web/src/lib/generated` relative to the root package, which is still the repository root. Types that move down (`ImageData` in inference; the pairing, remote-access status and `ArtifactSummary` types in net) are imported from their new paths; the generated files must not change, which `scripts/check-generated-types.sh` verifies. ts-rs names output files by type, not crate, so the cross-file imports stay the same. The per-type `export_bindings_*` tests each `#[ts(export)]` generates write to `./bindings` in their own crate's directory; `.gitignore:17` (`bindings/`, no leading slash) already ignores those at any depth.

## 10. Lints and formatting

- `[lints.rust]` and `[lints.clippy]` move verbatim, comments included, to `[workspace.lints.rust]` and `[workspace.lints.clippy]`. The root and every member set `[lints] workspace = true` (`manifest_scan` rule 1).
- Clippy and rustfmt find `clippy.toml` and `rustfmt.toml` by searching up from each crate's directory, so the root files apply to every member. E1 checks this directly: in a scratch commit, an `unwrap()` in a core unit test passes clippy (the test exemption is read), a `TempDir::keep()` in core fails it (`disallowed-methods` is read), and `cargo fmt --all --check` reports a misformatted core file.
- `#[expect(unsafe_code, reason = …)]` sites move with their code: `src/memory/vector_store.rs:83`, `src/mcp/client.rs:359`, the env-var setting in `src/config/resolve/{mod,models,hub}.rs`. `src/daemon.rs:259,309` and `src/notify/macos/bridge.rs:4` stay in the root.
- `pub(in crate::…)` paths in moved code are rewritten for the new crate root (`pub(in crate::config::resolve)` becomes `pub(in crate::resolve)`).
- The pre-commit hook's `dbg!` and `#[allow]` checks read the staged diff, so they cover every crate unchanged.

## 11. Tooling

### Git hooks

- **pre-commit**: `cargo fmt --all` and `cargo deny check` are workspace-wide already; manifests are staged as `*.toml`, which covers `crates/*/Cargo.toml`. `BACKLOG=src/testing/wall_clock_backlog.rs` (`.githooks/pre-commit:189`) stays valid because that file stays in the root. The hook's rename-aware backlog check (`RENAMES`, from `git diff --cached -M --diff-filter=R`) lets a backlog entry follow its file only when the `git mv` and the entry's path change are staged in the same commit and Git detects the move as a rename, so every chunk that moves a file listed in `wall_clock_backlog.rs` makes the move, the path rewrites and the entry change in one commit, with few enough line changes that the file stays a rename. No change to the hook.
- **pre-push**: the hook keeps its structure (each pushed commit checked as pushed, in place or in a temporary worktree; clippy, the selected tests, the source scans and the generated-types check when Rust paths change). Chunk A4 makes the Rust part package-aware, through a new script and one table of rules.

  `scripts/affected-packages.sh` reads changed paths on stdin and prints cargo package arguments. Its modes: `--affected` (the default) prints the packages a change touches plus every package that depends on one of them, directly or not, through normal, dev or build dependencies, as `-p residuum-x -p residuum …`, or `--workspace` when a path touches every package; `--members` prints the member packages among those (never `residuum`, never `--workspace`; empty when none); `--graph` prints the dependency graph, one `package: dependencies…` line each. The packages a path touches:

  | changed path | packages it touches |
  |---|---|
  | `crates/<dir>/**`, for a directory that holds a `Cargo.toml` | `residuum-<dir>` |
  | `src/**`, `tests/**`, `build.rs`, `web/src/lib/generated/**`, `web/app-shell-csp.txt`, `scripts/{check,export}-generated-types.sh`, `.github/workflows/release.yml` | `residuum` |
  | `web/src/styles/tokens.css` | the package whose directory holds `src/browser_landing.rs`: `residuum-net` when `crates/net/src/browser_landing.rs` exists in the checked tree, else `residuum` |
  | root `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `clippy.toml`, `rustfmt.toml`, `deny.toml`, `.cargo/**`, `assets/**`, `tests/fixtures/**`, `.github/workflows/{ci,rust-checks}.yml`, `scripts/affected-packages*`, `scripts/check-crate-isolation.sh`, any other path under `crates/` | every package |
  | anything else | none |

  The script reads the graph from `crates/*/Cargo.toml` with `awk`, without Cargo, so it runs on CI's light runners and in bash 3.2 (the hook's constraint); a member names a workspace dependency only as a `residuum-<x> = { workspace = true … }` line in its `[dependencies]`, `[dev-dependencies]` or `[build-dependencies]` table, and the `manifest_scan` test checks that `--graph` equals the graph read from the manifests with the `toml` crate. It prints, to stderr, one line per package it chose and why (`residuum-memory (changed: crates/memory/src/search.rs)`, `residuum (depends on residuum-memory)`). If it cannot read a manifest it prints a warning and `--workspace` (for `--members`, every member): a failure here checks more, never less. Selecting dependents is sound because dependencies point only downward: a change in the root cannot change a lower crate's build or test result.

  The path mapping is tested by `scripts/affected-packages-test.sh`, in the style of `scripts/ci-failures-test.sh`: it builds a scratch tree with three fake members (`crates/a` with no workspace dependency, `crates/b` depending on `a`, `crates/net` holding `src/browser_landing.rs` and depending on `a`) and a root manifest depending on all three, and for each row of the table above, for both `--affected` and `--members`, and for an unreadable manifest, compares the script's stdout with the expected line. The pre-commit hook runs it when `scripts/affected-packages*` is staged, beside the other script tests. It is a script, so it adds no cargo test.

  The hook's test selection, `plan_rust_tests`, becomes this table, one arm per changed path; the arms of one push combine (filters accumulate, any "every" wins over a filtered selection, the member list is the union):

  | changed path | clippy (`cargo clippy --all-targets --all-features <packages> -- -D warnings`) | root unit tests (`cargo test --quiet --no-fail-fast -p residuum --lib [--bins] -- <filters>`) | member tests (`cargo test --quiet --no-fail-fast <members> --lib --tests`, unfiltered) | integration tests (`-p residuum`) |
  |---|---|---|---|---|
  | `src/lib.rs`, `src/main.rs` | `--affected` | every root unit test, lib and bin (no filter) | — | — |
  | `src/<mod>/**`, `src/<mod>.rs`, a module `src/lib.rs` or `src/main.rs` declares | `--affected` | filter `<mod>::`; `--bins` too when `src/main.rs` declares it | — | — |
  | any other `src/**` path | `--affected` | every root unit test | — | — |
  | `crates/<dir>/**` for a member, other than its `Cargo.toml` | `--affected` (the member and every package above it, the root included) | no filter added | `--members`: the member and every member that depends on it | — |
  | `crates/<dir>/Cargo.toml` | `--affected` | filter `testing::manifest_scan` | `--members` | — |
  | root `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `clippy.toml`, `rustfmt.toml`, `deny.toml`, `.cargo/**`, any `crates/` path outside a member | `--workspace` | filter `testing::manifest_scan` | — | — |
  | `build.rs`, `web/src/lib/generated/**`, `scripts/{check,export}-generated-types.sh`, `scripts/affected-packages*`, `scripts/check-crate-isolation.sh`, `.github/workflows/{ci,rust-checks}.yml` | `--affected` | no filter added | — | — |
  | `assets/**`, `tests/fixtures/**` | `--workspace` | replaced by `cargo test --quiet --no-fail-fast --workspace --lib --bins` (every unit test of every package) | (included in the row before) | every integration test (`--test '*'`) |
  | `tests/it/**` | `--affected` | no filter added | — | `--test it` |
  | `tests/<name>.rs` (still present) | `--affected` | no filter added | — | `--test <name>` |
  | any other `tests/*/*` | `--affected` | no filter added | — | every integration test |
  | `web/src/styles/tokens.css` | `--affected` | every root unit test while `browser_landing.rs` is in the root (as today); no filter added once it is in `crates/net` | `--members` once it is in `crates/net` | — |
  | `web/app-shell-csp.txt`, `.github/workflows/release.yml` | `--affected` | every root unit test (as today) | — | — |
  | no list of changed paths | `--workspace` | replaced by `--workspace --lib --bins` | (included) | every integration test |

  The root unit-test command always carries `testing::source_scan` in its filters, so it runs on every push that runs the Rust checks, even when no arm added a filter (as today). Clippy's packages are the union of each arm's selection (`--workspace` when any arm says so). The generated-types check runs on every push that runs the Rust checks, as today. Two things change from today's hook: a manifest, toolchain or lint-configuration change now adds the `testing::manifest_scan` filter (today it adds no test), and a `crates/**` path, which today's `*)` arm would send to "every root unit test", has its own arms. Tests of root modules that depend on a changed member are not run by the hook, as today's hook does not run the tests of modules that depend on a changed module; clippy compiles them, and CI runs their tests (below).

  `RUST_PATHS_RE` (`.githooks/pre-push:36`) gains `crates` in its first group and the two scripts in its last, and stays byte-identical to the one in `ci.yml`: `^(src|tests|assets|crates)/|^(build\.rs|Cargo\.toml|Cargo\.lock|rust-toolchain\.toml|clippy\.toml|rustfmt\.toml|deny\.toml)$|^web/src/lib/generated/|^web/src/styles/tokens\.css$|^web/app-shell-csp\.txt$|^(scripts/((check|export)-generated-types|affected-packages(-test)?|check-crate-isolation)\.sh|\.github/workflows/(ci|rust-checks|release)\.yml)$`.
- **pre-push temporary worktree at a stable path** (chunk A5). A commit that cannot be checked in place is checked today in a fresh `mktemp -d "${TMPDIR:-/tmp}/residuum-pre-push.XXXXXX"` worktree, building into `target/pre-push`. Cargo fingerprints a workspace member by its path relative to the workspace root and by its files' modification times, not by the tree's absolute path. Checked: a workspace built at one path, copied with modification times preserved to another path and built with the same `CARGO_TARGET_DIR`, recompiles nothing; copied with fresh modification times, every member recompiles. A fresh worktree gives every file a new modification time, so every push from a temporary worktree rebuilds the whole crate (every workspace crate after the split), although `target/pre-push` already holds a build of nearly the same tree. A5 reuses one tree:
  - The tree is `$REPO_ROOT/target/pre-push/tree`, still building into `$REPO_ROOT/target/pre-push`. Deleting the checkout or its `target/` deletes it; `git worktree prune` then drops its registration.
  - First use: `git worktree add --quiet --detach "$TREE" "$sha"`. Later uses: when `$TREE` is listed by `git worktree list --porcelain`, `(cd "$TREE" && git checkout --quiet --force --detach "$sha" && git clean -fdq)`. `git checkout` rewrites only the files that differ between the tree's commit and `$sha`, so every other file keeps its modification time and Cargo keeps its build. `git clean -fdq` (no `-x`) removes untracked files and keeps ignored ones (`web/dist`, the `bindings/` directories). Nothing re-extracts or `-x`-cleans the tree, since that would reset every modification time. When `$TREE` exists but is not a registered worktree, the hook removes it, runs `git worktree prune`, and adds it again.
  - `web/node_modules`: before each use, when the checkout has `web/node_modules` and the tree's `web/package-lock.json` equals the checkout's, the tree's `web/node_modules` is (re)made a symlink to the checkout's (a real directory left by an earlier `npm ci` is removed first); otherwise the symlink is removed and `ensure_web_deps` runs `npm ci` in the tree. The hook's exit trap removes the symlink, as today, and leaves the tree.
  - `web/dist`: the hook writes the commit it last built `web/dist` from to `target/pre-push/tree-dist-commit`, and rebuilds `web/dist` in the tree when that file is missing, when `web/dist/index.html` or `web/dist/sw.js` is missing, or when `git diff --quiet <recorded> "$sha" -- web ':(exclude)web/**/*.md'` reports a difference. Otherwise it reuses the dist, which then matches the pushed commit's web sources.
  - A lock directory, `target/pre-push/tree.lock` (`mkdir`, with the holder's PID in `tree.lock/pid`), guards the tree; the exit trap removes it. When a live process holds it, the hook prints that another push is using the stable tree and checks in today's `mktemp` worktree instead; when the holder is gone, it takes the lock over and says so. The hook also uses the `mktemp` worktree, saying why, when preparing the stable tree fails, and when `$REPO_ROOT/.cargo` exists: Cargo reads `.cargo/config.toml` from every parent directory, so a tree below the checkout would build with the checkout's Cargo configuration as well as its own (the repository has none today).
  - The hook's comment at `.githooks/pre-push:47-53`, and the matching sentence in `CLAUDE.md` and `AGENTS.md` ("Git Hooks"), say sccache shares the dependency builds between in-place and temporary-tree checks. It does not: the hook exports `CARGO_TARGET_DIR`, sccache hashes `CARGO_*` environment values (§12), and an interactive build usually runs with `CARGO_TARGET_DIR` unset, so the two get different cache keys. A5 rewrites all three to say what holds: `target/pre-push` keeps its own builds from push to push, which the stable tree makes reusable.
  - A pushed commit from before A1 has no `[workspace]` table, and the stable tree sits inside the checkout, so it builds only because the checkout's `Cargo.toml` excludes `target` (§4).

  **A5's check.** On a scratch branch off the A5 branch, with an untracked file in the working tree so the hook cannot check in place, and the hook run directly rather than through a push:

  ```bash
  touch untracked-scratch-file                      # keeps the hook out of the in-place path
  run_hook() { printf 'refs/heads/a5-check %s refs/heads/a5-check %s\n' "$(git rev-parse HEAD)" "$(git rev-parse HEAD~1)" | .githooks/pre-push origin unused 2>&1 | tee "$1"; }
  echo '// a5 check 1' >>src/hub/runtime.rs && git commit -am "a5 check: c1" && run_hook /tmp/a5-c1.log   # first use: creates target/pre-push/tree
  echo '// a5 check 2' >>src/hub/runtime.rs && git commit -am "a5 check: c2" && run_hook /tmp/a5-c2.log
  grep -E '^ *(Checking|Compiling) ' /tmp/a5-c2.log  # expect exactly one line: Checking residuum v0.1.0 (...)
  grep -c 'Building web/dist' /tmp/a5-c2.log         # expect 0
  echo '// a5 check' >>web/src/main.ts && git commit -am "a5 check: c3" && run_hook /tmp/a5-c3.log
  grep -c 'Building web/dist' /tmp/a5-c3.log         # expect 1
  rm -rf target/pre-push/tree && run_hook /tmp/a5-c4.log   # recreates the tree; the run passes
  ```

  The C2 log's first line from the hook says it is checking in the stable tree. For the lock: start `run_hook /tmp/a5-l1.log &`, and while it runs, `run_hook /tmp/a5-l2.log`; the second log says another push holds the stable tree and that it checks in a temporary worktree. Afterwards delete the scratch branch, the untracked file and the logs. `grep` lines beginning `Checking` come from clippy, which the hook runs without `--quiet`.

### CI

The Rust checks run through `rust-checks.yml`, which `ci.yml` calls for pull requests and merge queue batches that change Rust paths, `release.yml` calls on every release tag, and `workflow_dispatch` runs by hand. Its jobs are `rust-lint` (fmt, clippy, `cargo deny`), `rust-lint-windows` (clippy for `x86_64-pc-windows-gnu`) and `rust-test` (build test binaries, tests, Pebble tests when `inputs.pebble`, generated types), each checking out shallowly. After A4:

- **`ci.yml`, `changes` job.** It already checks out with `fetch-depth: 0` and computes `changed` (the pull request from its merge base, `PR_BASE_SHA...PR_HEAD_SHA`; the batch from `QUEUE_BASE_SHA..QUEUE_HEAD_SHA`). It gains an output `packages`: `--workspace` for `merge_group`, and for `pull_request` the output of `scripts/affected-packages.sh --affected <<<"$changed"`, whose stderr goes to the step summary; when the script fails the step writes `--workspace` and a warning annotation. `RUST_PATHS_RE` becomes the value given above for the hook, byte for byte (so a pull request that edits only `scripts/affected-packages.sh` or `scripts/check-crate-isolation.sh` runs the Rust checks, including the `--graph` test), and `DEPS_PATHS_RE` gains `|^crates/[^/]+/Cargo\.toml$`, so a member's dependency change runs `cross-compile.yml` in the merge queue as a root one does.
- **`ci.yml`, `rust` job** passes `packages: ${{ needs.changes.outputs.packages }}` and `isolation: ${{ github.event_name == 'merge_group' }}` alongside `pebble`.
- **`rust-checks.yml` inputs.** `packages` (string, default `--workspace`): the cargo package arguments for clippy and the test steps. `isolation` (boolean, default `true`): run the isolation job. Both are declared under `workflow_call` and `workflow_dispatch` with the same defaults, so `release.yml` (which passes only `pebble`) and a manual run check the whole workspace and the isolation, as the merge queue does. Every job that uses `packages` first checks it against `^(--workspace|-p residuum(-[a-z]+)*( -p residuum(-[a-z]+)*)*)$` and fails otherwise, then passes it to cargo as words: `env: PACKAGES: ${{ inputs.packages }}` and `read -ra packages <<<"$PACKAGES"; cargo … "${packages[@]}"` (an unquoted `$PACKAGES` would fail actionlint's shellcheck).
- **What uses it.** `rust-lint`'s clippy step and `rust-lint-windows` run `cargo clippy --all-targets --all-features "${packages[@]}" [--target x86_64-pc-windows-gnu] -- -D warnings`; `rust-test`'s Build test binaries and Tests steps run `cargo test --quiet --no-run "${packages[@]}"` and `cargo test --quiet --no-fail-fast "${packages[@]}"`. Format (`cargo fmt --all --check`), `cargo deny check` and `scripts/check-generated-types.sh` stay whole-workspace on every run. The jobs keep their shallow checkout: the diff is computed in `changes`.
- **Isolation, re-checked in the merge queue and on release.** Cargo unifies features across every package in one build, so in a whole-workspace run a member that compiles only because another package, or a dev-dependency (hyper's `client`, for example), turns on a feature it needs passes. A new job, `rust-isolation` (`if: inputs.isolation`, on `lab-runners-wide`, no web build: no member has a build script), runs `scripts/check-crate-isolation.sh`. For each member directory it runs, as separate cargo invocations so nothing unifies across members, `cargo check --quiet -p residuum-<dir> --lib`, `cargo check --quiet -p residuum-<dir> --all-targets`, and, when the member has a `test-support` feature, `cargo check --quiet -p residuum-<dir> --lib --features test-support`; it runs every check, prints each failure with its command, and exits non-zero if any failed. With no members (before E1) it prints that there is nothing to check. Each extraction chunk runs the same script before its PR.
- **Pebble tests** (merge queue and release): `cargo test --quiet --lib -- --ignored --test-threads=1 pebble system_tests` becomes `cargo test --quiet -p residuum-net --lib -- --ignored --test-threads=1 pebble system_tests` in E6. Every Pebble and system test lives in `remote_access`, so none is dropped.
- **`cross-compile.yml`**: its pull-request dependency detection (`git diff --name-only "$BASE_SHA...$HEAD_SHA" -- Cargo.toml Cargo.lock build.rs rust-toolchain.toml`, line 62) adds `':(glob)crates/*/Cargo.toml'`. Its Windows test run (`cargo test --quiet --all-features`) covers every member through `default-members`.
- **Release builds**: `cargo build --release --target <triple>` builds the one binary with its libraries, at the same path, with the same name. No change.
- **Nightly stress** (`nightly-stress.yml`): unchanged commands (whole workspace through `default-members`); the naming changes are A7's (below).
- **Docker images**: unchanged by the split (`COPY . .` picks up `crates/`). `docker/Dockerfile` does not build from its compose context today, because `.dockerignore` excludes `assets/` (#684), independent of this work.

A pull request therefore lints and tests only the affected packages, the merge queue lints, builds and tests the whole workspace and re-checks each member alone, and release does both. A fault in the package selection can only shorten pull-request feedback; it cannot let an untested change reach `main`.

### Test names, failure reports and test commands across crates

A test path such as `tests::parses` can exist in two crates once there are members. Chunk A7 makes everything that reads test names say which crate a failure is in. With one package nothing it changes has an effect, so it can land any time before E1; E1 then runs the end-to-end check below.

**Names.** A failing test in a member crate is named by its full Rust path, crate name first: `residuum_memory::search::tests::ranks_recent_first`. A failing test in the root package keeps its bare path (`hub::host::tests::…`, as libtest prints it), so the open `Nightly stress: <test>` issues for root tests keep matching. No root module's name starts with `residuum_`, so the two cannot be confused, and the name has no space, so every existing parser that reads a name as one word keeps working. A test binary is a member's when its file name is `residuum_<x>-<16 hex>`: a member's only test target is its library (`manifest_scan` rule 2 forbids `[lib] name`, so the library's target name is the package name with `-` replaced by `_`, and members have no `tests/` directory, §7), while the root's binaries are `residuum-<hash>` (lib and bin) and `it-<hash>`.

The consumers of test names are:

- **`scripts/stress-tests.sh`** collects test binaries from `cargo test --no-run --message-format=json` (every member through `default-members`) and today runs each with `CARGO_MANIFEST_DIR="$PWD"` from the repository root (`:62-64`). `cargo test` runs a test binary from its package's directory, and some tests read paths relative to it, so the script's `jq` emits each artifact's `.executable` and `.manifest_path`, and the script runs each binary from its manifest's directory with `CARGO_MANIFEST_DIR` set to that directory. The per-copy logs keep today's label, `=== <binary file name>` (one word), and today's crash line. The failure list (`:104-113`, today a `sed` over `---- <name> stdout ----` plus an `awk` for crashes) becomes one `awk` pass that tracks the current binary from the `=== ` lines and prints `residuum_<x>::<name>` for a failure in a member binary and `<name>` otherwise. A crash keeps its form, `<count> <binary file name> (exited <N> without naming a failed test)`, for root and member binaries alike, so `scripts/ci-failures-parse.awk:257` (`[^ ]+` for the binary) needs no change, and `scripts/stress-issues.sh:106` turns `residuum_memory-<hash>` into `residuum_memory test binary`, separate from the root's `residuum test binary`. When `STRESS_TESTS_LOG` is set, the "also failed in the Tests step" comparison (`grep -Fxq` against the `---- <name> stdout ----` names in that log, `:127-134`) removes a leading `residuum_[a-z_]+::` from the name first, because `cargo test --quiet` prints no `Running …` line and the Tests log's names carry no crate. It keeps excluding the generated-types export.
- **`scripts/ci-failures-parse.awk`**, the parser behind `just ci-failures` and the nightly issue filer, needs no change: the summary's failure lines (`^ *[0-9]+ [^ ]+$`) and crash lines keep their one-word name.
- **`scripts/ci-failures.sh`** maps a failing test's name to the files that hold it (`module_files`, `:360-385`). For a name whose first segment is `residuum_<x>`, it looks under `crates/<dir>/src/` for the rest of the path, where `<dir>` is `<x>` with `_` replaced by `-`; for a bare name it looks under `src/` and `tests/it/` as today and under every `crates/*/src/`, and lists every file that matches (a bare name can match in several crates; listing all only widens the "touched" flag).
- **`scripts/stress-issues.sh`** titles an issue `Nightly stress: <test>` from the record title, so a member crate's test is filed as `Nightly stress: residuum_memory::search::tests::ranks_recent_first`. No change.
- **Fixtures.** `scripts/ci-failures-fixtures/` gains `rust-stress-members.log` and its `.records`: a stress summary with a member-crate failure (`residuum_core::util::fs::tests::x`), a root failure with the same path below the crate (`util::fs::tests::x`), and a member binary's crash (`residuum_core-0123456789abcdef`). `scripts/ci-failures-test.sh` checks the three are separate records with the right files (`crates/core/src/util/fs.rs` for the first, every `util/fs.rs` match for the second), and `scripts/stress-issues-test.sh` checks the three issue titles (`Nightly stress: residuum_core::util::fs::tests::x`, `Nightly stress: util::fs::tests::x`, `Nightly stress: residuum_core test binary`). The pre-commit hook runs both when those scripts are staged.
- **`nightly-stress.yml`** passes `STRESS_TESTS_LOG` and the logs as today; no change.
- **`CLAUDE.md` and `AGENTS.md`** ("Testing") say a member crate's tests arrive as `Nightly stress: residuum_<crate>::<test>`.

**E1's end-to-end check** (the first time a member exists). In a scratch commit on the E1 branch that is not pushed, add to `crates/core/src/util/fs.rs`'s tests a test `stress_label_check` that fails with `panic!("forced")` and a test `stress_cwd_check` that asserts `std::env::current_dir()` ends with `crates/core` and equals `env!("CARGO_MANIFEST_DIR")`; then:

```bash
STRESS_COPIES=1 STRESS_BURN=0 scripts/stress-tests.sh util::fs::tests ; echo "exit $?"   # expect exit 1
grep -E '^=== residuum_core-[0-9a-f]{16}$' target/stress/1/1.log        # the core binary ran
grep -E 'running [1-9][0-9]* tests' target/stress/1/1.log               # tests matched in it
grep 'residuum_core::util::fs::tests::stress_label_check' target/stress/summary.txt
! grep 'stress_cwd_check' target/stress/summary.txt                      # the cwd test passed
```

then drop the scratch commit.

The other scripts and recipes:

- **`scripts/export-generated-types.sh`** runs `cargo test --quiet -p residuum --test ts_export`.
- **`justfile`**: `clippy`, `cross-clippy` keep their commands (`default-members`). `pebble` becomes `-p residuum-net` in E6. A new recipe `test-crate crate *args` runs `cargo test --quiet --no-fail-fast -p residuum-{{crate}} "$@"` without the `_web-dist` prerequisite. libtest filters are substring matches applied to every binary, so after the split `just test memory::` runs the root tests whose path contains `memory::` and none of the memory crate's tests, whose paths start below the crate (`search::…`). `test` and `stress` keep passing their arguments through unchanged and add two messages, both to stderr, both no-ops until E1 creates a member:
  - **Which arguments are filters.** For `stress`, every argument (they all go to libtest). For `test`, cargo's arguments: before a `--`, an argument is a filter when it does not start with `-` and is not the value of the option before it (the options that take a value: `-p`, `--package`, `--test`, `--bin`, `--example`, `--bench`, `--exclude`, `-F`, `--features`, `--target`, `--target-dir`, `--manifest-path`, `--profile`, `-j`, `--jobs`, `--color`, `--message-format`, `--config`, `-Z`, unless written `--opt=value`); after a `--`, every argument that does not start with `-`.
  - **A note when a filter names a crate.** When a filter's first `::` segment is a member's directory (`crates/<seg>` exists, or `crates/<seg with _ read as ->` does), print `note: <seg> is crate directory crates/<crate> (residuum-<crate>), whose test paths start below the crate; run just test-crate <crate> <rest of the filter> or just test -p residuum-<crate> <rest of the filter>`. When a module inside a member names it (`crates/*/src/<seg>.rs` or `crates/*/src/<seg>/` exists), print `note: <seg> is a module of residuum-<crate>, so its tests are that crate's; run just test-crate <crate> <whole filter> or just test -p residuum-<crate> <whole filter>`, since the crate's test paths start at the module.
  - **A warning when nothing matched.** The recipe keeps cargo's output (`tee` to a temporary file) and, when at least one filter was given, sums the `N` of every `running N tests` line libtest prints (it prints them under `--quiet` too; for `stress`, over `target/stress/*/*.log`). When the sum is 0 it prints `warning: no test matched <filters>` after cargo's own output. The recipe's exit status stays cargo's.
- **`cargo deny`**: no change (§4).

## 12. Caching

**What sccache does with workspace crates** (measured on this machine, sccache 0.18.0, a two-crate workspace built in two directories):

- **It does not cache an incrementally compiled crate.** Under the default dev profile Cargo passes `-C incremental` for workspace crates, and sccache reports them `CannotCache(incremental)`. Today's 260k-line crate is therefore never served from sccache, locally or in CI; only registry dependencies are. It also never caches test harnesses or binaries (`CannotCache(crate-type)` for `--test` and `bin` builds).
- **With `CARGO_INCREMENTAL=0`, it caches a workspace crate's library build, but only at the same absolute path.** The same workspace built in a second directory missed on both workspace crates and on registry crates with build-script output (`serde`, `serde_core`), and hit on the build-script-less registry crates (`proc_macro2`, `quote`, `syn`, `unicode_ident`): 6 hits, 4 misses.
- **`SCCACHE_BASEDIRS` does not change that for Rust.** Conditions of the check: `SCCACHE_BASEDIRS="$R/p1:$R/p2"` (absolute paths, `:`-separated on Unix; `;` and `|` give one garbage entry), set in the environment of `sccache --start-server` (a running server reads it only at start: with only the client's environment set, `--show-stats` prints `Base directories (none)`), confirmed by `sccache --show-stats` printing both under "Base directories"; `CARGO_INCREMENTAL=0`; `CARGO_TARGET_DIR` unset; two identical checkouts. The result is identical with and without the variable: 6 hits, 4 misses, the same crates. The sccache binary's strings describe the feature as stripping base directories "from preprocessor output", the C/C++ path. sccache hashes the values of the `CARGO_*` environment variables rustc inherits: with the same sources at the same path, changing only `CARGO_TARGET_DIR` misses every crate, registry ones included, and setting it back hits every crate. `CARGO_MANIFEST_DIR` (workspace crates) and `OUT_DIR` (build-script crates) are absolute paths in that set, which a base directory does not rewrite; that this is the cause of the misses is inferred, not proved.
- Two further reasons it would not help agent worktrees even if a later sccache normalized Rust paths: `SCCACHE_BASEDIRS=/home/bearf/Projects/project-residuum/residuum` strips only that prefix and leaves `.claude/worktrees/agent-<id>/…`, which differs per worktree, so the list would have to name every worktree root, and the shared per-user server reads it only when it starts; and a cached rlib carries the first worktree's absolute source paths in its debug info and panic locations.

**Re-running the check** (for example after an sccache upgrade). It uses a private server, so it leaves the shared one alone:

```bash
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
```

The check passes when the p2 build shows cache hits for `lowc` and `topc`; on 0.18.0 both miss. The same reading works on a real build: `[residuum_memory]: compile result: cache hit` in the server log. Until the check passes, the decisions below stand. F2 carries this finding and the recipe into the "Workspace layout" section of `CONTRIBUTING.md`.

**CI's cache.** The runners (`lab-runners-fast`, `-wide`, `-light`, Actions Runner Controller scale sets in the cluster's `arc-runners` namespace) run the `github-runner` image, whose environment sets `RUSTC_WRAPPER=sccache` and an S3-compatible store: `SCCACHE_BUCKET=build-cache`, `SCCACHE_ENDPOINT=http://10.0.0.200:7072`, `SCCACHE_REGION=us-east-1`, `SCCACHE_S3_USE_SSL=false`, with the credentials (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`) from a secret on the scale set. This configuration lives with the runner pools in grizzly-platform, not in this repository. The store outlives the runner pods, which are ephemeral (their work directory is an `emptyDir`), so a later job can hit what an earlier one stored. Checkouts are at the fixed path `/home/runner/_work/residuum/residuum`, which is what sccache needs. No job sets `CARGO_TARGET_DIR`, and none may: sccache hashes its value, so a per-job target directory would miss every crate.

Decisions:

- **CI sets `CARGO_INCREMENTAL=0`** for every job that runs cargo on the lab runners (chunk A0: `rust-checks.yml` at workflow level, the `rust-stress` job of `nightly-stress.yml`, and the lab-runner jobs of `cross-compile.yml`). CI checkouts are fresh, so incremental state is never reused there, and with it off sccache can serve every unchanged member's library build from the shared store. The jobs' build summaries already print `sccache --show-stats`, which makes the hit rate visible per run.
- **Local builds keep the default incremental dev profile, with no per-package override.** Every agent worktree has its own path, so sccache cannot share member builds between worktrees even with incremental off. Within one checkout, Cargo's own fingerprints keep unchanged members from rebuilding, and incremental compilation helps the crate being edited, lower crates included. A checkout at a stable path reuses its own target directory from run to run, which is where local reuse comes from.
- **The pre-push temporary worktree gets a stable path** (§11, A5), so its target directory's fingerprints, and its incremental state, stay valid from push to push.
- **No `CARGO_INCREMENTAL` change in the hooks.** Checks in place use the checkout's own target directory; the stable temporary tree uses `target/pre-push` with incremental on.

## 13. When something regresses

The split changes how the code is built, not what it does, so most regressions it can cause are silent ones: something stops being logged, tested, linted or cached, and nothing errors. Each is listed with what a user or developer would see and what catches it.

| regression | what would be seen | what catches it |
|---|---|---|
| A crate's debug and info logs are filtered out | `residuum logs` has no lines from that crate; bug reports' span dumps lack its spans | the filter test over every workspace member (§8, A6) fails in CI and pre-push; `manifest_scan` rule 2 fails for a crate named outside the `residuum-` prefix |
| `residuum logs --module X` matches nothing for a moved module | an empty output | the no-match line printed by `residuum logs`; the matching tests for every row of §8's table |
| A `RUST_LOG=residuum::<module>` directive stops matching | a developer's override has no effect | not detected at runtime; documented with the crate list in `docs/systems-usage/hub.md` (each E-chunk) and `CLAUDE.md` (F2) |
| A test filter written as a module path matches other tests or none | `just test memory::` passes having run unrelated root tests | the `just test` / `just stress` note when a filter names a crate, and the warning when it matched no test (A7, §11) |
| A nested checkout stops building | every cargo command in an agent worktree of a pre-A1 commit, or in the stable pre-push tree checking one, fails with "current package believes it's in a workspace" | `exclude = [".claude", "target"]` in the root `[workspace]` (§4); A1's check runs `cargo metadata` in a nested pre-A1 checkout |
| Tests are lost in a move (a `mod tests;` declaration dropped) | fewer tests, all green | the test-count check every chunk runs (§14) |
| A member skips the workspace lints | lint violations stop failing the build in that crate | `manifest_scan` rule 1 |
| A member compiles only with features another package turns on | `cargo test -p residuum-<x>` fails for a developer although CI was green | the isolation job in the merge queue and on release (`scripts/check-crate-isolation.sh`) |
| A member's test binary has no TLS provider | that crate's HTTP tests panic with "No provider set" | the crate's own tests, in every CI run that selects it |
| The source scan skips a crate, or an exemption names a moved file | a wall-clock wait in that crate goes unflagged | A2's checks: the scanned roots equal the members; every hard-coded path exists |
| Test helpers reach the release build | test code compiled into `residuum` | `manifest_scan` rule 4; `test-support` is never in `[dependencies]` |
| A dev-dependency cycle | a crate compiled twice in tests; types and globals (`SPAN_BUFFER`, sqlite-vec's registration) stop unifying, with confusing test failures | `manifest_scan` rule 5 |
| `cargo test -p` and the workspace run build a crate with different `test-support` features | slower local test runs, nothing else | `manifest_scan` rule 8 |
| The pre-push hook or a pull request selects too few packages | a broken lower-crate test passes pre-push or the PR lane | the hook and the `changes` job print which packages they chose and why; the merge queue tests the whole workspace, so it cannot reach `main`; a selection failure falls back to `--workspace`; `scripts/affected-packages-test.sh` covers every path class, both modes and the fallback; the `--graph` test keeps the script's graph equal to the manifests' |
| Two crates' tests with one path merge in the stress report, or a member binary's crash is dropped | one failure counted twice, filed under the wrong crate, or not filed | A7's fixtures: a member-crate failure, a root failure with the same path and a member binary's crash stay three records with three titles; E1's end-to-end stress check |
| Failover notices stop reaching the user | no "switched to fallback" toast when the primary model fails | the failover tests on a recording sink (inference) and the root test of the `Publisher` impl (B3); a failed publish still logs at `warn` |
| The version shown is wrong | hub status, the web UI's version or the workbench SDK's `residuum.version` disagree with the binary | the existing tests at `src/gateway/web/config.rs:980` and `src/hub/http/tests.rs:1056`, which compare against `CURRENT_VERSION`; for the workbench SDK, B4's new test that the `SdkContext` the hub builds carries `update::CURRENT_VERSION` and `features::FEATURES` (the workbench's own injection tests assert literals passed in, so they cover the threading, not the values) |
| The generated TypeScript types change | the web app's protocol types drift from the Rust types | `scripts/check-generated-types.sh` in pre-push and CI |
| A member's dependency change skips the cross-compile checks | a Windows or aarch64 break merges | `DEPS_PATHS_RE` and the cross-compile pathspec cover `crates/*/Cargo.toml` (A4) |
| sccache stops serving member crates in CI | the build step gets slower | the jobs' sccache stats in the build summary; F3's measurement |
| The stable pre-push tree builds from stale inputs | a push passes on a stale `web/dist` | the recorded dist commit and the `web/` diff (A5); a doubt is resolved by deleting `target/pre-push/tree`, which the next push recreates |

**Recovery and intervention.** Every chunk is one pull request that leaves `main` working, so the work can stop after any chunk, and any chunk can be reverted on its own (a revert pull request of its merge commit): the preparatory chunks are ordinary refactors of the single crate, and reverting an extraction puts the module back in the root without touching the other crates. The orchestrator can pause between chunks to look at the measurements; nothing forces the next chunk to follow. The pre-push hook's stable tree is disposable: deleting `target/pre-push/tree` costs one full build.

**Prevention.** The manifest and source-scan rules are guards against shortcuts agents take (skipping the lints, test code in production, a scan quietly narrowed) and against silent mis-builds, in the same class as the lint regime, so they fail the build. Nothing in this work adds a guard to Residuum's runtime behavior.

## 14. Implementation plan

### How every chunk is verified

The common gate, run before the chunk is handed back:

- `cargo fmt --all --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --quiet --no-fail-fast` (whole workspace), `scripts/check-generated-types.sh`, and `cargo deny check` when a manifest changed.
- **Test count.** The number of tests after the chunk equals the number at the chunk's base plus the delta the chunk's row states. The base is the commit of `main` most recently merged into the branch, counted in a scratch worktree, and counted again after each later merge of `main` into the branch:

  ```bash
  count_tests() { cargo test --workspace -- --list 2>/dev/null | grep -c ': test$'; }
  base="$(git merge-base HEAD origin/main)"
  git worktree add --quiet --detach .claude/worktrees/count-base "$base"
  cp -r web/dist .claude/worktrees/count-base/web/          # build.rs needs a dist; any dist will do for counting
  (cd .claude/worktrees/count-base && CARGO_TARGET_DIR="$OLDPWD/target/count-base" count_tests)   # the base count
  count_tests                                                # the branch count
  git worktree remove --force .claude/worktrees/count-base
  ```

  Tests that move are renamed (their crate-relative path changes), not lost; a module that loses its `#[cfg(test)] mod tests;` declaration shows up here.
- `just stress <filter>` for any test the chunk adds or rewrites (`CLAUDE.md`, Testing).
- **Docs follow the files.** Every `.md` reference to a path the chunk moved or renamed is updated in the chunk: `git grep -nF '<old path>' -- '*.md' ':!docs/archive' ':!docs/design/workspace-split.md' ':!reference-mats'` prints nothing for each moved path. This covers `docs/systems-usage/` (authoritative, with its mirrors under `assets/bundled-skills/residuum-system/references/`), `src/background/README.md`, `CLAUDE.md` and `AGENTS.md` (which change together), `CONTRIBUTING.md` and the per-module `CLAUDE.md` files.
- Push through the pre-push hook; open a PR whose description lists what the chunk made `pub` (§6) and the test-count delta; enqueue it (`gh pr merge <n> --merge --auto`, `CLAUDE.md`, Merging). The merge queue's run, whole workspace plus isolation once A4 has landed, is the final gate. Chunks that change any `Cargo.toml` also run the cross-compile checks in the queue.

Each extraction (E-chunk) also verifies:

- `git diff -M --summary origin/main...HEAD` shows every moved file as a rename, so `git log --follow` and blame keep its history. A move and its path rewrites go in one commit; the rewrites change few lines per file, so rename detection holds (and the pre-commit backlog check sees the rename, §11).
- `cargo tree -p residuum-<x> -e normal,dev --depth 1`: the workspace crates it lists (`residuum-*`) are exactly those §3 and §7 give for the crate (external crates are listed too and are not part of the check).
- `scripts/check-crate-isolation.sh` passes, and `cargo test --quiet -p residuum-<x>` passes from a checkout without `web/dist` (it must not build the root; move `web/dist` aside for the run and back after).
- No stale path into the root, comments and doc links included: `grep -rnE 'crate::(gateway|hub|interfaces|bus|agent|tools|background|a2a|workspace|notify|inbox|checkpoints|pulse|subconscious|actions|daemon|update|features|commands|logging|process|client_context|setup_wizard|diagnostics_dispatch)\b|crate::testing::(bus_wait|source_scan|manifest_scan|wall_clock_backlog)\b|\bresiduum::' crates/<dir>` prints nothing. On the starting tree the only other hits in a moving module are the two test fixture strings `target: "residuum::test"` in `src/tracing_service/otel.rs:179` and `src/tracing_service/sanitize.rs:95`, which E5 rewrites to `"residuum_tracing_service::test"`.
- No stale path to a moved item in the root: for each row of §5 the chunk completes, `grep -rn 'crate::<old module path>::<item>' src tests` prints nothing (for example `grep -rn 'crate::tools::ToolResult' src tests`); the compiler finds the rest.
- The dependency check of §4 for the member and for the root.

### Chunks

Prefixes: M measure, A tooling and preparation that works on the single crate, B preparatory moves inside the single crate, E extractions, F follow-through. Every A and B chunk is a refactor of the single crate that the existing suite fully covers. Every chunk depends on PR #691 having merged.

**What may run at the same time.** Two chunks may be in flight together exactly when neither depends on the other, directly or through the "depends on" column. The column is complete: besides the logical dependencies, it orders every pair of chunks where one edits a file the other creates (`src/testing/manifest_scan.rs`: A1, then A4; `src/logging/`: A3, then A6; `src/testing/log.rs`: B3, then B6; `.githooks/pre-push`'s new code: A4, then A5; `crates/`: E1, then every other E-chunk), and it puts B2's wide rewrite before the other B chunks so they rebase onto it once. Chunks in flight together still meet in existing files (the root `Cargo.toml` dependency lists, `src/lib.rs`, `Cargo.lock`, `source_scan.rs` paths, the hub.md crate list, `CLAUDE.md`/`AGENTS.md` sections); the later branch merges `main` and resolves them, `Cargo.lock` as §4 says.

| chunk | scope | verification beyond the common gate | test delta | depends on |
|---|---|---|---|---|
| **A0** CI cache and CPU visibility | `CARGO_INCREMENTAL: '0'` in the `env` of `rust-checks.yml` (workflow level), the `rust-stress` job of `nightly-stress.yml` and the lab-runner jobs of `cross-compile.yml`, with a comment saying no job may set `CARGO_TARGET_DIR` (§12); in `rust-test`, `scripts/ci-cpu-report.sh start` as the first step after checkout and its `report` appended in the Build summary, as `web-checks.yml` does; the "Called from" line at the top of `scripts/ci-cpu-report.sh` names `rust-checks.yml` too. It lands before M0, so the CI baseline includes its effect and the split is not credited with it. | In the A0 pull request's own run (it changes `rust-checks.yml`, so the Rust checks run): the Workflow Lint job passes; the Rust Tests job's summary page (Actions UI, the run's Summary) shows the "CPU" table and the sccache stats, and the stats no longer list `incremental` among the non-cacheable reasons. | 0 | — |
| **M0** baseline | Run §15's protocol on `main` after A0 and before any other chunk below merges, for every row that exists on that tree (all but 3b); record every number, the conditions, the `--timings` summaries and the CI sccache stats in §15's M0 column and the targets that follow from them (this file). No code change. | §15's protocol is its own check: three runs per row, recorded. | 0 | A0 |
| **A1** workspace manifest | Root `Cargo.toml`: `[workspace]` (resolver 3, no members yet, `exclude = [".claude", "target"]`, §4), `[workspace.package]`, `[workspace.dependencies]` for every external dependency except the target-specific ones, root `[dependencies]`/`[dev-dependencies]` as `{ workspace = true }`, `[workspace.lints]` moved verbatim with `[lints] workspace = true`. Add `src/testing/manifest_scan.rs` (§4 rules 1-8; with no members it checks the root). | The block below the table. | +8 (one test per rule) | M0 |
| **A2** source scan over the workspace | `src/testing/source_scan.rs`: scan `src`, `tests`, and every `crates/*/src` and `crates/*/tests`, with paths relative to the repository root; count `#[cfg(any(test, feature = "test-support"))]` declarations as test code; fail when a hard-coded path or a `wall_clock_backlog.rs` entry names a file that does not exist; assert the scanned roots are exactly the root plus every directory `members` matches. | `cargo test --quiet --lib -- testing::source_scan` passes, and fails in a scratch commit that renames a file a hard-coded path names. | +2 (scanned roots, paths exist) | M0 |
| **A3** util and dispatcher preparation | `tracing_init` and `log_format` to `crate::logging` (`logging::format`); `process` to `crate::process`; the two URL consts to `crate::pulse`; `set_file_mode_600` into `util::fs`; `expand_env_vars` (with tests) into `util`; the diagnostics dispatcher to `crate::diagnostics_dispatch` (§5). Callers rewritten (`commands/*`, `hub/runtime.rs`, interfaces, pulse, tools, actions, a2a, mcp, the dispatcher's callers in tools, agent and gateway). The new modules' visibility per §6. | `grep -rnE 'crate::(config|workspace|a2a|pulse|skills)::' src/util src/diagnostics` and `grep -rn 'util::(tracing_init|log_format|process)' -E src` print nothing. | 0 | M0 |
| **A4** package selection | `scripts/affected-packages.sh`, `scripts/affected-packages-test.sh` and the pre-commit hook running it when it is staged; the `--graph` test in `manifest_scan`; the pre-push composition of §11; `RUST_PATHS_RE` (hook and `ci.yml`, byte-identical); `DEPS_PATHS_RE`; the `changes` job's `packages` output; `rust-checks.yml`'s `packages` and `isolation` inputs, their use in the clippy and test steps, and the `rust-isolation` job with `scripts/check-crate-isolation.sh`; the cross-compile pathspec; `CLAUDE.md` and `AGENTS.md` ("Git Hooks", "What Runs Where") and `CONTRIBUTING.md` updated to describe them. With one package everything selects `residuum`, so behavior is unchanged until E1. | `scripts/affected-packages-test.sh` passes; `diff <(sed -n "s/^RUST_PATHS_RE='\(.*\)'$/\1/p" .githooks/pre-push) <(sed -n "s/^ *RUST_PATHS_RE: '\(.*\)'$/\1/p" .github/workflows/ci.yml)` prints nothing; `scripts/check-crate-isolation.sh` prints that there is nothing to check and exits 0; the A4 pull request's own run shows the `changes` job's chosen packages in its summary. | +1 (graph agreement; the script test is not a cargo test) | A1 |
| **A5** stable pre-push tree | The temporary worktree at `target/pre-push/tree`, reused as §11 describes, with the lock, the `node_modules` and `web/dist` handling and the `mktemp` fallback; the corrected comment and the `CLAUDE.md`/`AGENTS.md` sentence about sccache. | "A5's check" in §11. | 0 | A4 |
| **A6** log targets for crates | `logging::format`: display and `--module` matching for `residuum` and `residuum_*` targets (§8); the no-match line; the filter test over the workspace members; the "Logging" section of `docs/systems-usage/hub.md`. | `cargo test --quiet --lib -- logging::` runs the new tests. | +24 (one per row of §8's display and matching tables, the no-match line, the filter test) | A3 |
| **A7** test names and test commands across crates | `scripts/stress-tests.sh`, `scripts/ci-failures.sh`, the new fixture and the two fixture tests, `scripts/export-generated-types.sh -p residuum`, the `justfile` recipe and messages, and the `CLAUDE.md`/`AGENTS.md` Testing line (§11). | `scripts/ci-failures-test.sh` and `scripts/stress-issues-test.sh` pass with the new fixture; `STRESS_COPIES=1 STRESS_BURN=0 just stress util::fs::tests` passes and `target/stress/1/1.log` has one `=== ` line per test binary; `just test -- no_such_test_name` prints the no-match warning. | 0 (the fixture tests are scripts) | M0 |
| **B2** workspace layout into config | `src/workspace/layout.rs` to `src/config/layout.rs` (private, `WorkspaceLayout` re-exported at config's root); rewrite every reference (the compiler finds them). The test hook keeps `#[cfg(test)]` until E2. Wide but mechanical, so it goes before the other B chunks. | `grep -rn 'workspace::layout' src tests` prints nothing. | 0 | M0 |
| **B1** config-bound moves | `ThinkingConfig`/`ThinkingLevel` and `RetryConfig` into `config`; `completion_options_for_role` into `inference` as a free function (with its tests); `WEB_UI_ENDPOINT` into `config::constants`; `SkillDir`/`SkillSource` into `config`; `SharedToolsPath`/`apply_tools_path` into `config`; `config/wizard.rs` to `crate::setup_wizard`. Every caller imports each item from its new home (§6, rule 7). | `grep -rnE '^[^/]*crate::(inference|interfaces|skills|workspace)::' src/config` prints nothing (the pattern skips comment lines, such as the plain-text mention in `src/config/provider.rs:42`). | 0 | B2 |
| **B3** inference preparation | `NoticeSink` and the `Publisher` impl, the five callers, the failover tests on a recording sink plus the one bus-level test; `ToolError`/`ToolResult` into `inference`, every caller importing them from there; `strip_code_fences` into `inference`; `EventLog`/`LoggedEvent` into `crate::testing::log`. | `grep -rn 'crate::bus\|crate::hub' src/inference` prints nothing. | +1 (the `Publisher` test) | B2 |
| **B4** net preparation | `browser_landing`, `cross_site`, `remote_control_guard` out of `gateway` to top-level `pub(crate)` modules of the single crate (their names in net), callers rewritten from `crate::gateway::…`; `workbench/watcher.rs` to `hub::workbench_watcher`, with `discover_artifacts` and `DiscoveredArtifact` made `pub(crate)`; `workbench::SdkContext`, built by a function the hub's `start_workbench_listener` (`src/hub/runtime.rs:1032`) calls, and a test that this function returns `version == update::CURRENT_VERSION` and `features == features::FEATURES`; the two path constants into `remote_access`. `ArtifactSummary` stays in `gateway::protocol` until E6 (§5). | `grep -rnE 'crate::(gateway|bus|features|update|interfaces|a2a)::' src/{tunnel,remote_access,pairing,workbench,browser_landing.rs,cross_site.rs,remote_control_guard.rs}` prints only the `ArtifactSummary` import in `src/workbench/mod.rs`. | +1 (the `SdkContext` test) | B2 |
| **B5** tracing_service preparation | `tracing_service/client_context.rs` to `crate::client_context`; every caller rewritten (the compiler finds them); the intra-doc link `` [`client_context::gather_for_bug_report`] `` at `src/tracing_service/mod.rs:163` becomes plain text naming the root module, since no grep finds it and CI runs no `cargo doc`. | `grep -rn 'client_context' src/tracing_service` prints only that plain-text mention. | 0 | B2 |
| **B6** testing split | `crate::testing::wait` split into the generic `wait` and `bus_wait` (§5); `util::test_ports` to `crate::testing::ports`; the `reserve_port` re-export in `src/hub/test_support.rs:39` deleted and its users rewritten (§7). | `grep -rn 'test_ports\|test_support::.*reserve_port' src` prints nothing. | 0 | B3 |
| **E1** extract `residuum-core` | `git mv src/util src/time.rs src/diagnostics` into `crates/core/src/`; `testing::{wait, gate, clock, script, log, ports}` into `crates/core/src/testing/` behind `test-support`, and `src/testing/crypto.rs` to `crates/core/src/testing/crypto.rs` with the three `#[path]` includes updated (§7); every `crate::testing::{wait, gate, clock, script, log, ports}` caller rewritten to `residuum_core::testing::…` (§7); `members`/`default-members` added; the facade line for core; source-scan paths and backlog entries for the moved files; the clippy and rustfmt config check of §10; the `test-support` `#[expect]`s of §7; the hub.md crate list. | E1's end-to-end stress check (§11) and the clippy and rustfmt check (§10). | 0 | A1, A2, A3, A4, A6, A7, B6 |
| **E2** extract `residuum-config` | `src/config`, `src/agent_keys` into `crates/config`; `include_str!` depths; `WorkspaceLayout`'s test hook under `any(test, feature = "test-support")` with `tempfile` optional (§7); API per §6. | | 0 | E1, B1, B2 |
| **E3** extract `residuum-skills` | `src/skills` into `crates/skills`; the `CARGO_MANIFEST_DIR` path; `diagnose_skill_md` public. | | 0 | E2 |
| **E4** extract `residuum-inference` | `src/inference` into `crates/inference`; `testing::model` behind `test-support` and its callers rewritten to `crate::inference::testing::model`; the crypto include, `ctor`, `rustls`; `wall_clock_backlog.rs` entries for the moved files updated in the move's commit; API per §6. | | 0 | E2, B3 |
| **E5** extract `residuum-tracing-service` | `src/tracing_service` into `crates/tracing-service`; the crypto include; the two `"residuum::test"` fixture strings (above). | | 0 | E2, B5 |
| **E6** extract `residuum-net` | `src/{tunnel,remote_access,pairing,workbench}` and the three helper modules into `crates/net`; `ArtifactSummary` from `gateway::protocol` into `workbench`, with `tests/ts_export.rs` naming `residuum_net::workbench::ArtifactSummary` (§5); `include_str!` depths; Pebble commands to `-p residuum-net` (`rust-checks.yml`, `justfile`), and the test commands in `docs/systems-usage/secure-tunnel.md` ("Tests") to `cargo test --quiet -p residuum-net remote_access:: tunnel::v2`; `is_hub_level` list; test hooks behind `test-support`; the crypto include; API per §6. The largest API surface: every method and field is decided by §6's rule and listed in the PR. | `scripts/check-generated-types.sh` shows no change in `web/src/lib/generated`. | 0 | E2, B4 |
| **E7** extract `residuum-memory` | `src/memory` into `crates/memory`; `serde(with)` paths; doc links; `include_str!` depths; `test-support`; the crypto include. | §15 row 3b, the first time it can exist: measured as §15 says and recorded in §15's M0 column (marked "measured at E7"). | 0 | E4 |
| **E8** extract `residuum-mcp` | `src/mcp` into `crates/mcp` (with its `CLAUDE.md`); the crypto include. | | 0 | E4 |
| **F1** integration-test waits | `tests/it/support/{until,guarded}.rs` replaced by `residuum_core::testing::wait`; their source-scan exemptions removed. | `grep -rn 'mod until\|mod guarded' tests/it/support` prints nothing. | 0 | E1 |
| **F3** measure | Re-run §15's protocol; fill in the result column; open an `investigate` issue for any target missed, with the `--timings` report attached. | | 0 | E1-E8 |
| **F2** docs | `CLAUDE.md`/`AGENTS.md` (the lint location, `just test-crate`, test filters, `RUST_LOG` crate names, where test helpers live, the workspace layout under Key References), `CONTRIBUTING.md` ("Waiting in tests", the hook and CI descriptions, and a new "Workspace layout" section describing the crates, their dependency rules, the facade, the API policy and §12's sccache finding with its recipe); this document moves to `docs/archive/` with its measured results. The per-module `CLAUDE.md` files (`src/config/CLAUDE.md`, `src/mcp/CLAUDE.md`) move with their modules in E2/E8, and their paths are corrected there. | The final stale-path sweep prints nothing: `git grep -nE 'src/(util|time\.rs|diagnostics|config|agent_keys|skills|inference|tracing_service|memory|mcp|tunnel|remote_access|pairing|workbench|gateway/(browser_landing|cross_site|remote_control_guard)\.rs|workspace/layout\.rs|hub/test_support\.rs|testing/(wait|gate|clock|script|model|crypto)\.rs)\b' -- '*.md' ':!docs/archive' ':!docs/design/workspace-split.md' ':!reference-mats'`. | 0 | F1, F3 |

**A1's check.** `Cargo.lock` and the dependency tree are unchanged, and a checkout of a pre-A1 commit nested inside the A1 checkout still builds, while the same checkout without the `exclude` entry would not:

```bash
git diff --exit-code origin/main -- Cargo.lock
tree_of() { (cd "$1" && cargo tree --workspace -e all --prefix none --format '{p} {f}' | sed 's/ ([^)]*)//' | sort -u); }
git worktree add --quiet --detach .claude/worktrees/a1-nested origin/main     # a pre-A1 commit, nested in the A1 checkout
diff <(tree_of .claude/worktrees/a1-nested) <(tree_of .)                       # prints nothing
(cd .claude/worktrees/a1-nested && cargo metadata --format-version 1 --no-deps >/dev/null && echo nested-ok)
sed -i.bak '/^exclude = \[".claude", "target"\]$/d' Cargo.toml                 # without the entry ...
(cd .claude/worktrees/a1-nested && cargo metadata --format-version 1 --no-deps >/dev/null 2>&1 || echo nested-fails-as-expected)
mv Cargo.toml.bak Cargo.toml
git worktree remove --force .claude/worktrees/a1-nested
```

Integration order: A0; M0; then A1, A2, A3, A7 and B2; then A4, A6 and B1, B3, B4, B5 as their dependencies land; then A5 and B6; E1; E2; E3-E6 in any order; E7 and E8; F1 whenever after E1; F3; F2. Worktrees that were created before A1 merged keep building after the main checkout pulls A1 only through A1's `exclude` entry (§4); once such a worktree merges `main` with A1 in it, it has its own `[workspace]` table and builds on that.

### The extraction procedure (E1-E8)

1. `git mv` the module into `crates/<dir>/src/`; the module's `mod.rs` becomes `lib.rs` (core and net get a new `lib.rs` declaring their modules).
2. Write `crates/<dir>/Cargo.toml` from the member template in §4, with `[features]` and every `[dev-dependencies]` entry from §7's table, and the external dependencies the check in §4 finds.
3. In the moved files, rewrite `crate::<self>::` to `crate::` and `crate::<other lower module>::` to `residuum_<crate>::…`; fix `pub(in …)` paths, `include_str!` depths, `serde(with)` strings, and doc links.
4. In the root `Cargo.toml`: add `residuum-<dir> = { path = "crates/<dir>", version = "0.1.0" }` to `[workspace.dependencies]`, `residuum-<dir> = { workspace = true }` to `[dependencies]`, and, when the crate has `test-support`, `residuum-<dir> = { workspace = true, features = ["test-support"] }` to `[dev-dependencies]` (`manifest_scan` rule 8); the same dependency lines in every member that uses the new crate. In `src/lib.rs`, replace `pub mod <x>;` with the facade line (§6). Remove root dependencies the check in §4 shows the root no longer names.
5. Build the workspace and decide each visibility error by §6's rule; never widen a whole module.
6. Put helpers that root tests need behind `test-support`.
7. Update `source_scan.rs` and `wall_clock_backlog.rs` paths for the moved files, in the commit that moves them.
8. Update the `.md` references to the moved paths and add the crate to the hub.md list (§8).
9. Run the verification above.

## 15. Measuring the result

### Before #691 (context, not the baseline)

Measured 2026-10-15 on thermaltake-tower (i7-5820K, 6 cores / 12 threads), `main` at c83ddf78, which still had 11 integration test binaries, full dev debug info and aws-lc. These predate #691 and A0, were taken with a slightly different protocol (row 1 with an empty sccache), and only say where the starting point came from; the targets below are set against M0.

| measure | value | from the `--timings` report |
|---|---|---|
| cold `cargo test --no-run` | 302s | wall 289.1s. The dependencies finish at about 108s (the aws-lc-sys build script alone 57s, serenity 28.6s on the tail); `residuum "lib" (test)` runs 108.3s to 289.1s (180.8s) and is the critical path, with the `residuum` lib (107.8s) beside it and the 11 integration binaries plus the bin taking 10-25s each after 216s |
| rebuild after `touch src/lib.rs` | 60s | wall 59.2s: `residuum "lib" (test)` 54.8s, the lib 28.8s, then the 11 integration binaries 22-25s each in parallel |
| `cargo clippy --all-targets --all-features` | 141s | — |
| full `cargo test --quiet` run (binaries built) | 45s | — |
| CI "Build test binaries" | 421s | on the CI node (intel-nuc), averaging about 1.6 CPU of the `lab-runners-fast` limit of 16: the step is close to serial, bound by per-core speed |

The two reports are in the main checkout at `target/cargo-timings/cargo-timing-20261015T144827225Z-95b4e3e563a84412.html` (cold) and `cargo-timing-20261015T145316560Z-95b4e3e563a84412.html` (rebuild), until that target directory is cleaned.

### Protocol

M0 runs it on `main` after #691 and A0; F3 runs it after E8; a row that cannot exist on M0's tree is measured first by the chunk that makes it possible (row 3b, by E7), and that value goes in the M0 column marked so. Every run is on thermaltake-tower, in a checkout on the SSD (`/dev/sda5`), with:

- **The machine's jobserver**, as interactive shells get it: `CARGO_MAKEFLAGS` set the way `~/.bashrc` sets it (`-j --jobserver-auth=fifo:${XDG_RUNTIME_DIR:-/run/user/$UID}/build-jobserver.fifo`, exported only while `build-jobserver` runs; a non-interactive shell has to export it itself), and no other build running (`build-jobserver status` shows 0 tokens in use and `pgrep -c rustc` prints 0 before each run).
- **Recorded conditions**, next to the results: the commit, `rustc --version`, `sccache --version`, and the token count from `build-jobserver status` (12 when this was written).
- **Fixed target directories on the SSD**, one per row, `"$PWD/target/measure/<row>"` (absolute, so the value sccache hashes is the same in every run), never `mktemp -d`: `/tmp` is a 16 GB tmpfs, too small for a build of this tree and on different storage from everything else. Each directory takes tens of GB; remove `target/measure/` when the measurement is done.
- **Reports kept.** Each timed cargo run passes `--timings`; right after it, copy the newest `<dir>/cargo-timings/cargo-timing-*.html` to `target/measure/reports/<row>-run<k>.html` (row 1 removes its target directory between runs). The reports' summaries go into the results; the files can go once they are summarized.
- **Three runs per row**, each timed with `/usr/bin/time -f %e`, each with its own edit where the row has one, and the median recorded.

1. **Cold `cargo test --no-run`, no cache.** Cold on purpose: for each run `k`, `rm -rf target/measure/row1`, then `CARGO_BUILD_RUSTC_WRAPPER= RUSTC_WRAPPER= CARGO_TARGET_DIR="$PWD/target/measure/row1" /usr/bin/time -f %e cargo test --quiet --no-run --timings`, then copy the report. The empty wrapper variables bypass sccache (they override the `rustc-wrapper` in `~/.cargo/config.toml`); `sccache --show-stats` shows the same compile-request count before and after, which confirms it. From each report record the wall time, `D` and the workspace phase `wall − D`, where the workspace units are those named `residuum` or `residuum-*` in the report's unit table (the root's build script included) and `D` is the latest end time (start plus duration) of any other unit. The dependency phase is the same before and after the split, so row 1 is judged on the workspace phase.
2. **Rebuild after a root edit.** Warm: one untimed `CARGO_TARGET_DIR="$PWD/target/measure/row2" cargo test --quiet --no-run`; then, three times, `touch src/lib.rs` and a timed `cargo test --quiet --no-run --timings` with the same `CARGO_TARGET_DIR`, sccache on as usual. Record the per-unit times of the root's units (lib, lib test, bin test, `it`, `ts_export`) from each report.
3. **Rebuild after a lower-crate edit.** 3a, warm, in `target/measure/row3a`: as row 2 with `touch src/memory/search.rs` (`crates/memory/src/search.rs` once memory is a crate). 3b, warm, in `target/measure/row3b`, from E7 on: one untimed `cargo test --quiet --no-run -p residuum-memory`, then three times `touch crates/memory/src/search.rs` and a timed `cargo test --quiet --no-run -p residuum-memory --timings`.
4. **Clippy after a root edit.** Warm, in `target/measure/row4`: one untimed `cargo clippy --all-targets --all-features -- -D warnings`, then three times `touch src/lib.rs` and a timed run of the same command.
5. **CI build step.** Three throwaway pull requests, each a branch off `main` with one commit that appends one comment line to one root file (`// measure row 5` to `src/hub/runtime.rs`, `src/gateway/mod.rs` and `src/agent/mod.rs` respectively), opened against `main` and titled `measure: row 5 (<file>)`. They run only the pull request lane; they are never enqueued, and each is closed with `gh pr close <n> --delete-branch` once its run has finished. F3 opens the same three edits. For each run record the "Build test binaries" step's duration (`gh run view <run-id> --json jobs --jq '.jobs[] | select(.name == "Rust / Rust Tests") | .steps[] | select(.name == "Build test binaries") | [.startedAt, .completedAt]'`), the job's average CPU from A0's CPU table in the job summary (runner CPU used ÷ job seconds; it covers the whole job, `npm ci`, the web build and the tests included, not the step alone), and the sccache stats from the same summary. After the split also record which member crates were cache hits.
6. **pre-push.** Warm, checked in place, the hook run directly (no push), with `export CARGO_TARGET_DIR="$PWD/target/measure/row6"` (the hook checks in place into `CARGO_TARGET_DIR` when it is set). Let `P` be the commit of `main` being measured. Make three sibling branches off `P`, `m0-row6-1` to `m0-row6-3`, each with one commit that appends a different comment (`// measure row 6 run <k>`) to the row's file: `src/hub/runtime.rs` for 6a, `src/memory/search.rs` for 6b (`crates/memory/src/search.rs` once memory is a crate). For each `k`, with a clean working tree:

   ```bash
   git switch --quiet --detach "$P"
   cargo clippy --all-targets --all-features -- -D warnings && cargo test --quiet --no-run   # untimed: the parent's state
   git switch --quiet "m0-row6-$k"
   printf 'refs/heads/m0-row6-%s %s refs/heads/m0-row6-%s %s\n' "$k" "$(git rev-parse HEAD)" "$k" "$P" \
     | /usr/bin/time -f %e .githooks/pre-push origin unused
   ```

   Switching back to `P` rewrites the edited file, so each run rebuilds from the parent's state. Delete the branches afterwards.
7. **Test run.** Warm, in `target/measure/row7`: one untimed `cargo test --quiet --no-run`, then three timed `cargo test --quiet --no-fail-fast` with no edit: every test binary in series and every `rustdoc --test` pass (one before the split, nine after).

### Results and targets

The M0 column is filled by M0 (row 3b by E7) and the result column by F3. Targets are relative to M0.

| measure | M0 | result | target | expected because |
|---|---|---|---|---|
| 1. cold `cargo test --no-run`: wall, `D`, workspace phase | | | workspace phase ≤ 90% | the lower crates compile in parallel with each other and the root's harness starts once their metadata exists; the critical path becomes core, config and inference metadata, then the root's 184k lines instead of 260k |
| 2. rebuild, root edit | | | ≤ 85% | only the root's units rebuild, and their front end covers 70% of today's lines; linking each root test binary still includes every lower crate's code, which does not shrink with the split |
| 3a. rebuild, lower-crate edit, whole workspace | | | ≤ 110% | the lower crate and the root both rebuild; incremental compilation limits the root's share |
| 3b. rebuild, lower-crate edit, `-p residuum-memory` | (measured at E7) | | ≤ 25% of M0's 3a | only memory's harness rebuilds, and nothing above memory is in the build |
| 4. clippy, root edit | | | ≤ 80% | only the root is checked again |
| 5. CI build step, root-only PR | | | ≤ 75%, with every member's library a cache hit | `CARGO_INCREMENTAL=0` (in M0 already) makes members cacheable, and package selection leaves out the lower crates' test harnesses; the step is near-serial, so what shrinks is the root units' size, not parallelism |
| 6a. pre-push, root-only push | | | ≤ 85% | clippy checks only the root, and the root's harness is smaller |
| 6b. pre-push, memory-only push | | | ≤ 105% | memory's tests run in their own harness, but the source scans and the generated-types check still build the root's lib, lib harness and `ts_export` |
| 7. test run | | | ≤ 110% | the same tests in eight more test binaries (the members' harnesses, twelve in all) plus eight more empty doctest passes |

A missed target is investigated from the `--timings` reports and the sccache stats before anything is changed. A missed build-time target alone is not a reason to revert an extraction: the crates also give per-crate test runs and an acyclic lower layer that round 2 builds on.

## 16. What round 1 leaves in place

The root still holds about 184k lines, and inside it the cycles the scan found remain: gateway with hub (28 and 17 references) and with interfaces (36 and 25), the runtime group (agent, tools, background, a2a, subconscious with each other), and the bus.

- **The bus** (`src/bus/events.rs`, `registry.rs`) still embeds types from modules above it: background (`SessionInfo`, `SessionState` and the sender consts, `events.rs:435-493,801,805`), interfaces (`FileAttachment`, `MessageOrigin`, `ConversationDirectory`, `teams::ENDPOINT`; `events.rs:12-13`, `registry.rs:7,121-122`; the `WEB_UI_ENDPOINT` import at `registry.rs:8` points at config after round 1), agent (`AutoModeVerdict`, `SessionUsageTotals`; `events.rs:323,844,910`), workspace (`WorkspaceChange`, `WorkspaceResyncReason`; `events.rs:1149,1151`), a2a (`TrackedTask`, `events.rs:743`) and notify (`ExternalChannelConfig/Kind`, `registry.rs:9`). Its references to inference and to `memory::types::Visibility` point downward after round 1. Round 2 starts by moving those payload types into a bus crate that sits on core, config, inference and memory.
- **Above the bus**, workspace, notify, inbox and checkpoints have no upward edges once the bus moves, except `workspace/channel_builder.rs` (9 references into notify) and `workspace/identity.rs:62` (`&TeamLink`).
- **The runtime group** reaches the hub for `TeamLink`, `hub::directory` and an agent-facing slice of `hub::types`.
- **Round 1 adds these root modules**, which round 2 may move again once their dependencies leave the root: `logging`, `process`, `client_context`, `setup_wizard`, `diagnostics_dispatch`, `hub::workbench_watcher`, `testing::bus_wait`, and the `NoticeSink for Publisher` impl in the bus module.
- **Two levers outside round 1 that F3's numbers may point at**: the repository scans (`source_scan`, `manifest_scan`) run inside the root's lib harness, so a push that touches only a lower crate still builds that harness (row 6b); and every root test binary links all of the workspace's code with the default linker, which the split does not shorten (row 2).

Round 2 is planned from F3's measurements: how much of the remaining build time is the root, and which of its parts change most.
