# Multi-Agent Hub — Implementation Phases

**Status:** Accepted, not built. Read with [design.md](design.md).

> Module level only. Each phase is self-contained, depends only on phases before it, and leaves `main` in a working, releasable state. Each phase is implemented in its own session, on its own branch, with its own PR.

## Phase 1 — Hub split: layout and config (single agent)

- **Modules:**
  - config (bootstrap, loading, resolution, validation, last-known-good, environment overrides);
  - workspace layout;
  - checkpoints;
  - daemon and process files, tracing init, update and update-watchdog;
  - agent-key and A2A-key stores (location);
  - setup wizards (CLI and web);
  - gateway startup.
- **Preconditions:** none.
- **Shape when done:**
  - **Layout.** `~/.residuum/` has `hub/` plus exactly one agent directory. Hub config and agent config are separate schemas, loaded from their new locations and hot-reloaded independently, each with its own last-known-good.
  - **Config fields.** `name` and `workspace_dir` are gone. Agent-scoped environment overrides are removed, and a startup notice names any that are still set. Hub-level overrides still work.
  - **Hub files.** Secrets, key stores, logs, bin, checkpoints, pid, lock, ready and update markers all live in `hub/`. No path is hard-coded to the old root.
  - **Checkpoints.** A local-only hub config repo, plus per agent a workspace repo and a local-only config repo, as the design specifies.
  - **Hub-level limits.** Hop limits and the session budget are read from hub config.
  - **Agent names.** Validation exists (relay slug rules plus the reserved names) and is used by onboarding.
  - **Onboarding.** It asks for the user's name and the first agent's name.
  - **Unchanged.** The process still runs exactly one agent. The HTTP API, web UI, tunnel and A2A behave as before.
- **Verification:**
  - A fresh install onboards into the new layout.
  - The agent runs normally from the new layout: memory, sessions, skills, adapters and checkpoints all work.
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
  - checkpoints.
- **Preconditions:** Phase 1.
- **Shape when done:**
  - **Team directory.** `team/` exists with `AGENTS.md`, `USER.md`, `wiki/` (including `wiki/agents/`), `workbench/` and `skills/`.
  - **Pulses.** The first agent's `HEARTBEAT.yml` includes `wiki_lint`. The created-agent template (used from Phase 3) omits it.
  - **Bootstrap.** Onboarding writes the team layer's defaults and the first agent's role page. Bundled skills are installed into `team/skills/`, not the agent's directory.
  - **Prompt.** Prompt assembly reads SOUL from the agent and AGENTS, USER and the wiki index from the team, in the order the design gives.
  - **The `team/` namespace.** It works in file tools, the web file API, and change-feed watches.
  - **Team write coordination.** Version recorded at read, per-path lock, conflict error naming the other writer, atomic writes, web `If-Match` under the same lock.
  - **Search.** The team wiki has its own index, agent memory search merges it in, and agent indexes no longer hold wiki pages.
  - **Skills.** Layered agent, then team, then configured directories. Bundled skills are installed into `team/skills/`.
  - **Workbench.** Served from `team/workbench/`.
  - **Checkpoints.** A team checkpoint repo (workspace-style).
  - **Unchanged.** Still one agent in the process, and the API paths are unchanged.
- **Verification:**
  - A fresh install has `team/`, and the agent's prompt carries the team `AGENTS.md`, `USER.md` and wiki index.
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
  - the A2A listener (per-agent routing on the local port, per-agent cards, public URLs and visibility);
  - the web UI's API and WebSocket base paths, plus a minimal agent picker;
  - the Mac app (remove the agent registry and agent tabs; its single connection uses an agent's prefixed WebSocket path).
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
  - **A2A.** Each agent is reachable locally at `/agents/<name>/` on the A2A port with its own card and visibility, and can call out with `a2a:` addresses. The tunnel stops declaring the `a2a` capability until Phase 6 teaches the relay per-agent routing, so there's no remote inbound A2A in between (A2A isn't in a release yet).
  - **Hub notices.** Created, deleted and failed notices reach the hub WebSocket and the acting or affected agent's inbox.
  - **Web UI.** It works against the prefixed API with a basic picker (the full UI is Phase 5).
  - **Mac app.** No multi-agent code remains, and it chats with one agent over the prefixed path. Swift can't be built on Linux, so this is verified on a Mac.
- **Verification:**
  - Integration test: a hub with two agents. Chat with each over its WebSocket, and confirm memory and sessions stay separate.
  - Stopping one agent leaves the other serving.
  - A forced panic in one agent marks it `failed` and the other keeps running.
  - With a budget of one, sessions from both agents queue against it.
  - Create through the CLI produces a running agent with a role page and no bootstrap interview, and the description arrives as its first message.
  - Delete removes the directory and the role page, and the agent can be restored from its checkpoint.
  - A local A2A caller with a key reaches each agent's card and can run a task on it. A private agent returns `404` to that caller.

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
  - **relay repository:** tunnel protocol (the `agents` capability and `AgentsUpdate` frame), database schema for instance agents, A2A routing by instance and agent (replacing the per-instance route), directory entries;
  - **residuum:** tunnel client (declaring `agents` and `a2a` again, sending `AgentsUpdate` on changes), dispatch of relay-forwarded A2A requests by the frame's `agent` field, relay-based public URLs, sibling discovery (once per hub, fanned out to every agent, excluding teammates, `<instance>/<agent>` names).
- **Preconditions:** Phase 3. It is independent of Phases 4 and 5.
- **Shape when done:**
  - **Relay.** Relay changes are deployed first. Released clients' web UI tunneling is unaffected.
  - **Declaring agents.** A hub declares `agents` and keeps the relay's agent list current across create, delete and visibility changes, and after every reconnect.
  - **Routing.** Each agent is reachable at `{origin}/a2a/{instance}/{agent}` with its own card and visibility. The old per-instance A2A route is gone.
  - **Discovery.** The directory lists per-agent entries. Sibling discovery excludes teammates.
- **Verification:**
  - Relay tests: a client without `agents` still tunnels the web UI; `AgentsUpdate` replaces the list idempotently; routing by instance and agent works; private agents are hidden from non-siblings.
  - Residuum integration test against a local relay: two agents in one hub, each reachable by URL with its own card, one set private and hidden from a non-sibling key holder.
  - A second hub discovers both agents as siblings. The hub's own agents never list each other as siblings.

## Phase 7 — Documentation and clients

- **Modules:**
  - `docs/systems-usage/` (a new hub and teams document, plus updates to config, background tasks, A2A, wiki, workbench, skills and cloud tunnel);
  - bundled `residuum-system` skill references;
  - Docker docs;
  - archiving this design.
- **Preconditions:** Phases 1–6.
- **Shape when done:**
  - Systems-usage docs and bundled references describe the hub as built.
  - Docker docs match.
  - This design and its phases live in `docs/archive/`.
  - Issue #205 is closed.
- **Verification:**
  - Run doc-sync and find no drift between code and docs.
  - The design doc's goals hold end to end on a real install: switch agents, hand work off, agent-created agent, shared wiki and workbench, per-agent A2A entries.
