# Workspace split, round 2: the bus, the runtime, the hub and the gateway as their own crates

**Status:** design, not built. The decisions in §2 wait for the owner's choice; nothing in §10 starts before they are made. Round 1, whose shape and rules this builds on, is in [`docs/archive/workspace-split.md`](../archive/workspace-split.md); the layout it produced is the [Workspace layout](../../CONTRIBUTING.md#workspace-layout) section of `CONTRIBUTING.md`.

## 1. Summary

After round 1 the root package `residuum` still holds about 186k lines (343 files under `src/`), and a one-file edit there rebuilds all of it: about 13.5s for `cargo build` and about 19s for `cargo test --no-run` and clippy, of which 64% is the root crate's front end and 7-9% is linking. The lever is the size of the crate an edit lands in and of the crates above it, so round 2 moves everything between the lower crates and the binary into ten new member crates and leaves the root as the binary, the CLI and the wiring.

| crate | directory | holds | ≈ lines | ≈ commits, 30 days |
|---|---|---|---|---|
| `residuum-bus` | `crates/bus` | `bus`, plus the payload types its events carry (moved down from background, interfaces, agent, a2a and workspace), the reload and command channel types, the notice helpers | 5.5k | 63 |
| `residuum-workspace` | `crates/workspace` | `workspace` (minus `channel_builder`), `inbox`, and the inbound-attachment code from `interfaces/attachment.rs` as `inbox::attachment` | 9.9k | 75 |
| `residuum-checkpoints` | `crates/checkpoints` | `checkpoints` | 4.1k | 24 |
| `residuum-notify` | `crates/notify` | `notify` (minus its config types), plus `channel_builder` | 2.9k | 25 |
| `residuum-proactivity` | `crates/proactivity` | `pulse` and `actions` | 6.2k | 38 |
| `residuum-runtime` | `crates/runtime` | `agent`, `tools`, `background`, `a2a`, `subconscious`, a new `team` module (the team router, `TeamLink`, the directory the runtime sees, the agent lifecycle types), `diagnostics_dispatch`, `client_context` | 52k | 271 |
| `residuum-protocol` | `crates/protocol` | the agent WebSocket protocol (`gateway/protocol.rs`) and the conversions from bus events to its frames | 2.6k | 33 |
| `residuum-interfaces` | `crates/interfaces` | `interfaces` (the chat adapters and their shared core), minus the WebSocket subscriber, plus `AdapterSenders` | 14.5k | 81 |
| `residuum-hub` | `crates/hub` | `hub` (minus the team slice), `update`, `features`, the hub config watcher, the agent launch types and the `AgentLauncher` and `HubWebRoutes` traits | 31k | 102 |
| `residuum-gateway` | `crates/gateway` | `gateway` (the event loop, startup, reload, every web route), the WebSocket subscriber from `interfaces`, and the hub-plus-gateway integration tests | 49k | 336 |
| `residuum` (root) | `.` | `main.rs`, `commands/`, `lib.rs` (the facade, `BUILD_INFO` and the `wiring` module that composes the hub with the gateway), `daemon`, `logging`, `setup_wizard`, `testing/` (the repository scans), `build.rs`, the embedded `web/dist` | 11k | 114 |

Line counts include tests. Commit counts are the 671 non-merge commits from 2026-09-16 to 2026-10-16 that touch at most 40 files, counted by which crate each touched file would be in. The window is the September-October burst (745 and 753 non-merge commits in those months), so it says where work is going now.

What it buys, by a rebuild model over those commits (§2 explains it): the mean critical path of the crates a one-file edit rebuilds falls from about 192k lines today to about 113k, and 31% of edits rebuild at most 60k lines, against none today. The gateway, which takes half of all commits, sits at the top: an edit there rebuilds the gateway and the thin root. The root edits rebuild 11k lines. `cargo test -p residuum-<x>` works for every crate except the root without `web/dist`. §12 says how the result is measured and what the targets are.

