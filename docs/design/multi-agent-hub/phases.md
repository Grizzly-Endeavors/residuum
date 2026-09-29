# Multi-Agent Hub — Implementation Phases

**Status:** Draft, not built. Read with [design.md](design.md).

> Module level only. Each phase is self-contained, depends only on phases before it, and leaves `main` in a working, releasable state. Each phase is implemented in its own session, on its own branch, with its own PR.

## Phase 1 — Hub split: layout, config, migration (single agent)

- **Modules:**
  - config (bootstrap, loading, resolution, validation, last-known-good, environment overrides);
  - workspace layout;
  - checkpoints;
  - daemon and process files, tracing init, update and update-watchdog;
  - agent-key and A2A-key stores (location);
  - setup wizards (CLI and web);
  - the new layout-migration module;
  - gateway startup.
- **Preconditions:** none.
- **Shape when done:**
  - **Layout.** `~/.residuum/` has `hub/` plus exactly one agent directory. Hub config and agent config are separate schemas, loaded from their new locations and hot-reloaded independently, each with its own last-known-good.
  - **Config fields.** `name` and `workspace_dir` are gone. Agent-scoped environment overrides are removed, and a startup notice names any that are still set. Hub-level overrides still work.
  - **Hub files.** Secrets, key stores, logs, bin, checkpoints, pid, lock, ready and update markers all live in `hub/`. No path is hard-coded to the legacy root.
  - **Checkpoints.** A local-only hub config repo, plus per agent a workspace repo and a local-only config repo, as the design specifies.
  - **A2A.** Hop limits and `[a2a] legacy_alias` are hub-level, and the alias is set by onboarding and migration.
  - **Agent names.** Validation exists (relay slug rules plus the reserved names) and is used by onboarding and migration.
  - **Onboarding.** It asks for the user's name and the first agent's name.
  - **Migration.** The layout migration framework exists: a layout version marker, a journal, rollback, and backups. Its *hub split* step converts a legacy install, including the `residuum migrate --agent-name` entry point and the interactive and headless naming paths.
  - **Unchanged.** The process still runs exactly one agent. The HTTP API, web UI, tunnel and A2A behave as before.
- **Verification:**
  - A fresh install onboards into the new layout.
  - A copied legacy `~/.residuum` fixture migrates, and the agent starts with its memory, sessions, skills, adapters and checkpoints intact.
  - An injected failure mid-migration rolls back to the legacy layout, and startup reports it.
  - Rerunning migration is a no-op.
  - Config hot reload works for both files.
  - The existing test suite passes.

## Phase 2 — Team layer

- **Modules:**
  - workspace layout and bootstrap;
  - prompt assembly and the subconscious context loader;
  - file tools and path policy;
  - workspace file HTTP API and change feed;
  - wiki search indexing and memory search;
  - skills discovery;
  - workbench server and artifact paths;
  - checkpoints;
  - layout migration (new step).
- **Preconditions:** Phase 1.
- **Shape when done:**
  - **Team directory.** `team/` exists with `AGENTS.md`, `USER.md`, `wiki/` (including `wiki/agents/`), `workbench/` and `skills/`.
  - **Migration.** The *team layer* step moves those files out of the agent, relocates skills that match bundled skills, and creates the agent's role page.
  - **Prompt.** Prompt assembly reads SOUL from the agent and AGENTS, USER and the wiki index from the team, in the order the design gives.
  - **The `team/` namespace.** It works in file tools, the web file API, and change-feed watches.
  - **Team write coordination.** Version recorded at read, per-path lock, conflict error naming the other writer, atomic writes, web `If-Match` under the same lock.
  - **Search.** The team wiki has its own index, agent memory search merges it in, and agent indexes no longer hold wiki pages.
  - **Skills.** Layered agent, then team, then configured directories. Bundled skills are installed into `team/skills/`.
  - **Workbench.** Served from `team/workbench/`.
  - **Checkpoints.** A team checkpoint repo (workspace-style).
  - **Unchanged.** Still one agent in the process, and the API paths are unchanged.
- **Verification:**
  - A migrated Phase 1 install gains `team/`, and the agent's prompt contains the same content as before.
  - Wiki search finds team pages.
  - Two concurrent writers to one team file (tests with two tool instances, and a tool plus the web API) produce exactly one success and one conflict error that names the writer.
  - An agent skill shadows a team skill of the same name.
  - Workbench artifacts load and save state.

## Phase 3 — Multi-agent runtime and lifecycle

- **Modules:**
  - gateway runtime, split into hub and per-agent runtime, plus a new agent host;
  - session runtime (the shared budget);
  - tracing (the agent field, `logs --agent`);
  - removal of the process working-directory change and any other process-global agent state;
  - the HTTP router (hub, team and agent prefixes, per-request agent resolution);
  - WebSocket handlers;
  - CLI `agent` commands;
  - artifact session start (required agent);
  - the hub-level bus, hub notices and the hub WebSocket;
  - agent-key creator attribution (`agent:<name>`);
  - the A2A listener and tunnel, bound to the `legacy_alias` agent;
  - the web UI's API and WebSocket base paths, plus a minimal agent picker.
- **Preconditions:** Phase 2.
- **Shape when done:**
  - **Discovery and startup.** The hub scans for agent directories and starts `autostart` agents, each in its own runtime with its own bus, event loop, adapters and pulse.
  - **Lifecycle.** States (`starting`, `running`, `stopped`, `failed` with error) and operations (start, stop, restart, create, delete), exposed through `/api/hub/agents` and `residuum agent list|create|delete|start|stop|restart`.
  - **Create and delete.** Both follow the design's order. Create includes the blank template, role page, and first message from the description, but in this phase is available only to users. Delete includes the checkpoint before removal.
  - **Isolation.** A panic or fatal error in one agent leaves the hub and the other agents running.
  - **Shared budget.** One semaphore sized by the hub's `max_concurrent` governs every agent's sessions.
  - **Logs.** Every log line carries `agent`.
  - **HTTP.** Agent-scoped routes live under `/api/agents/<name>/`, with `404` for an unknown agent and `409` for one that isn't running (except config and file routes).
  - **Artifacts.** Artifact session start requires an agent.
  - **A2A.** It behaves exactly as today on behalf of the `legacy_alias` agent. Other agents can call out but have no inbound entry. Sibling discovery results reach every agent.
  - **Hub notices.** Created, deleted and failed notices reach the hub WebSocket and the acting or affected agent's inbox.
  - **Web UI.** It works against the prefixed API with a basic picker (the full UI is Phase 5).
- **Verification:**
  - Integration test: a hub with two agents. Chat with each over its WebSocket, and confirm memory and sessions stay separate.
  - Stopping one agent leaves the other serving.
  - A forced panic in one agent marks it `failed` and the other keeps running.
  - With a budget of one, sessions from both agents queue against it.
  - Create through the CLI produces a running agent with a role page and no bootstrap interview, and the description arrives as its first message.
  - Delete removes the directory and the role page, and the agent can be restored from its checkpoint.
  - An external A2A caller that worked before this phase still reaches the alias agent at the same URL.

## Phase 4 — Teamwork: addressing, messaging, agent tools

- **Modules:**
  - addressing and parsing;
  - the new hub team router;
  - the messenger (cross-agent delivery, attribution, hop propagation);
  - `message_agent` and `list_agents`;
  - new `agent_create` and `agent_delete` tools;
  - prompt assembly (the `TEAM` block);
  - notifications (created and deleted notices).