**Evidence.** Module graphs and item references come from scans of `main` at eeaf2ad1 and 64728997 (#725); line numbers are against 64728997 and are re-located by name in each chunk, since `main` moves. Every claim a boundary depends on was checked against the code; §13 lists what was not.

## 2. Decisions for the user

Each decision below changes how the code is laid out for every future change, so each states what it decides, what it costs and saves later, the options, and a recommendation. The numbers come from one rebuild model:

- **The model.** For each of the 671 recent commits and each file it touches, the crate holding the file and every crate that depends on it rebuild. "Critical path" is the longest dependency chain among the rebuilt crates, summed in lines, because a crate waits for the one below it; it is a proxy for wall time, since round 1's profiling put most of a rebuild in the front end, which scales with lines. "Small edits" are edits that rebuild at most 60k lines in all. It ignores linking and per-crate overhead, which §12 measures.

| crate graph | critical path per file edit | small edits |
|---|---|---|
| today (round 1) | 192k | 0% |
| **recommended (the choices marked below)** | **113k** | **31%** |
| D1 (b): hub above the gateway, the shared services moved below | 120k | 15% |
| D1 (c): hub and gateway one crate | 131k | 5% |
| D2 (b): the hub-level web routes moved into the hub | 111k | 32% |
| D4 (b): Discord, Telegram and Teams as three crates | 113k | 33% |
| D6 (b): the runtime as four crates | 100k | 31% |
| D10 (a): workspace, checkpoints, notify and proactivity as one crate | 116k | 31% |

### D1. Which way the hub and the gateway stack, and how their cycle breaks

**What it decides.** The hub (the process: hosting agents, the hub HTTP app, push, the team) and the gateway (one agent's runtime and every web route) reference each other: the hub starts agents through `gateway::event_loop::start_agent` and mounts gateway web routers (`src/hub/host.rs:33-37,724,902,1753-1789`, `src/hub/http/mod.rs:56-61`, `src/hub/http/process.rs`), and the gateway runs on hub services (`HubServices`, `ActivityTracker`, `TeamChangeFeed`, `AgentChangeFeed`, the workbench watcher). One of the two has to sit above the other.

**Impact.** The gateway is the most edited code in the repository: 336 of 671 commits touch it, against 102 for the hub. Whichever sits on top is rebuilt alone when it is edited; the one below rebuilds both.

**Options.**
- **(a) The gateway on top.** The hub defines `AgentLauncher` (start an agent; build its repair and file routers) and `HubWebRoutes` (the process-level web routes and the web app's static files), the gateway implements them, and the root's `wiring` hands the gateway's implementation to `run_hub`. Most of what crosses is already plain data over lower types (`AgentControl`, `AgentStartInputs`, `AgentCleanup`), so only the agent start, its event loop and four router builders go through the traits. Cost: the hub's integration tests that run real agents (`src/hub/host/tests.rs` and `src/hub/host/tests/`, about 6.7k lines and 123 tests) can't stay in the hub crate, since the hub can't depend on the gateway even for tests, so they move into the gateway crate; the hub's HTTP test harness switches to a stub launcher, and its tests that check a gateway route through the hub's dispatch move with them. The hub can be tested without real agents. Gateway edits rebuild 58k lines; hub edits rebuild 89k.
- **(b) The hub on top, with the services it gives agents moved below the gateway.** A `residuum-hub-services` crate under the gateway holds `services`, `activity`, `push`, the `agent_watch` feed types, `types` and the workbench watcher; the host, HTTP app and runtime stay in `residuum-hub` above the gateway and keep calling it directly, so no trait is added and every hub test stays where it is. Cost: two hub crates split along a line that has to be kept clean by hand, and every gateway edit also rebuilds the hub's 29k lines (and, with D9 (b), runs the hub's tests before each push).
- **(c) One crate for both** (about 77k lines). No cycle work at all; every edit to either rebuilds both.

**Recommendation: (a).** It puts the most edited code where it costs least, and the trait surface is small because the shared types are already data. The test relocation is real work (one Opus-sized chunk, §10) and is done once.

### D2. Where `gateway::web` goes, and whether it splits into hub-level and agent-level routes

**What it decides.** `src/gateway/web/` (20k lines, 27 files, 159 commits, the hottest directory) mixes three kinds of route: the process-level routes the hub mounts once (hub config, cloud, update, tracing, hub and team checkpoints, team files and the workbench, the static web app), the per-agent repair and file routes the hub mounts whether or not the agent runs, and the running agent's routes (sessions, scheduled, inbox, memory, model, A2A status), which the gateway's event loop mounts. Several files serve two kinds (`web/config.rs` has both `api_hub_config_*` and the agent `api_config_*`; `web/mod.rs` builds all three router groups).

**Impact.** By the model the choice is build-neutral (111k against 113k). What differs is how much code moves and how often a web change has to touch two crates.

**Options.**
- **(a) Keep `web` whole in the gateway.** The hub gets the process routes through `HubWebRoutes::process_routes` and the repair and file routers through `AgentLauncher` (D1 (a)); `src/hub/http/process.rs`, the hub-side builder of the process routes, moves into the gateway with them.
- **(b) Move the hub-level and repair routes into the hub.** About 14k lines of the web code move below the gateway, including `web/mod.rs` and `web/config.rs`, which have to be split by file first; an edit to a route used by both kinds then spans two crates.

**Recommendation: (a).** No build gain justifies splitting the busiest files, and the routes stay next to the state types and helpers they share (`ConfigApiState`, the workspace file API used by both the agent and the team scope).

### D3. Where the wire types live (`ServerMessage` and the other ts-exported protocol types)

**What it decides.** `gateway::protocol::ServerMessage` (1.7k lines with its siblings, 18 `#[derive(TS)]` types) is used by the gateway, by the hub (`HubEvent::SessionFrame` wraps one, `src/hub/types.rs:14,432`; the hub WebSocket, `src/hub/http/ws.rs:39`; the session relay through `gateway::sessions::session_event_to_server_message`, `src/hub/http/session_relay.rs:21`) and by the WebSocket subscriber in `interfaces` (`workspace_frame`, used by `src/hub/http/ws.rs:47`). With the gateway on top, the hub needs these from below. Once the bus carries the payload types (D7), `protocol.rs` depends only on bus, inference and memory types.

**Impact.** Build-neutral either way, since only the hub and the gateway depend on it; the difference is where a reader looks for the protocol and which crate a protocol change shows up in.

**Options.**
- **(a) A `residuum-protocol` crate** on bus and workspace, holding `protocol.rs` and the frame conversions the hub needs (`session_event_to_server_message`, `summary_from_live`, `workspace_frame`).
- **(b) Inside the hub crate** (`hub::protocol`), next to the hub's own wire types (`HubSocketFrame`, `HubEvent`). One crate fewer; the agent's WebSocket protocol is then owned by the hub.
- **(c) Inside the bus crate.** Rejected: every crate depends on the bus, so each of the protocol's 33 monthly commits would rebuild everything.

**Recommendation: (a).** The protocol the web app reads is one unit with one home, neither the hub's nor the gateway's, at the cost of one small manifest.

### D4. How the interfaces↔gateway cycle breaks, and whether the chat adapters become their own crates

**What it decides.** `interfaces` references the gateway 44 times in production: the control channel types (`ServerCommand`, `StopRequest`, `ReloadSender`, `ReloadSignal`, 18 references), `gateway::helpers::{publish_notice, publish_error}` (7), `gateway::event_loop::AdapterSenders` (4), `gateway::sessions::stop_session`, `gateway::web::teams_setup::types` (re-exported by `interfaces/teams/setup_types.rs`), and, from `interfaces/websocket/subscriber.rs` alone, the protocol, the file server, the turn journal and post-turn types (15). The gateway constructs every adapter (`DiscordInterface::new` and the others, `src/gateway/event_loop/http.rs:159-227`, `src/gateway/reload.rs:939-996`).

**Impact.** The gateway is edited four times as often as the interfaces (336 against 81 commits), so the interfaces go below it. The adapter question decides whether a Teams edit (44 of the interfaces' commits; Teams is 7.2k lines, 43% of the module) rebuilds the other adapters.

**Options for the cycle.** (a) Move the channel types and notice helpers into the bus, `AdapterSenders` into `interfaces`, `stop_session` into `background` (returning its own error, whose messages the gateway and the `/stop` command show unchanged), the Teams setup types into `interfaces::teams::setup_types` (they are Teams' types; the gateway's setup routes import them), and the WebSocket subscriber into the gateway (it is the gateway's WebSocket fan-out, and nothing else uses it); or (b) put the interfaces above the gateway behind an adapter-factory trait the gateway owns, which puts the less edited code on top.

**Options for the adapters.** (a) One `residuum-interfaces` crate; (b) `residuum-interfaces` (the shared core, 5.6k) plus `residuum-teams`, `residuum-discord`, `residuum-telegram`. Each adapter depends only on the core and the gateway depends on each, so the split is clean, but the model shows no gain on the critical path (113k either way) and 2 points more small edits.

**Recommendation: cycle (a), adapters (a).** One crate now; split an adapter out when a measurement shows its cost.

### D5. Where `TeamLink`, the agent directory and the agent-facing slice of `hub::types` go

**What it decides.** The runtime reaches up into the hub for the team: `TeamLink` (`src/hub/team.rs:335`; used by agent, background, tools and workspace, 14 files), the `TeamRouter` that holds each agent's `AgentMessenger` (`team.rs:171-190`, which makes `hub::team` and `background::messaging` a cycle), `TeamTarget` and `parse_team_address` (tools), `DirectoryHandle` and `AgentDirectory` (`src/hub/directory.rs`; tools' `agent_create`/`agent_delete`, the A2A listener, `StaticAgentDirectory`), and the lifecycle types they return (`AgentSummary`, `AgentState`, `LifecycleError`, `Actor`, `CreateAgentRequest`, `A2aVisibility`, `DeleteOutcome`, `AgentLastError`, `AgentErrorKind`; `src/hub/types.rs:15-256,487`). `AgentDirectory` itself can't move down whole: its `subscribe` returns `HubEvent`, which carries the overview, the team event log and protocol frames.

**Impact.** Decides whether the runtime has any edge into the hub, and how many traits sit between agents and the hub.

**Options.**
- **(a) Move the team slice into the runtime** as a `team` module: the router, `TeamLink`, `TeamTarget`, `parse_team_address`, the lifecycle types, `DirectoryHandle`, and a new `TeamDirectory` trait with the five methods the runtime calls (`list`, `summary`, `agent_a2a_router`, `create`, `delete`). The hub's `AgentDirectory` becomes `AgentDirectory: TeamDirectory` and adds the routers, lifecycle and events; `DirectoryHandle` holds a `Weak<dyn TeamDirectory>`. The A2A listener and `StaticAgentDirectory` (`src/a2a/listener.rs`, `static_directory.rs`) need only `TeamDirectory`, so they and their tests stay in the runtime.
- **(b) Keep `hub::team` in the hub and give background, agent, tools and workspace a trait each for what they ask of the team.** More traits in the most edited runtime files, and tools still needs the lifecycle types from somewhere below.
- **(c) Move `AgentDirectory` and all of `hub::types` into the runtime.** Drags the overview, team events and the protocol into the runtime.

**Recommendation: (a).** The team router is runtime machinery (it delivers through the background messenger), and one narrow trait replaces every runtime edge into the hub. The ts-exported lifecycle types change path (`residuum::team::AgentSummary`), not content.

### D6. How agent↔tools↔background↔a2a breaks: one runtime crate or several

**What it decides.** agent, tools, background, a2a and subconscious (52k lines with the team slice) reference each other densely: the agent owns a `ToolRegistry` and reloads tools in it (`src/agent/core.rs:33,211,246`), and the tests in `core.rs` alone build registries about 40 times; background runs turns through the agent (25 references) and builds each session's `ToolRegistry` from `SubagentToolDeps` (`src/background/subagent.rs:24`, with the tool handles carried in `SpawnContext`, `src/background/types.rs:161-176`); tools call the background messenger, registry and store (7 references) and the A2A client (4); a2a delivers through the background messenger (6).

**Impact.** 271 of 671 commits touch the runtime, and as one crate each such edit rebuilds 157k lines. Split in four (agent < background < a2a < tools), a tools edit (123 commits) rebuilds 73k, and the critical path falls from 113k to 100k.

**Options.**
- **(a) One `residuum-runtime` crate.** Every cycle stays inside it; nothing is redesigned.
- **(b) Four crates.** Needs three changes in the runtime's busiest files: the `Tool` trait and the `ToolRegistry` container move down into `agent` while the registration methods (`register_defaults`, `reload_timezone_tools`, the `exec` dependency injection) become functions in `tools`, and the agent's two tool-reload methods move to their callers; the session's tool registry is built through a factory trait that background owns and tools implements, which also takes the A2A and lifecycle handles out of `SpawnContext`; the agent's tests use test tools instead of the real registry builders.

**Recommendation: (a) in round 2, (b) planned as its own piece of work after §12's measurements.** (a) gets the root small with no redesign; (b) changes how every tool is wired (the main/session registry parity in `src/tools/CLAUDE.md` included) and deserves a design pass of its own. The cost is that (b) later re-cuts the runtime crate, which round 1's "extract each crate once" rule would avoid.

### D7. Where `MessageOrigin`, `FileAttachment` and the other bus payload types go

**What it decides.** The bus's events embed types from six modules above it (`src/bus/events.rs:12-13,230,256,323,435-493,743,801,805,844,910,1149,1151`; `src/bus/registry.rs:8-9,121-122`): `SessionInfo`/`SessionState`/`SessionCategory` and the sender constants (background), `MessageOrigin` with `ConversationContext`/`ConversationKind`, the conversation directory and `FileAttachment` (interfaces), `SessionUsageTotals` and `AutoModeVerdict` (agent), `TrackedTask` (a2a), `WorkspaceChange` (workspace), `ExternalChannelConfig` (notify), and `teams::ENDPOINT`. Every one is data or a constant.

**Options.** (a) Move them into the bus crate, by topic module (`bus::session`, `bus::origin`, `bus::attachment`, …), except `ExternalChannelConfig`/`ExternalChannelKind`, which are the parsed `channels.toml` and go to `residuum-config`, and `teams::ENDPOINT`, which joins `WEB_UI_ENDPOINT` in config; (b) a separate payload crate under the bus, which buys nothing, since the bus and its payloads change together; (c) move the event layer above the modules instead, which is not possible, because checkpoints, workspace, notify and inference's `NoticeSink` impl publish on the bus from below.

**Impact.** The moved items are named in about 100 files, which change their imports mechanically, guided by the compiler. The bus stays the most expensive crate to edit (186k lines rebuild, 63 commits a month), which no option avoids: the event vocabulary is what every producer and consumer shares.

**Recommendation: (a).**

### D8. Which crate embeds `web/dist`, which has the build script, and how the version reaches member crates

**What it decides.** `#[derive(Embed)] #[folder = "web/dist/"]` sits in `src/gateway/web/mod.rs:40-53`; the root's `build.rs` asserts `web/dist/index.html`, watches `web/dist/`, and sets `RESIDUUM_VERSION` and `RESIDUUM_GIT_COMMIT`, rerunning whenever `HEAD` moves (`build.rs:29,57-66`, so on every commit). The version is read by `update::CURRENT_VERSION` (`src/update.rs:30`; read by the hub, the gateway's config route and the CLI), by `client_context.rs:35-36,66-67,155` (used by the hub, the gateway and the `submit_feedback` tool) and by `commands/bug_report.rs:173-174`. `cargo:rustc-env` applies only to the package whose build script sets it, and a crate whose build script reruns on every commit recompiles, with every crate above it, on every commit.

**Options for the assets.** (a) The root keeps the embed and passes `WebAssets::get` (a plain `fn(&str) -> Option<EmbeddedFile>`) to the gateway's launcher, which serves through it: only the root needs `web/dist`, and a web build rebuilds only the 11k-line root; (b) the gateway embeds `web/dist` and gets a build script of its own (the assert and the watch, no version), so `cargo test -p residuum-gateway` needs a web build and a web build rebuilds the gateway; (c) a `residuum-web-assets` crate, whose rebuild still rebuilds the gateway above it.

**Options for the version.** (a) The root alone reads the environment, into `pub const BUILD_INFO: residuum_core::BuildInfo`, and passes it down: `run_hub` takes it, `HubServices` carries it to each agent and its tools, `client_context`'s functions and `update`'s take it as a parameter, as `workbench::SdkContext` already does; (b) a process-wide value set once at startup, read anywhere (fewer signatures change, but member crates' tests see an unset value and the dependency is hidden); (c) any member reading `env!` itself, which would rebuild it and everything above it on every commit.

**Recommendation: assets (a), version (a).** Both keep the property round 1 relied on: member crates build and test without a web build and without anything that changes per commit.

### D9. What the pre-push hook tests when a push touches a member crate

**What it decides.** Today a push touching `crates/<dir>/` runs that member's tests and those of every member that depends on it (`.githooks/pre-push`, `plan_rust_tests`, through `scripts/affected-packages.sh --members`), and never the root's tests beyond the source scans. In round 1 few members had member dependents, so this was cheap. After round 2 every crate above the bus is a member, so a push touching the bus, workspace, config or memory would run nearly the whole test suite before the push.

**Impact.** 556 of the 671 commits touch a crate that has member crates above it (everything but the gateway and the root): under (b) each such push also runs the tests of every crate above, so a runtime push runs the interfaces', hub's and gateway's tests and a bus push nearly the whole suite. The pull request lane runs the dependents' tests either way, and the merge queue runs everything.

**Options.** (a) The hook runs the tests of the member crates the push touches; clippy still covers those crates and every crate above them, and the PR lane and merge queue run the dependents' tests. This is the rule the hook already applies to the root's modules ("tests of root modules that depend on a changed module are not run by the hook"), and the `CLAUDE.md` advice to run dependents' tests by hand when a shared helper changes stays as it is. (b) Keep running the dependents' tests in the hook.

**Recommendation: (a).** The hook stays proportional to the push, and nothing untested reaches `main`. §12 row 6c measures it.

### D10. How granular to go overall

**What it decides.** More, smaller crates parallelize cold builds and narrow what an edit rebuilds; each crate also costs a manifest, a test binary, a `rustdoc --test` pass, three isolation checks in the merge queue, a line in the hub.md crate list, and an entry in the docs.

**Options.** (a) Coarser, six crates: bus, one "services" crate (workspace, checkpoints, notify, proactivity), runtime, interfaces, hub (with the protocol), gateway: critical path 116k. (b) The ten in §1: 113k. (c) Finer, fifteen: (b) plus the runtime in four (D6 (b)) and the adapters in three (D4): about 100k.

**Recommendation: (b).** The lower four are separate because each is its own domain with its own heavy dependencies (gix in checkpoints, the macOS and Windows notification crates in notify), they test alone, and they log under their own names; the gain over (a) is small and the cost is three manifests. The finer cuts each need a redesign (D6) or show no gain yet (D4).

## 3. Goals and constraints

- **The root ends thin.** After round 2 it holds the binary (`main.rs`, `commands/`), the facade and the wiring in `src/lib.rs` and `src/wiring.rs`, `daemon`, `logging`, `setup_wizard`, the repository scans in `src/testing/`, `build.rs`, the embedded `web/dist` and `BUILD_INFO`: about 11k lines.
- **Frequently edited code sits high, stable code low.** The gateway is at the top, the hub and the interfaces below it, the runtime below those; §4's table shows each crate's commits and what an edit to it rebuilds.
- **Round 1's rules hold unchanged.** The workspace lints, `manifest_scan`'s eight rules, the public-API policy and the "decide each widening" rule (`CONTRIBUTING.md`, "Public API"), `test-support` only from `[dev-dependencies]`, no dev-dependency cycles, the facade rules, moved items never re-exported from their old home.
- **Behavior does not change**, except where §8 says so (how `residuum logs` displays the bundle crates' targets and what a `RUST_LOG` directive names), and except the type renames §5 lists, which no user sees.
- **The release binary, its name and the release artifacts are unchanged.**
- **Every chunk leaves `main` working**, and every preparatory chunk is an ordinary refactor of the code where it lies, which the existing suite covers.

## 4. The crate graph

### What each crate holds

Paths are today's. "Whole" means the file or directory moves as is; an item list means only those items move and the file's other items stay.

**`residuum-bus`** (`crates/bus`; `src/bus/mod.rs` becomes `lib.rs`)
- `src/bus/*` whole, with the `NoticeSink for Publisher` impl (`src/bus/handle.rs:201-214`; legal there, the type is local).
- `session` (new module): from `src/background/registry.rs`, `SessionInfo` (`:237`), `SessionState` (`:135`), `SessionCategory` (`:75`, with `from_trigger`), `OWNER_ADDRESS`, `MAIN_ADDRESS`, `MAIN_DEPTH`, `TEAMMATE_SENDER_PREFIX`, `TEAMMATE_SENDER_CATEGORY`, `ARTIFACT_SENDER_PREFIX`, `ARTIFACT_SENDER_CATEGORY`, `artifact_sender_address`, `generate_address`, `generate_run_id` (`:41-68` and their definitions); `src/background/events.rs` whole (`publish_session_event`).
- `origin`: `src/interfaces/types.rs` whole (`MessageOrigin`, `ConversationContext`, `ConversationKind`, `BACKGROUND_ENDPOINT`, `chat_interface_endpoint`).
- `conversations`: `src/interfaces/conversations.rs` whole (`ConversationDirectory`, `ConversationSource`, `KnownConversation`, `Registration`).
- `attachment`: from `src/interfaces/attachment.rs`, `detect_mime_type` (`:24`) and `FileAttachment` with `from_path` (`:33-76`).
- `usage`: `SessionUsageTotals` and its impl (`src/agent/usage.rs:62-110`); the rest of `usage.rs` (`TurnUsage`, `UsageSink`, `MainUsageSink`, load and save) stays in the agent.
- `auto_mode`: `AutoModeDecision`, `AutoModeVerdict` and `AutoModeVerdict::unchecked` (`src/agent/auto_mode.rs:34-60`); `AutoModeGate` stays.
- `remote_task`: `TrackedTask` and its pure impl (`src/a2a/client/tracker.rs:45-130`); `read_open_tasks` and the tracker stay.
- `workspace_change`: `WorkspaceChange`, `WorkspaceChangeKind`, `WorkspaceResyncReason` (`src/workspace/watch/mod.rs:31-65`).
- `control`: `ReloadSignal`, `ReloadSender`, `ReloadReceiver`, `ServerCommand`, `StopRequest` (`src/gateway/types.rs:26-42,95-130`).
- `notice`: `src/gateway/helpers.rs` whole (`publish_notice`, `publish_reload_notice`, `publish_error`); the copies in `src/tools/mod.rs:105` and `src/checkpoints/notice.rs` call it instead.
- `testing` behind `test-support`: `next_event` and `bus_barrier` from `src/testing/bus_wait.rs`.

**`residuum-workspace`** (`crates/workspace`; `src/workspace/mod.rs` becomes `lib.rs`)
- `src/workspace/*` except `channel_builder.rs`; `src/workspace/config.rs:13` (`pub use super::channel_builder::build_external_channels`) goes.
- `inbox`: `src/inbox/` whole.
- `inbox::attachment`: the rest of `src/interfaces/attachment.rs` (`MAX_IMAGE_INLINE_SIZE`, `is_supported_image`, `encode_image_from_file`, `AttachmentInfo`, `SavedAttachment`, `download_attachment`, `save_attachment_bytes`, `format_attachment_line`, `format_failed_attachment_line`, `finalize_attachment`). This ends the `inbox` ↔ `interfaces::attachment` cycle (`src/inbox/mod.rs:13,83,354` against `src/interfaces/attachment.rs:248-261`).

**`residuum-checkpoints`** (`crates/checkpoints`): `src/checkpoints/` whole.

**`residuum-notify`** (`crates/notify`): `src/notify/` except `types.rs`, which goes to `residuum-config` as `channels` (§5); `src/workspace/channel_builder.rs` as `channel_builder`, with `build_external_channels` re-exported at the crate root.

**`residuum-proactivity`** (`crates/proactivity`; a new `lib.rs` declares the two modules): `pulse` (`src/pulse/` whole), `actions` (`src/actions/` whole).

**`residuum-runtime`** (`crates/runtime`; a new `lib.rs` declares the modules)
- `agent`, `tools` (with `TOOLS.md` and `CLAUDE.md`), `background` (with `README.md`), `a2a`, `subconscious`: whole, minus what moved to the bus.
- `team` (new): `src/hub/team.rs` whole (`TeamRouter`, `TeamLink`, `TeamTarget`, `TeamAddressError`, `TeamSendError`, `parse_team_address`); from `src/hub/directory.rs`, `DirectoryHandle` (`AgentFiles` and the rest of the hub's `AgentDirectory` stay in the hub); a new `TeamDirectory` trait (§5); from `src/hub/types.rs`, `AgentState`, `A2aVisibility`, `AgentErrorKind`, `AgentLastError`, `AgentSummary`, `Actor`, `CreateAgentRequest`, `DeleteOutcome`, `LifecycleError`.
- `diagnostics_dispatch` (`src/diagnostics_dispatch.rs`), `client_context` (`src/client_context.rs`, with `BuildInfo` as a parameter).

**`residuum-protocol`** (`crates/protocol`): `src/gateway/protocol.rs` as the crate root; `frames`: `summary_from_live` (`src/gateway/sessions.rs:23`) and `session_event_to_server_message` (`:106`), `workspace_frame` from `src/interfaces/websocket/subscriber.rs:181`.

**`residuum-interfaces`** (`crates/interfaces`): `src/interfaces/` except `websocket/`, `types.rs`, `conversations.rs` and the attachment code; plus `AdapterSenders` (`src/gateway/event_loop/http.rs:16-27`) and the Teams setup types (today `src/gateway/web/teams_setup/types.rs`) as `teams::setup_types`, which `src/interfaces/teams/setup_types.rs` re-exports today.

**`residuum-hub`** (`crates/hub`; `src/hub/mod.rs` becomes `lib.rs`)
- `src/hub/*` except `team.rs`, the moved part of `directory.rs`, the moved types, `http/process.rs`, and the integration tests that run real agents (§7).
- `launch` (new): `AgentStartInputs`, `AgentControl` (with `deliver_to_main`), `AgentCleanup`, `AgentExit` (`src/gateway/event_loop/run_loop.rs:32-170`), a new `StartedAgent`, `agent_span`, and the `AgentLauncher` and `HubWebRoutes` traits (§5).
- `update` (`src/update.rs`), `features` (`src/features.rs`), `config_watcher` (`src/gateway/watcher.rs`), `HubExit` (today `gateway::types::GatewayExit`) and `TermSignal` (`src/gateway/types.rs:45-93`), `CloudStatusResponse` with `current` (`src/gateway/web/cloud.rs:41-112`).

**`residuum-gateway`** (`crates/gateway`; `src/gateway/mod.rs` becomes `lib.rs`)
- `src/gateway/*` except what moved to bus, protocol, interfaces, hub and config; `web/` whole, minus the Teams setup types and `CloudStatusResponse`.
- `websocket`: `src/interfaces/websocket/` whole, minus `workspace_frame`.
- `web::hub_routes`: `src/hub/http/process.rs`, which implements `HubWebRoutes`.
- `launcher` (new): `GatewayLauncher`, implementing `AgentLauncher` and `HubWebRoutes`.
- `hub_tests` (`#[cfg(test)]`): `src/hub/host/tests.rs` and `src/hub/host/tests/`, and the hub HTTP tests whose subject is a gateway route.

**`residuum` (root)**: `src/main.rs`, `src/commands/`, `src/lib.rs`, `src/wiring.rs` (new), `src/daemon.rs`, `src/logging/`, `src/setup_wizard.rs`, `src/testing/` (`source_scan`, `manifest_scan`, `wall_clock_backlog`, the crypto include), `build.rs`, the `WebAssets` embed (moved from `src/gateway/web/mod.rs:40-53` to `src/wiring.rs`), `BUILD_INFO`. `src/process.rs` moves to `residuum-core` (§5).

### Dependencies

Each row lists the workspace crates a crate depends on directly; every crate also depends on `residuum-core` and `residuum-config`, which the column leaves out.

| crate | depends on (workspace, besides core and config) | ≈ lines | commits, 30 days | an edit rebuilds ≈ |
|---|---|---|---|---|
| `residuum-bus` | inference, memory | 5.5k | 63 | 186k |
| `residuum-workspace` | inference, mcp, bus | 9.9k | 75 | 182k |
| `residuum-checkpoints` | bus, workspace | 4.1k | 24 | 161k |
| `residuum-notify` | bus, workspace | 2.9k | 25 | 60k (gateway and root only; see below) |
| `residuum-proactivity` | bus (dev: workspace) | 6.2k | 38 | 163k |
| `residuum-protocol` | inference, memory, bus, workspace | 2.6k | 33 | 91k |
| `residuum-runtime` | skills, inference, tracing-service, memory, mcp, net, bus, workspace, checkpoints, proactivity (dev: notify) | 52k | 271 | 157k |
| `residuum-interfaces` | inference, net, bus, workspace, checkpoints, runtime | 14.5k | 81 | 73k |
| `residuum-hub` | inference, tracing-service, memory, net, bus, workspace, checkpoints, proactivity, runtime, protocol | 31k | 102 | 89k |
| `residuum-gateway` | skills, inference, tracing-service, memory, mcp, net, bus, workspace, checkpoints, notify, proactivity, runtime, protocol, interfaces, hub | 49k | 336 | 58k |
| `residuum` | every member | 11k | 114 | 11k |

The runtime's one use of notify is in a test (`src/background/runtime.rs:2471`, after the test module starts at `:1526`), so notify is only its dev-dependency, and notify's only production dependents are the gateway and the root; X4 and X6 confirm this with `cargo tree`. The extraction of each crate checks its row with `cargo tree -p residuum-<x> -e normal,dev --depth 1` and corrects this table in the same pull request if the compiler disagrees.

```
L8   residuum (root)                                   every member
L7   residuum-gateway                                  hub, interfaces, notify, and everything below
L6   residuum-hub            residuum-interfaces       hub: runtime, protocol; interfaces: runtime
L5   residuum-runtime                                  checkpoints, proactivity, workspace, bus (dev: notify)
L4   residuum-checkpoints  residuum-notify  residuum-protocol  residuum-proactivity
                                                       the first three: workspace, bus; proactivity: bus (dev: workspace)
L3   residuum-workspace                                bus
L2   residuum-bus                                      inference, memory
L0-1 round 1: memory and mcp on inference; inference, skills, tracing-service and net on config; config on core
```

**It is acyclic.** Every edge in the table points to a crate on a lower line of the diagram (the right-hand column names each line's direct workspace dependencies above round 1's crates), and §5 lists, for every reference that points the other way today, the move or trait that removes it. Test-only edges obey the same order: §5's last table lists the tests that would need a higher crate and where they go, and `manifest_scan` rule 5 fails any dev-dependency cycle.

**Where the edits land.** The five crates edited most (gateway 336, runtime 271, root 114, hub 102, interfaces 81 commits) are the top five of the graph; the crates below the runtime have 24 to 75 commits each, the highest being the bus (63) and workspace (75), whose payloads and file APIs every crate above shares. Edits to the gateway or the root, 31% of all file edits in the window, rebuild at most 58k lines; edits to the runtime, 21%, rebuild about 157k (§13).

### Why each boundary is where it is

- **The bus carries data only.** Every type it embeds is data or a constant (§5), so moving them down cuts every bus edge into the root without a trait.
- **workspace carries the inbox.** The inbox's only upward edge is the attachment type, and the attachment code is the inbox's inbound side; together they depend only on the bus.
- **proactivity needs nothing above the bus.** pulse's production code uses only the bus (once the address helpers move there) and `process` (in core); its one workspace use is a test (`src/pulse/types.rs:1128`), so workspace is only its dev-dependency.
- **protocol is beside the runtime.** Once the payloads are in the bus, the protocol names no runtime type, so a protocol change rebuilds only the hub, the gateway and the root.
- **The A2A listener stays in the runtime.** It needs `summary` and `agent_a2a_router` only, which `TeamDirectory` gives, so it, `StaticAgentDirectory`, `listener_tests.rs` and `server_e2e_tests.rs` (1.5k lines driving the A2A server) stay with the A2A code they test.
- **daemon and logging stay in the root.** The hub uses one function of each (`daemon::write_ready_file`, `src/hub/runtime.rs:513`; `logging::global_filter_handle`, `:815`); `run_hub` takes them as inputs, and the CLI, their main user, keeps them.

## 5. The cycles to break, item by item

"Move" means the code moves and every caller is rewritten to its new path; nothing is re-exported from the old one. Counts are references in the repository (production and tests) and the files that hold them, outside the defining file.

### The bus payloads (chunks P1-P3)

| reference | what | users | resolution |
|---|---|---|---|
| `src/bus/events.rs:801,805`, `:435-493` | `SessionInfo`, `SessionState`, `SessionCategory`; the sender and address constants; `artifact_sender_address` | `SessionInfo` 89 refs in 25 files; `SessionState` 128/23; `SessionCategory` 99/25; `MAIN_ADDRESS` 92/19; the rest under 20 files each | move to `bus::session` with `generate_address`, `generate_run_id`, `MAIN_DEPTH` (pulse, subconscious and the interfaces webhook use them from below the runtime) |
| `src/agent/turn.rs:215,264,388` | `background::events::publish_session_event`, the agent's only edge into background | 17/4 | `src/background/events.rs` (72 lines, bus-only) moves to `bus::session` |
| `src/bus/events.rs:13,156,1447` | `interfaces::types::MessageOrigin` (its `teammate_sender` reads `TEAMMATE_SENDER_CATEGORY`, `types.rs:103`) | 53/21; `ConversationKind` 72/19; `ConversationContext` 32/12 | `src/interfaces/types.rs` whole moves to `bus::origin` |
| `src/bus/registry.rs:8,43,77` | `interfaces::conversations::ConversationDirectory` | 5/2; `ConversationSource` 14/6; `KnownConversation` 21/6 | `src/interfaces/conversations.rs` whole moves to `bus::conversations`; its `pub(crate)` items become `pub` |
| `src/bus/events.rs:12,230,256` | `interfaces::attachment::FileAttachment` | 31/10; `detect_mime_type` 7/6 | the two move to `bus::attachment`; the rest of the file moves to `inbox::attachment` |
| `src/bus/events.rs:844,910` | `agent::usage::SessionUsageTotals` | 70/26 | move the struct and impl to `bus::usage` |
| `src/bus/events.rs:323` | `agent::auto_mode::AutoModeVerdict` | 3/2 | move it, `AutoModeDecision` and `unchecked` to `bus::auto_mode` |
| `src/bus/events.rs:743` | `a2a::TrackedTask` | 23/12 | move the struct and its pure impl to `bus::remote_task` |
| `src/bus/events.rs:1149,1151` | `WorkspaceChange`, `WorkspaceResyncReason` (with `WorkspaceChangeKind`) | 35/9, 24/7, 38/8 | move to `bus::workspace_change` |
| `src/bus/registry.rs:9` | `notify::types::{ExternalChannelConfig, ExternalChannelKind}` | 18/5, 34/4 | `src/notify/types.rs` (57 lines) moves to `residuum_config::channels`, re-exported at config's root; it is the parsed `channels.toml`, read by `workspace/config.rs:321` |
| `src/bus/registry.rs:121-122` | `interfaces::teams::ENDPOINT` | 1 | move to config's constants as `TEAMS_ENDPOINT`, beside `WEB_UI_ENDPOINT` |
| doc comments in `src/bus/events.rs:393,401,422,511,584,716,729`, `registry.rs:131` | intra-doc links to background and gateway items | — | rewrite as plain text naming the module; CI runs no `cargo doc` |

### workspace, inbox, notify, proactivity (chunk P7)

| reference | what | resolution |
|---|---|---|
| `src/workspace/identity.rs:62` | `&hub::team::TeamLink` in `with_team_roster`, which calls `team.prompt_block()` | the function takes the roster block (`Option<String>`); its callers pass `team.prompt_block()` |
| `src/workspace/channel_builder.rs:3-5,74-75,111,147,174` | nine `notify` items | the file moves into notify (§4); callers of `workspace::config::build_external_channels` import it from notify |
| `src/workspace/config.rs:11` | `notify::types::{ExternalChannelConfig, ExternalChannelKind}` | they move to config (above) |
| `src/inbox/mod.rs:13,83,354` | `FileAttachment`, `detect_mime_type` | they move to the bus (above) |
| `src/pulse/executor.rs:3`, `src/pulse/script.rs:17` | `background::registry::{MAIN_DEPTH, generate_address, generate_run_id}` | they move to the bus (above) |
| `src/pulse/script.rs:23`, `src/tools/exec.rs:22`, `src/interfaces/teams/{setup_job,atk_runner}.rs` | `crate::process` | `src/process.rs` (662 lines, depends on `util` only) moves into `residuum-core` as `process`, with `nix` as core's `[target.'cfg(unix)'.dependencies]`; the facade line becomes `pub use residuum_core::{diagnostics, process, time, util};` |
| `src/actions/store.rs:332` | `pulse::MIGRATION_GUIDE_URL` | both are in `residuum-proactivity`; it becomes `crate::pulse::MIGRATION_GUIDE_URL` |
| `src/checkpoints/notice.rs:5-7` | its own copy of the notice publisher | calls `bus::notice` |

### The runtime and the hub (chunk P8)

| reference | what | users | resolution |
|---|---|---|---|
| `src/agent/core.rs:61,158`; `src/background/messaging.rs:38,112,157,164`; `src/background/spawn_context.rs:277`; `src/tools/{background,registry}.rs` | `TeamLink` | 36/14 | `src/hub/team.rs` whole moves to `team::router` in the runtime (with `TeamRouter`, whose messenger map closes the `hub::team` ↔ `background::messaging` cycle inside the runtime) |
| `src/tools/message_agent.rs:15` | `TeamTarget`, `parse_team_address` | 3/2, 15/8 | move with the router |
| `src/tools/agent_lifecycle.rs:3,13-15`; `src/a2a/listener.rs:26`; `src/a2a/static_directory.rs:16-20` | `AgentDirectory`, `DirectoryHandle`, and the lifecycle types | `AgentDirectory` 80/24; `AgentState` 200/26; `LifecycleError` 254/17; `AgentSummary` 94/15; `Actor` 98/18; `A2aVisibility` 147/25 | new trait `team::TeamDirectory: Send + Sync` with `list`, `summary`, `agent_a2a_router`, `create`, `delete`, the methods the runtime calls (`team.rs:221,296`, `agent_lifecycle.rs:152-261`, `listener.rs:217,240`); `DirectoryHandle` moves to `team` and holds `Weak<dyn TeamDirectory>`; the hub's trait becomes `pub trait AgentDirectory: TeamDirectory` and keeps the agent, repair and file routers, `agent_files`, `activity`, `stopping`, `live_sessions`, `list_deleted`, `restore`, `start`, `stop`, `restart`, `patch` and `subscribe`; the lifecycle types move to `team::types` |
| `src/a2a/listener.rs:164-296` | `Arc<dyn AgentDirectory>` | — | `Arc<dyn TeamDirectory>`; `src/hub/runtime.rs:282` passes the host, upcast (trait upcasting is stable since Rust 1.86; the toolchain is 1.99.0) |
| `src/a2a/static_directory.rs:200` | `impl AgentDirectory for StaticAgentDirectory` (it refuses everything but `summary` and `agent_a2a_router`, and publishes `HubEvent`s) | 11/5 (a2a 3, hub tests 2) | implements `TeamDirectory` only and drops its `HubEvent` channel and its stopping set. The hub tests that use it as a full directory (`src/hub/relay_agents.rs:188-366`, which needs `subscribe` and `stopping`; `src/hub/inbox.rs:610`) get `StaticHubDirectory` in the hub's `#[cfg(test)]` code: a `StaticAgentDirectory` plus the hub-event channel, with the same `with_agent`/`add_agent`/`remove_agent`/`set_state`/`begin_stopping`/`set_visibility` methods publishing what they publish today |
| `src/hub/types.rs` `NoticeLevel`, `HubEvent`, `AgentPatch`, `RestoreAgentRequest`, `DeletedAgent`, `AgentActivity`, `AgentListResponse`, the socket frames | hub-only | — | stay in the hub |
| `src/hub/activity.rs:201` | `impl agent::ReplyAudience for ActivityTracker` | — | stays: the hub implements a runtime trait, a downward edge |

### The interfaces and the gateway (chunk P5)

| reference | what | users | resolution |
|---|---|---|---|
| `src/interfaces/mod.rs:92-94,129,340,349,392,414,420`; `discord/handler.rs:20,46`; `telegram/handler.rs:20,48`; `teams/mod.rs:35,63` | `ServerCommand`, `StopRequest`, `ReloadSender`, `ReloadSignal` | `ReloadSignal` 88/20; `ServerCommand` 35/11; `StopRequest` 34/8; `ReloadSender` 20/12 | move to `bus::control` (P4) |
| `src/interfaces/outbound.rs:475,504,562`; `discord/mod.rs:155`; `telegram/mod.rs:134`; `teams/mod.rs:171`; `boot_retry.rs:9` | `gateway::helpers::{publish_notice, publish_error}` | 45/16, 8/3 | move to `bus::notice` (P4) |
| `src/interfaces/{discord,telegram,teams}/mod.rs`, `telegram/handler.rs:19` | `gateway::event_loop::AdapterSenders` | 18/8 | move to `interfaces` (`interfaces::AdapterSenders`); its fields are bus, control, `SessionRegistry` and `ConversationDirectory` types, all below |
| `src/interfaces/mod.rs:248` | `gateway::sessions::stop_session` | 5/3 | move the registry check into background as `background::registry::stop_named_session(&SessionRegistry, &str) -> Result<(), StopSessionError>`, whose `Display` texts are today's messages; `gateway::sessions` maps its two variants to `SessionCommandErrorCode::{InvalidRequest, NotLive}` and the `/stop` command prints the `Display` text, so both say exactly what they say today |
| `src/interfaces/teams/setup_types.rs:3,5` | `pub use gateway::web::teams_setup::types::*` | 6/4 | the 19 types move to `interfaces::teams::setup_types` (the file becomes their definition); `src/gateway/web/teams_setup/mod.rs` and `tests/ts_export.rs` import them from there; `pub use web::teams_setup` in `src/gateway/mod.rs:27` goes |
| `src/interfaces/websocket/subscriber.rs:8-10,155-161,269,289,302,392` | the protocol, `FileRegistry`, the turn journal, `post_turn`, `sessions` | 15 references | `src/interfaces/websocket/` moves to `gateway::websocket`; `WsSubscribers` (used by `src/gateway/ws.rs:19` and `tests/it`) is `pub` there |
| `src/gateway/{event_loop/http.rs, reload.rs, ...}` → interfaces | adapter constructors, `WebhookTable`, `dispatch_stop_request` | — | downward edges; unchanged |

### The protocol (chunk P6)

| reference | what | resolution |
|---|---|---|
| `src/gateway/protocol.rs:7-11,179,236,307,538,810` | `SessionUsageTotals`, `SessionCategory`, `SessionState`, `TrackedTask`, `AutoModeVerdict`, `WorkspaceChange`, `PulseOverlap` | all in the bus after P1-P3; `protocol.rs` moves to `src/protocol/mod.rs` (a root module until X7) |
| `src/hub/http/session_relay.rs:21` | `gateway::sessions::session_event_to_server_message` (with `summary_from_live`) | move to `protocol::frames`; `gateway::sessions` keeps the commands and `summary_from_record` (which needs `background::store::RunRecord`) |
| `src/hub/http/ws.rs:47` | `interfaces::websocket::subscriber::workspace_frame` | move to `protocol::frames` |
| `src/hub/types.rs:14,432`; `src/hub/http/ws.rs:39` | `ServerMessage` | path change only |

### The hub and the gateway (chunks P10, P11)

| reference | what | users | resolution |
|---|---|---|---|
| `src/hub/host.rs:33-37,724,898-902` | `start_agent`, `spawn_agent_loop`, `AgentStartInputs`, `AgentControl`, `AgentCleanup`, `AgentExit`, `gateway::types::AgentRuntime` | `start_agent` 8/4; `AgentControl` 7/3 | `AgentStartInputs`, `AgentControl`, `AgentCleanup`, `AgentExit` move to `hub::launch` (their fields are `HubServices`, bus, background, mcp, workspace and axum types, all at or below the hub); new `pub struct StartedAgent { pub control: AgentControl, pub event_loop: Pin<Box<dyn Future<Output = AgentExit> + Send>> }`; new `#[async_trait] pub trait AgentLauncher: Send + Sync { async fn start(&self, inputs: AgentStartInputs) -> Result<StartedAgent, FatalError>; fn repair_router(&self, inputs: AgentRepairInputs) -> Router; fn file_router(&self, inputs: AgentFileInputs) -> Router; }`. The gateway's `start` is today's `start_agent`, with `event_loop` the boxed `run_agent_loop(runtime)`; the host spawns it in the agent's span where it calls `spawn_agent_loop` today (`src/gateway/event_loop/run_loop.rs:1395-1407`). `AgentRuntime` stays private to the gateway |
| `src/hub/host.rs:1735-1794` | `ConfigApiState`, `WorkspaceScope::Agent`, `agent_repair_api_router`, `AgentFilesState`, `agent_files_api_router`, `FileRegistry::for_agent`, `CheckpointAccess::lazy` | — | `AgentRepairInputs` and `AgentFileInputs` (hub, plain data) carry what `repair_router_for` and `file_router_for` read today: the hub directory, the agent's name and directory, its memory directory, the reload sender when it runs, the checkpoint engine (repair) or the checkpoint opener (files), and the team view; the gateway builds the web states, the `FileRegistry` and the routers from them |
| `src/hub/http/mod.rs:56-61,107`; `src/hub/http/process.rs` | re-exports of `ConfigApiState`, `WorkspaceScope`, `agent_repair_router`; the process routes; `web::static_assets()` | — | new `pub trait HubWebRoutes: Send + Sync { fn process_routes(&self, hub: &HubHttpState) -> Router; fn web_app(&self) -> Router; }`; `HubHttpState` carries an `Arc<dyn HubWebRoutes>`; `hub_router` merges `process_routes` where it merges the six `process::*_routes` today and uses `web_app` as its fallback; `process.rs` moves to `gateway::web::hub_routes` (its `super::state::HubHttpState` becomes `residuum_hub::http::HubHttpState`); the re-exports go |
| `src/hub/http/lifecycle.rs:16,198` | `gateway::web::cloud::CloudStatusResponse` | 2/1 | the struct and `current` move to the hub; `gateway::web::cloud` imports them |
| `src/hub/runtime.rs:19`; `commands/serve/foreground.rs:88-130` | `GatewayExit`, `TermSignal` | 14/3, 4/1 | move to the hub; `GatewayExit` is renamed `HubExit`, since `run_hub` returns it |
| `src/hub/runtime.rs:498` | `gateway::watcher::spawn_hub_config_watcher` | 1 | `src/gateway/watcher.rs` (267 lines, uses only `ReloadSignal` and `util`) moves to `hub::config_watcher`; the gateway calls its agent and workspace watchers there |
| `src/hub/runtime.rs:18,70,515,717,774,1731`; `commands/serve/startup_config.rs:28-29` | `gateway::last_known_good` | 18/5 | `src/gateway/last_known_good.rs` (365 lines, uses only config and `util`) moves to `residuum_config::last_known_good`; the CLI's `residuum::gateway::{has_hub_last_known_good, load_hub_last_known_good}` become `residuum::config::last_known_good::hub::{exists, load}` |
| `src/hub/http/dispatch.rs:234`; `src/hub/agent_watch/watcher.rs:23` | `agent_span`, `AgentControl` | 13/5 | `agent_span` moves to `hub::launch` |
| `src/hub/runtime.rs:993-994,1022-1025`; `src/hub/http/lifecycle.rs:204`; `src/gateway/web/config.rs:153-154` | `update::CURRENT_VERSION`, `features::FEATURES` | 10/5, 4/2 | `update.rs` and `features.rs` move into the hub; the version comes from `BuildInfo` (below) |
| `src/hub/runtime.rs:513,815` | `daemon::write_ready_file`, `logging::global_filter_handle` | — | `run_hub` takes a `HubInputs` with `on_ready: Box<dyn FnOnce(&Path) + Send>` and `log_filter: Option<&'static dyn FilterReload>`; the `FilterReload` trait (`src/logging/mod.rs`) moves to `residuum_config` beside `LogLevel`, which it takes; `logging` keeps the global handle and implements it |
| `src/gateway/*` → `hub` | `HubServices`, `ActivityTracker`, `TeamChangeFeed`, `Actor`, `DirectoryHandle`, `NoticeLevel`, `AgentChangeFeed`, `spawn_workbench_watcher` | — | downward edges; unchanged (the `pub(crate)` ones become `pub`, decided by §6's rule) |

The host's integration tests (`src/hub/host/tests.rs:599` builds a `FileRegistry`) and the hub HTTP test harness (`src/hub/http/tests.rs:24-33,173-300`) use the gateway in tests; §7 says where those tests go.

### The version and the web assets (chunk P9, part of P11)

| reference | what | resolution |
|---|---|---|
| `src/update.rs:30` and its readers; `src/client_context.rs:35-36,66-67,155`; `src/commands/bug_report.rs:173-174` | `env!("RESIDUUM_VERSION")`, `option_env!("RESIDUUM_GIT_COMMIT")` | new `residuum_core::BuildInfo { pub version: &'static str, pub commit: Option<&'static str> }` (`Copy`); `src/lib.rs` defines `pub const BUILD_INFO: BuildInfo` from the two variables, and nothing else in the workspace reads them. `run_hub` takes it in `HubInputs`; `HubServices` carries it; `client_context::gather_for_*` and `update`'s functions that compare against the running version take it or its `version`; the session and main tool registries get it through `SubagentToolDeps` and `init_tool_registry` for `submit_feedback`; the hub builds `workbench::SdkContext` from it. Tests that compare with `CURRENT_VERSION` (`src/update.rs:946`, `src/gateway/web/config.rs:979`, `src/hub/http/tests.rs:1058`, `src/hub/runtime.rs:1063`) pass a literal `BuildInfo` and assert it comes back |
| `src/gateway/web/mod.rs:40-53`; `src/gateway/web/assets.rs:34,138` | `WebAssets` (the embed) and its one production use, `WebAssets::get(path)` | the embed moves to `src/wiring.rs`; `static_assets` takes `get: fn(&str) -> Option<rust_embed::EmbeddedFile>`, which `GatewayLauncher::new` receives from the root. The `assets.rs` tests that read the real build (`:285-298`, `:589`, `:709-733`) move to the root's `wiring` tests; the rest use a fixture function |

### Tests that would need a crate above their own

A dev-dependency on a crate above is never added (it would be a cycle, `manifest_scan` rule 5). Such a test is a check of the higher crate's reader against the lower crate's output, so it moves to the higher crate:

| test | uses | goes to |
|---|---|---|
| `src/workspace/team.rs:522-…` (the heartbeat-template tests) | `pulse::types::HeartbeatConfig` | `pulse::types` tests, next to `:1128`, which already parses `first_agent_heartbeat` |
| `src/workspace/bootstrap.rs:1190-1201` | `a2a::AgentCardFile::load` | `a2a::card` tests |
| the hub tests that run real agents | the gateway's launcher | `gateway::hub_tests` (§7) |

The compiler finds any others when a crate is extracted (`cargo test -p residuum-<x>` fails to build them), and they follow the same rule.

After these moves no production or test reference points up the graph in §4.

## 6. Visibility and API policy

Round 1's policy applies as it stands in `CONTRIBUTING.md` ("Public API", "The root facade"), including the five-step rule for each item the compiler reports as private to the crate that needs it, and each extraction's pull request lists every item it made `pub` with its kind, the step that applied and its callers outside the crate. What round 2 adds:

- **`unused_pub` starts to apply.** `testing::source_scan::unused_pub` checks only member crates (`src/testing/source_scan/unused_pub.rs:106-113`), so every `pub` item of the code that moves becomes a candidate: an item named nowhere in the workspace outside its own declaration, its own crate's `#[cfg(test)]` code and its own `use` lines fails the scan. Each extraction runs the scan and, for each report, deletes the item, narrows it to `pub(crate)`, or moves a test-only helper into `#[cfg(test)]` code. A use in the crate's own production code counts, so this reports only dead or test-only items.
- **Existing `pub` stays `pub`** (rule 5) where `commands/`, `tests/it` or `tests/ts_export.rs` use it. Items that are `pub` today with no user outside their crate are what `unused_pub` reports and get narrowed.
- **The top two crates' API is what the root uses.** `residuum-gateway` exports `GatewayLauncher`, what `commands/` names, and what `tests/it` and `ts_export` name; `event_loop`, `startup`, `reload` and the other `pub(crate)` modules stay private. `residuum-hub` exports `run_hub`, `HubInputs`, `HubExit`, the launch types and traits, and the types the gateway uses (`HubServices`, `ActivityTracker`, `TeamChangeFeed`, `AgentChangeFeed`, `HubHttpState`, `NoticeLevel`, the workbench watcher), which are `pub(crate)` today and are decided by the rule.
- **The runtime keeps its module namespaces.** `agent`, `tools`, `background`, `a2a`, `subconscious` and `team` are `pub` modules of `residuum-runtime`, as they are `pub mod` of the root today, and inside them the items used across the boundary are `pub`.
- **Moved test helpers are `#[cfg(any(test, feature = "test-support"))] pub`** (§7), and clippy treats that code as production code, so an `unwrap` there carries a scoped `#[expect(clippy::…, reason = "test-support helper: …")]`, as in round 1.

The facade after X10 (round 1's lines unchanged except core's):

```rust
pub use residuum_config as config;
pub use residuum_config::agent_keys;
pub use residuum_core::{diagnostics, process, time, util};
// ... round 1's other lines ...
pub use residuum_bus as bus;
pub use residuum_workspace as workspace;
pub use residuum_workspace::inbox;
pub use residuum_checkpoints as checkpoints;
pub use residuum_notify as notify;
pub use residuum_proactivity::{actions, pulse};
pub use residuum_runtime::{a2a, agent, background, client_context, diagnostics_dispatch, subconscious, team, tools};
pub use residuum_protocol as protocol;
pub use residuum_interfaces as interfaces;
pub use residuum_hub as hub;
pub use residuum_hub::update;
pub use residuum_gateway as gateway;

pub mod daemon;
pub mod logging;
pub mod setup_wizard;
pub mod wiring;
#[cfg(test)]
pub(crate) mod testing;

pub const BUILD_INFO: residuum_core::BuildInfo = /* env!("RESIDUUM_VERSION"), option_env!("RESIDUUM_GIT_COMMIT") */;
```

`features` was `pub(crate)` and is reached as `residuum_hub::features` by the gateway; the root needs no alias for it. Each alias is `pub` because `commands/`, `tests/it` or `ts_export` names the module through `residuum::…`; an alias nothing outside the root names is `pub(crate)`, as round 1's net helpers are.

## 7. Test support

The rules are round 1's (`CONTRIBUTING.md`, "Dependency rules"): a crate's own tests use its `#[cfg(test)]` code; helpers other crates' tests use sit in the lowest crate that can hold them, behind `test-support`, turned on only from `[dev-dependencies]`; every package turns on `test-support` for each workspace dependency that has it (`manifest_scan` rule 8); external crates that `test-support` code needs are declared twice.

| helper | today | after | used by |
|---|---|---|---|
| `next_event`, `bus_barrier` | `src/testing/bus_wait.rs:25,41` (`crate::testing::bus_wait`) | `residuum_bus::testing` (feature `test-support`) | background, agent, interfaces, gateway, a2a, tools, bus, hub tests (21 files) |
| `impl Recv for AgentChangeReceiver` | `src/testing/bus_wait.rs:9-18` | `residuum-hub`, `#[cfg(any(test, feature = "test-support"))]` beside `AgentChangeReceiver` (the trait is core's, the type the hub's) | hub and gateway tests |
| `mount_reply`, `write_agent` | `src/hub/test_support.rs` | `residuum_hub::testing` (`test-support`) | hub tests, `src/gateway/startup/tools.rs:628`, `gateway::hub_tests` |
| `HubServices::for_tests`, the `AgentHost` and `build_app` entry points the host tests use | `#[cfg(test)]` or `pub(crate)` in `src/hub/` | `test-support` in the hub, each decided by the rule in §6 | `gateway::hub_tests`, gateway startup tests |
| `StubLauncher`, `StaticHubDirectory` | new | hub, `#[cfg(test)]` | the hub's HTTP and relay tests |
| `team_namespace_tests::Hub` | `src/tools/team_namespace_tests.rs` (`#[cfg(test)]`) | `residuum_runtime::testing::team_namespace` (`test-support`) | tools tests, `src/gateway/web/workbench.rs:436`, `src/gateway/web/workspace_team_tests.rs:19` |
| `pulse::script::tests::env` | `src/pulse/script.rs` tests | `residuum_proactivity::pulse::testing::script_env` (`test-support`) | `src/gateway/event_loop/pulse.rs:517` |
| `background::subagent::test_memory_extras` and the other runtime helpers gateway tests use | `#[cfg(test)]` | runtime `test-support` as the compiler reports them | gateway tests |

**The integration tests that run real agents move to the gateway.** `src/hub/host/tests.rs` and `src/hub/host/tests/` (51 + 72 tests) build a hub over temp agent directories and run real agents against mock model servers, so they need the gateway's launcher. They move to `crates/gateway/src/hub_tests/` as `#[cfg(test)]` modules of the gateway, where both crates are available, and name the hub through its public and `test-support` API. The hub's HTTP harness (`src/hub/http/tests.rs`, 74 tests, with 115 more in `tests/`) builds its fake directory's agent routers from real gateway routers (`:173-300`); it switches to `StubLauncher`, whose routers answer fixed routes, and the tests whose subject is a gateway route served through the hub's dispatch (the repair, file and agent config routes) move to `gateway::hub_tests` with the host tests. P11c lists each test it moves; the test-count check (§10) catches a lost one.

**The repository scans stay in the root** (`src/testing/{source_scan,manifest_scan,wall_clock_backlog}.rs` and the crypto include). The root's lib harness becomes small, so the scans that run on every Rust push no longer build a 186k-line harness.

**The TLS provider.** Each new crate whose code builds HTTP clients includes `crates/core/src/testing/crypto.rs` by `#[path]` under `#[cfg(test)]`, dev-depends on `ctor` and `rustls`, and lists them in `[package.metadata.cargo-shear] ignored`: workspace (attachment download), notify (ntfy and webhook), runtime, interfaces, hub, gateway. A crate that misses it fails its own HTTP tests with reqwest's "No provider set".

**`tests/it` and `tests/ts_export.rs`** stay in the root and link every crate. Their `residuum::…` paths resolve through the facade; the items that changed module are rewritten (`residuum::protocol::…`, `residuum::interfaces::teams::setup_types::…`, `residuum::team::…`, `residuum::gateway::websocket::WsSubscribers`, `residuum::bus::…` for the payloads). The generated files must not change.

## 8. Logging targets, assets, generated types, build script

**Log targets.** A crate's events log under its library name, so `residuum::gateway::event_loop` becomes `residuum_gateway::event_loop`. The default filter (`residuum=debug,warn`) matches every `residuum_*` target by prefix, and A6's filter test, which reads the member list, covers the new crates without a change. Under round 1's display rule (`residuum_<x>::rest` shows as `<x>::rest`), the single-module crates (bus, checkpoints, notify, protocol, interfaces, hub, gateway, and workspace for its own modules) display exactly as today; the bundle crates show their crate name: `runtime::agent::turn`, `runtime::tools::exec`, `proactivity::pulse::scheduler`, `workspace::inbox`, `core::process`. `residuum logs --module agent` still matches `residuum_runtime::agent::…` through the module form. This display change is visible to anyone reading logs, and comes from D6 (a): four runtime crates would display as today.

- **`RUST_LOG` directives name crates.** `RUST_LOG=residuum::hub=trace` becomes `residuum_hub=trace`, and `residuum::agent=trace` becomes `residuum_runtime::agent=trace`. Each extraction adds its crate, and the modules it holds, to the crate list in the "Logging" section of `docs/systems-usage/hub.md` (today its last line says `residuum`: "every other module"), and F1 updates the `RUST_LOG` paragraph of `CLAUDE.md` and `AGENTS.md`.
- **`is_hub_level`** (`src/hub/host/tests.rs:1322-1331`) lists `residuum::hub`, `residuum_net::tunnel`, `residuum::update`, `residuum::daemon`. X9 changes it to `residuum_hub` (which covers `update`), `residuum_net::tunnel`, `residuum::daemon`, in the file's new place under `gateway::hub_tests`. If it is missed, the test fails, because the hub's logs then look like agent logs without an agent.
- The test fixtures that name `residuum::` targets (`src/logging/format.rs` tests, `src/commands/logs.rs:281`) stay in the root and test the rule, not a module's location.

**`include_str!`, `include_bytes!` and path reads.** A file that moves from `src/<m>/` to `crates/<dir>/src/<m>/` gains one `../` per level: `src/workspace/team.rs:13-33` and `src/workspace/bootstrap.rs:10-179` (`../../assets/…` becomes `../../../assets/…` from `crates/workspace/src/`), `src/subconscious/prompt.rs:11`, `src/interfaces/teams/atk.rs:40-56` and its test includes of `tests/fixtures/*.schema.json` (`:1083,1090`), `src/gateway/web/assets.rs:59` (`web/app-shell-csp.txt`), `src/update.rs:977` (`.github/workflows/release.yml`, a test). `src/interfaces/teams/auth.rs:301` includes `testdata/` beside it, which moves with it. A wrong path fails the build.

**Generated types.** `ts-rs` becomes a dependency of each crate with `#[derive(TS)]`: bus, workspace (`InboxAttachment`), checkpoints, runtime (the team types), protocol, interfaces (the Teams setup types), hub. `tests/ts_export.rs` imports each exported type from its new path, and `scripts/check-generated-types.sh` must show no change in `web/src/lib/generated`. The `export_bindings_*` tests write to `./bindings` in their own crate's directory, which `.gitignore:17` ignores at any depth.

**The build script stays the root's**, unchanged: it asserts `web/dist/index.html`, watches `web/dist/` and `HEAD`, and sets the two version variables. No member crate gets a build script.

**Target-specific dependencies move with their code.** `nix` goes to core's `cfg(unix)` table (for `process`) and stays in the root's (for `daemon` and `commands`); `windows-sys` stays the root's (`daemon`); `objc2`, `objc2-foundation`, `objc2-user-notifications`, `block2` and `tauri-winrt-notification` go to notify. `CONTRIBUTING.md`'s rule "the target-specific dependencies are the root's alone" becomes "a target-specific dependency is declared by the crate whose code uses it, in that crate's target table, and is not in `[workspace.dependencies]`". The Windows clippy job and the merge queue's cross-compile run check them.

## 9. Tooling

Most of the machinery round 1 built reads the manifests and `crates/*`, so a new member is picked up with no change: the source scan's roots (`scan_roots`, from the root manifest's `members`), `manifest_scan`, `scripts/check-crate-isolation.sh`, `scripts/affected-packages.sh`'s graph, `scripts/ci-failures.sh`'s `residuum_<x>::` mapping, `scripts/stress-tests.sh`, `scripts/test-filters.sh` (the `just test` notes), `nightly-stress.yml`, `cross-compile.yml`'s `crates/*/Cargo.toml` pathspec, `RUST_PATHS_RE`'s `crates/` prefix and the Docker builds. What does change:

- **Files the tests read by path** (chunk T1). Today `web/app-shell-csp.txt` and `.github/workflows/release.yml` are classified to `residuum` in `scripts/affected-packages.sh` (`classify_path`, `:158-160`) and send the pre-push hook to "every root unit test" (`.githooks/pre-push`, the `*)` arm, `:503-512`). After X10 the CSP is read by `crates/gateway/src/web/assets.rs` and after X9 the workflow by `crates/hub/src/update.rs`. T1 makes each path classify to the package whose source reads it: `residuum-gateway` when `crates/gateway/src/web/assets.rs` exists, `residuum-hub` when `crates/hub/src/update.rs` exists, `residuum` otherwise; and gives each an arm in `plan_rust_tests` that adds that package to the member tests (or every root unit test while it is still the root). `scripts/affected-packages-test.sh` gains a case for each with and without the member. `RUST_PATHS_RE` already lists both and does not change. The hand-kept list in `CLAUDE.md`/`AGENTS.md` ("Workspace") and `CONTRIBUTING.md` ("The crates") of files a member embeds from outside its directory and outside `assets/` gains both.
- **Pre-push member-test scope** (chunk T2, if D9 (a)). `scripts/affected-packages.sh` gets a mode `--touched-members` that prints the members the changed paths touch, without their dependents, and `plan_rust_tests` uses it for the member tests; clippy keeps `--affected`. The hook's header comment (`:373-399`), `CLAUDE.md`/`AGENTS.md` ("Git Hooks", "What Runs Where") and `scripts/affected-packages-test.sh` change with it.
- **Hard-coded paths in the scans** (each X chunk, in the commit that moves the file). `src/testing/source_scan.rs`: `EXEC_MODE_ALLOWED` (`:555-567`: `src/update.rs`, `src/checkpoints/engine.rs`, `src/gateway/web/agent_keys.rs`), the classifier fixtures (`:646-689`: `src/agent/turn.rs`, `src/hub/host.rs`, `src/hub/host/tests.rs`, `src/hub/host/tests/agent_watch.rs`, `src/hub/test_support.rs`, `src/interfaces/teams/setup_job.rs`), which the existence test reads as real paths; `src/testing/wall_clock_backlog.rs:14` (`src/a2a/server_e2e_tests.rs`). The pre-commit hook lets a backlog entry follow its file only when the `git mv` and the entry change are in one commit and Git sees a rename, so the move, the path rewrites and the entry go together.
- **Doc paths.** `src/tools/CLAUDE.md` names `src/tools/TOOLS.md`, `src/tools/registry.rs` and `src/gateway/startup/tools.rs`; `src/background/README.md`, `docs/systems-usage/*.md` and their mirrors under `assets/bundled-skills/residuum-system/references/`, `CONTRIBUTING.md` and `CLAUDE.md`/`AGENTS.md` name module paths. Each chunk updates every `.md` reference to a path it moves (`git grep -nF '<old path>' -- '*.md' ':!docs/archive'` prints nothing).
- **`justfile`.** No recipe names a root module; `pebble` stays on `residuum-net`. `just test-crate <crate>` works for every new crate.
- **Layout docs** (chunk F1). `CONTRIBUTING.md` ("The crates" table and diagram, "Dependency rules", "The root facade"), `CLAUDE.md`/`AGENTS.md` ("Workspace", "Testing": where `bus_wait` and the other helpers live; the `RUST_LOG` paragraph), `docs/systems-usage/hub.md` ("Logging").

## 10. Implementation plan

### How every chunk is verified

Round 1's common gate (`docs/archive/workspace-split.md` §14), unchanged:

- `cargo fmt --all --check`; `cargo clippy --all-targets --all-features -- -D warnings`; `cargo test --quiet --no-fail-fast`; `scripts/check-generated-types.sh`; `cargo deny check` and `cargo shear` when a manifest changed.
- **Test count**: the number of tests after the chunk equals the count at its base plus the delta its row states, counted with round 1's `count_tests` procedure. Tests that move are renamed, not lost.
- `just stress <filter>` for every test the chunk adds or rewrites.
- **Docs follow the files**: no `.md` outside `docs/archive` names a path the chunk moved.
- The chunk's own check (the table's column), push through the pre-push hook, a pull request listing every item made `pub` (§6) and the test-count delta, `gh pr merge <n> --merge --auto`.

Each extraction (X chunk) also verifies: every moved file shows as a rename in `git diff -M --summary origin/main...HEAD`; `cargo tree -p residuum-<x> -e normal,dev --depth 1` lists exactly the workspace crates §4 gives it (the chunk corrects §4 and `CONTRIBUTING.md` if the compiler disagrees); `scripts/check-crate-isolation.sh` passes; `cargo test --quiet -p residuum-<x>` passes with `web/dist` moved aside; `grep -rnE 'crate::(<each module still in the root or in a crate above>)\b|\bresiduum::' crates/<dir>` prints nothing; the source scan, `unused_pub` included, passes.

### The extraction procedure (X1-X10)

1. `git mv` the modules into `crates/<dir>/src/`; a single-module crate's `mod.rs` becomes `lib.rs`, a bundle crate (proactivity, runtime) gets a new `lib.rs` declaring its modules.
2. Write `crates/<dir>/Cargo.toml` to `CONTRIBUTING.md`'s rules: `[lints] workspace = true`, inherited package fields, the `test-support` feature where §7 gives one, the dependencies its sources name (round 1 §4's grep check, one per candidate crate), and the dev-dependencies §7 requires.
3. In the moved files, rewrite `crate::<self>::` to `crate::` (or to `crate::<module>::` in a bundle crate), and each `crate::<other member>::` to `residuum_<crate>::…`; fix `pub(in …)` paths, `include_str!` depths and doc links.
4. Root `Cargo.toml`: the `[workspace.dependencies]` entry with path and version, the `[dependencies]` line, the `[dev-dependencies]` line with `test-support` when the crate has it; the same lines in every member that uses it. Replace `pub mod <x>;` in `src/lib.rs` with its facade line (§6). Remove root dependencies the root no longer names.
5. Build, and decide each visibility error by §6's rule; never widen a whole module.
6. Put the helpers §7 lists behind `test-support`.
7. Update `source_scan.rs` and `wall_clock_backlog.rs` paths in the commit that moves the files.
8. Update the `.md` references and add the crate to the hub.md list.
9. Run the verification above.

### Chunks

Prefixes: M measure, T tooling, P preparation inside the code where it lies today (ordinary refactors), X extraction, F follow-through. "Haiku" means a focused pass by a Haiku agent finishes it: mechanical moves the compiler guides. "Opus" says why it needs more.

| chunk | scope | own check | test delta | depends on | agent |
|---|---|---|---|---|---|
| **M0** baseline | §12's protocol on `main`, every row, recorded in §12's M0 column. No code change. | three runs per row, recorded | 0 | — | Haiku |
| **T1** path routing for read-by-path files | §9's first item | `scripts/affected-packages-test.sh` passes with the new cases | 0 | M0 | Haiku |
| **T2** pre-push member-test scope | §9's second item (only if D9 (a)) | the script test; a scratch push touching `crates/inference/` runs `residuum-inference`'s tests and not those of `residuum-memory` and `residuum-mcp`, which depend on it | 0 | T1 | Haiku |
| **P1** session vocabulary into the bus | §5, the session rows | `grep -rnE '^[^/]*crate::background::' src/bus` prints nothing | 0 | M0 | Haiku |
| **P2** origin, conversations, attachment | §5, the interfaces payload rows; `TEAMS_ENDPOINT` into config; the attachment remainder to `inbox::attachment` | `grep -rnE '^[^/]*crate::interfaces::' src/bus src/inbox` prints nothing | 0 | P1 | Haiku |
| **P3** the remaining payloads | usage, auto-mode verdict, tracked task, workspace change into the bus; `notify/types.rs` into `residuum_config::channels`; the bus's doc links as plain text | `grep -rnE 'crate::(agent|a2a|workspace|notify|background|interfaces)::' src/bus` prints nothing, doc comments included | 0 | P2 | Haiku |
| **P4** control types and notices | `bus::control`, `bus::notice`; the tools and checkpoints copies call it | `grep -rn 'gateway::helpers\|gateway::types::\(Reload\|ServerCommand\|StopRequest\)' src` prints nothing | 0 | P1 | Haiku |
| **P5** interfaces off the gateway | `AdapterSenders`, `stop_named_session`, the Teams setup types, `websocket` into the gateway | `grep -rnE '^[^/]*crate::gateway::' src/interfaces` prints nothing; the `/stop` and session-stop tests pass unchanged | 0 | P4 | Opus: the WebSocket move and the stop-error split touch the gateway's busiest files and must keep every message identical |
| **P6** the protocol module | `src/protocol/` with `frames` | `grep -rn 'gateway::protocol\|gateway::sessions::session_event\|websocket::subscriber::workspace_frame' src tests` prints nothing | 0 | P3, P5 | Haiku |
| **P7** workspace, notify, proactivity edges | §5's P7 table; `process` into core; the two upward tests moved | `grep -rnE '^[^/]*crate::(hub|notify|pulse|a2a)::' src/workspace src/inbox` and `grep -rnE '^[^/]*crate::background::' src/pulse src/actions` print nothing | 0 (tests move, not added) | P3 | Haiku |
| **P8** the team slice | `src/team/` (router, `TeamDirectory`, `DirectoryHandle`, types); the hub's `AgentDirectory: TeamDirectory`; the A2A listener and `StaticAgentDirectory` on `TeamDirectory`; `StaticHubDirectory` | `grep -rnE '^[^/]*crate::hub\b' src/{agent,tools,background,a2a,subconscious,team,workspace} src/diagnostics_dispatch.rs src/client_context.rs` prints nothing | 0 | P1 | Opus: a trait split under the hub's lifecycle, the A2A listener and agent tools, where an error would change what an agent may do to a teammate |
| **P9** `BuildInfo` | §5's version row | `grep -rn 'RESIDUUM_VERSION\|RESIDUUM_GIT_COMMIT' src crates` prints only `src/lib.rs`; the version tests pass with literals | 0 | M0 | Haiku |
| **P10** the hub's process leaves | `update`, `features`, `config_watcher`, `HubExit`, `TermSignal`, `CloudStatusResponse`, `agent_span` into the hub; `last_known_good` and `FilterReload` into config; `HubInputs` with `on_ready` and `log_filter` | `grep -rnE '^[^/]*crate::(update|features|daemon|logging)\b' src/hub` and `grep -rn 'gateway::\(watcher\|last_known_good\|types::GatewayExit\|types::TermSignal\|web::cloud::CloudStatusResponse\)' src` print nothing | 0 | P4, P9 | Haiku |
| **P11a** the launcher | `hub::launch`, `AgentLauncher`, `StartedAgent`; `GatewayLauncher`; the host starts agents through it; `src/wiring.rs` builds it; repair and file routers through it | `grep -rn 'gateway::event_loop\|gateway::web::\(ConfigApiState\|agent_repair\|agent_files\|AgentFilesState\|CheckpointAccess\)\|gateway::file_server' src/hub --include=*.rs \| grep -v '/tests'` prints nothing; the hub tests pass unchanged (in the single crate they can still build gateway routers) | 0 | P8, P10 | Opus: the agent start, supervision and restart path |
| **P11b** web routes and assets | `HubWebRoutes`; `process.rs` into `gateway::web::hub_routes`; the embed into `src/wiring.rs` and `static_assets` over the getter | `grep -rnE '^[^/]*crate::gateway' src/hub --include=*.rs \| grep -v '/tests'` prints nothing; the hub HTTP tests pass unchanged | 0 | P11a | Haiku |
| **P11c** hub tests | the host integration tests and the gateway-route HTTP tests into `src/gateway/hub_tests/`; `StubLauncher`; the HTTP harness on it | `grep -rn 'crate::gateway' src/hub` prints nothing; test count unchanged | 0 | P11b | Opus: about 125 host tests and the gateway-route HTTP tests change crate-to-be, and each must keep testing what it tests |
| **X1** `residuum-bus` | §4; the `Recv for AgentChangeReceiver` impl into `src/hub/` | the extraction checks | 0 | P1-P4 | Haiku |
| **X2** `residuum-workspace` | §4 | | 0 | X1, P7 | Haiku |
| **X3** `residuum-checkpoints` | §4 | | 0 | X2 | Haiku |
| **X4** `residuum-notify` | §4; the notification target dependencies | the Windows and macOS clippy via the `cross-compile` label | 0 | X2 | Haiku |
| **X5** `residuum-proactivity` | §4 | | 0 | X2, P7 | Haiku |
| **X7** `residuum-protocol` | §4 | `scripts/check-generated-types.sh` shows no change | 0 | X2, P6 | Haiku |
| **X6** `residuum-runtime` | §4; `test-support` helpers; `TOOLS.md`, `CLAUDE.md`, `README.md` paths | `cargo test -p residuum-runtime` alone | 0 | X3, X4, X5, P8, P9 | Opus: about 52k lines and the largest public surface; every widening is decided by §6's rule |
| **X8** `residuum-interfaces` | §4 | | 0 | X6, P5 | Haiku |
| **X9** `residuum-hub` | §4; `is_hub_level`; `test-support` helpers | `cargo test -p residuum-hub` alone, without `web/dist` | 0 | X6, X7, P11c, T1 | Opus: the hub's API to the gateway is new, and its tests change harness |
| **X10** `residuum-gateway` | §4; `tests/it` and `ts_export` paths | `cargo test -p residuum-gateway` alone, without `web/dist` | 0 | X4, X8, X9, T1 | Opus: the root and the integration tests reach most of the gateway's items, each decided by §6's rule |
| **F1** docs | §9's last item; this document moves to `docs/archive/` with §12's results | `git grep -nE 'src/(gateway|hub|interfaces|bus|agent|tools|background|a2a|workspace|notify|inbox|checkpoints|pulse|subconscious|actions|update\.rs|features\.rs|process\.rs|client_context\.rs|diagnostics_dispatch\.rs)\b' -- '*.md' ':!docs/archive'` prints nothing | 0 | X10 | Haiku |
| **F2** measure | §12's protocol on the split; an `investigate` issue for each missed target | | 0 | X10 | Haiku |

**What runs at the same time.** Two chunks may be in flight together exactly when neither depends on the other through the last column. After M0: T1, P1 and P9 together; then P2, P4 (both after P1); P3 after P2; P5 after P4; P6, P7 and P8 once theirs land; P10; then P11a, P11b, P11c in order. X1 waits for every P chunk it lists; then X2; then X3, X4, X5 and X7 together; X6; X8 and X9 together; X10; F1 and F2. Chunks in flight together still meet in shared files (`src/lib.rs`, the root `Cargo.toml`, `Cargo.lock`, `src/bus/mod.rs`, `source_scan.rs`); the later branch merges `main` and resolves them, `Cargo.lock` by round 1's procedure (take `main`'s, let `cargo metadata` add the branch's members, check the diff touches only `residuum*` entries).

**Stopping between chunks.** Every chunk leaves `main` working, so the work can pause after any of them, and each can be reverted on its own. After the P chunks the root is still one crate, with its internal graph acyclic; the X chunks then only move files.

## 11. Risks, and what to do when something regresses

Round 1's regression table (`docs/archive/workspace-split.md` §13) still applies; these are the ones round 2 adds or makes likelier.

| regression | what would be seen | what catches it |
|---|---|---|
| A test is lost when tests change crate (P11c, the upward tests) | fewer tests, all green | the test-count check of every chunk |
| An agent's start, stop or crash recovery behaves differently through the launcher | an agent that doesn't start, restart, or report its failure | the host integration tests, unchanged in substance, now in `gateway::hub_tests`; P11a lands while they still run in place |
| The version shown is wrong | hub status, the web UI's version, the workbench SDK or a bug report disagrees with the binary | the version tests, which assert the `BuildInfo` passed in comes back out; P9's grep that only `src/lib.rs` reads the variables |
| A member reads `env!("RESIDUUM_VERSION")` later | it fails to compile (the variable is only set for the root), which is the guard | the compiler |
| The web app is missing or stale | a blank page or an old UI | the root's assets tests (moved from `assets.rs`), which read the real build; `build.rs` still asserts `web/dist` |
| A log line moves to a target nobody filters on | `RUST_LOG=residuum::hub=trace` has no effect | not detected; documented in hub.md's crate list and `CLAUDE.md` (each X chunk, F1) |
| `is_hub_level` misses the hub's new prefix | the host test fails, reporting hub lines without an agent | the test itself (X9 updates it) |
| A Teams or `/stop` message changes wording | a user sees a different message | the existing message tests, which P5 keeps unchanged |
| Pre-push stops running a dependent's tests (D9 (a)) | a broken dependent passes pre-push | the PR lane runs the affected packages' tests, the merge queue everything; the hook prints what it chose |
| The test run gets slower | `cargo test` takes longer although builds are faster | row 7; libtest runs each crate's binary in turn, so ten more binaries add their slowest tests in series. If it misses its target, measure where the time goes before changing the runner |
| A dev-dependency cycle sneaks in through a test helper | a crate compiled twice in tests, types that stop unifying | `manifest_scan` rule 5 |
| A crate builds only because another turns on a feature | `cargo test -p residuum-<x>` fails for a developer though CI was green | the merge queue's isolation job |

**Recovery.** Any chunk is reverted by a revert pull request of its merge commit. A P chunk's revert restores the old module path in one place; an X chunk's revert puts the files back in the root without touching the other crates. Measurements are taken at M0 and F2, and an orchestrator can measure between X chunks to decide whether to continue.

## 12. Measurement plan

Round 1's protocol (`docs/archive/workspace-split.md` §15, "Protocol") applies unchanged: thermaltake-tower, a checkout on the SSD, the machine's jobserver with no other build running (`build-jobserver status` shows 0 tokens in use, `pgrep -c rustc` prints 0), the commit, `rustc --version`, `sccache --version` and the token count recorded, one fixed target directory per row under `"$PWD/target/measure/<row>"`, `--timings` reports kept, three runs per row each with its own edit, medians recorded. M0 runs on `main` before the first chunk after it merges; F2 runs on `main` after X10. Each row names its file on both trees as `M0 path` / `split path`.

1. **Cold `cargo test --no-run`, no cache.** As round 1 row 1. Record the wall time, `D` (the end of the last non-workspace unit) and the workspace phase.
2. **Rebuild after a gateway edit.** As round 1 row 2, with `touch src/gateway/event_loop/run_loop.rs` / `crates/gateway/src/event_loop/run_loop.rs` (the most edited file). Record the per-unit times of the rebuilt units.
   - **2r. Rebuild after a root edit.** Round 1's row 2 as it was: `touch src/lib.rs` on both trees.
3. **Rebuild after a lower-crate edit.** 3a as round 1 (`touch crates/memory/src/search.rs`, whole workspace). 3b as round 1 (`-p residuum-memory`). 3c (new): `touch src/tools/exec.rs` / `crates/runtime/src/tools/exec.rs`, whole workspace.
4. **Clippy after a gateway edit.** As round 1 row 4, with row 2's file.
5. **CI build step.** As round 1 row 5: three throwaway pull requests appending `// measure row 5` to `src/hub/runtime.rs` / `crates/hub/src/runtime.rs`, `src/gateway/mod.rs` / `crates/gateway/src/lib.rs`, and `src/agent/mod.rs` / `crates/runtime/src/agent/mod.rs`, never enqueued, closed with `gh pr close <n> --delete-branch`. Record the "Build test binaries" step's duration from `gh run view --json jobs`, and, from the job summary page by hand, the CPU table and which member crates were sccache hits.
6. **pre-push**, warm, checked in place, the hook run directly with `CARGO_TARGET_DIR="$PWD/target/measure/row6<x>"`, by round 1 row 6's sibling-branch procedure:
   - 6a. a hub push: `src/hub/runtime.rs` / `crates/hub/src/runtime.rs` (round 1's 6a file).
   - 6b. a push touching only one new member crate: row 2's gateway file.
   - 6c. a memory-only push: `crates/memory/src/search.rs` on both trees (round 1's 6b, never measured).
7. **Full test run.** As round 1 row 7: one untimed `cargo test --quiet --no-run`, then three timed `cargo test --quiet --no-fail-fast` with no edit.

| measure | M0 | result | target | expected because |
|---|---|---|---|---|
| 1. cold `cargo test --no-run`: workspace phase | | | ≤ 80% | the root's lib test harness, the largest unit and the critical path after round 1, becomes ten crates' harnesses that build in parallel once their libraries' metadata exists |
| 2. rebuild, gateway edit | | | ≤ 65% | the gateway (49k lines) and the root (11k) rebuild instead of 186k; linking the root's test binaries, which still link everything, remains |
| 2r. rebuild, root edit | | | ≤ 45% | the root's front end is 6% of today's; what remains is mostly linking the four root test targets |
| 3a. rebuild, memory edit, whole workspace | | | ≤ 110% | the same lines rebuild, now as a chain of crates instead of one; per-crate overhead may add a little |
| 3b. rebuild, memory edit, `-p residuum-memory` | | | ≤ 110% | nothing round 2 does is in memory's build |
| 3c. rebuild, tools edit, whole workspace | | | ≤ 90% | about 157k of 186k lines rebuild (the runtime and the crates above it) |
| 4. clippy, gateway edit | | | ≤ 50% | clippy is front end only, and only the gateway and the root are checked again |
| 5. CI build step, the three PRs | | | ≤ 75%, with every unchanged member's library an sccache hit | package selection builds only the affected crates' test binaries, and `CARGO_INCREMENTAL=0` makes unchanged members cacheable |
| 6a. pre-push, hub push | | | ≤ 85% | clippy covers the hub, the gateway and the root; the tests are the hub's own; the scans build a small root harness |
| 6b. pre-push, gateway push | | | ≤ 70% | clippy and tests cover the gateway and the root only |
| 6c. pre-push, memory push | | | ≤ 110% with D9 (a); with D9 (b), record only | with (a) memory's own tests run and clippy covers every crate above it, as the root today; with (b) nearly every test runs |
| 7. full test run | | | ≤ 130% | the same tests in ten more binaries, which libtest runs one after another, each with its own slowest test as a tail, plus ten more empty doctest passes |

A missed target is investigated from the `--timings` reports and the sccache stats before anything is changed. A missed build-time target alone is not a reason to revert an extraction.

## 13. What round 2 leaves in place

- **The runtime is one 52k-line crate**, the second most edited (271 commits in the window), and each edit to it rebuilds about 157k lines. D6 (b) is the lever: the tool container into the agent, the tool builders as functions, a session-tool factory owned by background. It is planned from F2's numbers.
- **The bus stays expensive to edit.** Every crate above it depends on its event vocabulary, so its 63 monthly commits each rebuild everything; no layout changes that.
- **The chat adapters stay in one crate** (D4); Teams is the candidate to split first.
- **Two `A2aVisibility` enums** remain, one in `residuum_config` (the parsed setting, `crates/config/src/types.rs:253`) and one in the lifecycle types (the API form, moving to `team::types`); round 2 moves the second and does not merge them.
- **The root keeps `daemon`, `logging` and `setup_wizard`**, which only the CLI and the wiring use.
- **Not checked by this design**: the exact list of `pub` items each extraction widens or `unused_pub` reports (decided per item during the extraction), the methods and fields the gateway reaches on hub types through values, the complete set of test helpers that become `test-support` (the compiler reports them at each `cargo test -p`), and whether `server_e2e_tests.rs` needs any runtime item that is `#[cfg(test)]` today beyond `test_memory_extras`.