- **Preconditions:** Phase 3.
- **Shape when done:**
  - **Addresses.** `agent:<name>` and `agent:<name>/<session>` parse and route through the team router to the target agent's messenger, with today's delivery semantics.
  - **Attribution.** The receiver sees the sender's fully qualified address, labeled as a teammate, and can reply by messaging it.
  - **Hops.** Hop counts carry across agents, and the soft and hard limits apply to the whole chain.
  - **Failed deliveries.** Delivery to a stopped, failed or unknown agent, or to a missing session, returns a tool error explaining which.
  - **`list_agents`.** It includes teammates with state and role line.
  - **`TEAM` block.** Every agent's prompt carries it.
  - **Agent tools.** `agent_create` (inheriting the creator's `providers.toml`) and `agent_delete` work, and both publish notices naming the actor.
- **Verification:**
  - Agent A messages `agent:b`, B replies to A's address, and A receives it attributed to B.
  - A session of A messages B, and B's reply to `agent:a/<session>` reaches that session.
  - A two-agent message loop hits the hard hop limit and both sides see the refusal.
  - Messaging a stopped agent returns the stopped error.
  - `agent_create` with a description produces an agent that writes `SOUL.md` notes and fills its role page on its first turn. The creator's model config is copied, and the user sees the notice.

## Phase 5 — Web UI and visibility

- **Modules:**
  - the web SPA (router, switcher, team view, settings split, onboarding, workbench and artifact bridge);
  - the hub WebSocket;
  - turn and session counters (tool-call count);
  - the notifications surface.
- **Preconditions:** Phase 4.
- **Shape when done:**
  - **Routes.** `/agent/<name>/...` and `/team/...`, with `/` going to the last-used or first agent.
  - **Switcher.** Always visible, with a state dot and activity or unread indicator per agent, fed by `/api/hub/ws`.
  - **Team view.** State, last error, counters, create (name, description, model config from a chosen agent), start, stop, restart, delete and the `autostart` toggle.
  - **Settings split.** Hub settings under `/team/settings` and agent settings under `/agent/<name>/settings`.
  - **Onboarding.** The web onboarding flow includes the user's name and the first agent's name.
  - **Artifacts.** Artifact sessions pass their agent, and the workbench bridge surfaces the refusal error.
  - **Counters.** Turns and sessions count and expose tool calls next to tokens and elapsed time.
  - **Notices.** Created, deleted and failed notices appear in the UI.
- **Verification:**
  - A UI walkthrough, driven in a real browser: onboarding, create a second agent from the team view, switch between agents, and watch the other agent's unread indicator light up when it receives a teammate message.
  - Stop and restart from the team view.
  - Edit hub versus agent settings, and confirm each lands in the right file.
  - The artifact session names an agent.
  - Web lint, type-check and unit tests pass.

## Phase 6 — A2A per agent and relay protocol

- **Modules:**
  - **relay repository:** tunnel protocol (the `agents` capability and `AgentsUpdate` frame), database schema for instance agents, A2A routing by instance and agent, legacy alias, directory entries, compatibility gating;
  - **residuum:** tunnel client (declaring the capability, sending `AgentsUpdate` on changes), A2A listener routing by agent, per-agent cards and public URLs, per-agent visibility, sibling discovery filtering and `<instance>/<agent>` sibling names;
  - migration step (legacy alias).
- **Preconditions:** Phase 4. It is independent of Phase 5.
- **Shape when done:**
  - **Relay.** Relay changes are deployed first and are no-ops for clients without `agents`.
  - **Declaring agents.** A hub declares `agents` and keeps the relay's agent list current across create, delete and visibility changes, and after every reconnect.
  - **Routing.** Each agent is reachable at `{origin}/a2a/{instance}/{agent}` with its own card and visibility. The legacy instance path reaches the alias agent.
  - **Discovery.** The directory lists per-agent entries to `agents`-capable callers only. Sibling discovery excludes teammates.
- **Verification:**
  - Relay tests: a legacy client is unaffected, including directory output; `AgentsUpdate` replaces the list idempotently; routing by instance and agent works; private agents are hidden from non-siblings; the legacy alias routes correctly.
  - Residuum integration test against a local relay: two agents in one hub, each reachable by URL with its own card, one set private and hidden from a non-sibling key holder.
  - A second hub discovers both agents as siblings. The hub's own agents never list each other as siblings.

## Phase 7 — Documentation and clients

- **Modules:**
  - `docs/systems-usage/` (a new hub and teams document, plus updates to config, background tasks, A2A, wiki, workbench, skills and cloud tunnel);
  - bundled `residuum-system` skill references;
  - migration guide;
  - Docker docs;
  - Mac app (per the open-question outcome);
  - archiving this design.
- **Preconditions:** Phases 1–6.
- **Shape when done:**
  - Systems-usage docs and bundled references describe the hub as built.
  - The migration guide covers the layout change, the removed environment overrides, and the A2A URL change and alias.
  - Docker docs match.
  - The Mac app matches the chosen outcome.
  - This design and its phases live in `docs/archive/`.
  - Issue #205 is closed.
- **Verification:**
  - Run doc-sync and find no drift between code and docs.
  - A reader following only the migration guide upgrades a legacy install.
  - The design doc's goals hold end to end on a real install: switch agents, hand work off, agent-created agent, shared wiki and workbench, per-agent A2A entries.
