# Multi-Agent Hub — Design

**Status:** Built and shipped; this record matches the implementation. Tracks issue #205. The implementation phases are in [phases.md](phases.md). Current behavior is described in `docs/systems-usage/`, starting with [Hub](../../systems-usage/hub.md).

> Systems level only. This document stands on its own: it is implemented in fresh sessions that have only this doc, `phases.md`, and the codebase.

## Goal & context

Residuum runs one agent per process. A user who wants several agents today has to run several processes and relay instances, and link them through A2A. A2A is built for handing work to *remote* agents. It is the wrong tool for a user's own agents working as a team.

This design has one Residuum process host any number of **durable peer agents** that work as a team:

- **Easy switching.** One web UI with an agent switcher, one local port, one relay connection.
- **Teamwork.** Agents hand work to each other directly with `message_agent`, inside the process, with no relay or A2A hop.
- **Cheap agent creation.** The user can create agents, and so can any existing agent. A new agent can be given a role description that it turns into its own notes on first start.
- **A shared team layer.** All agents share the user profile, the team rules, the knowledge wiki, the workbench, and team skills. Each agent keeps its own identity, memory, model config, adapters, pulses and personal skills.

Agents are peers. Residuum has no lead agent. A user may choose to talk mostly to one agent, but that is the user's choice, not something the product encodes.

### Terms

- **Hub.** The single Residuum process, and the hub-level state it owns: the HTTP server, relay tunnel, A2A listener, workbench server, tracing, updater, secrets, key stores, and the shared background concurrency budget.
- **Agent.** A durable identity with its own directory, config, memory, sessions, adapters and event loop. It is hosted by the hub. Its name is its identity everywhere: the directory name, its address prefix, its A2A entry, and its URL segment.
- **Team layer.** The files every agent shares, under `~/.residuum/team/`.
- **Teammate.** Another agent in the same hub.
- **Session.** Unchanged from today: a temporary fork of *one* agent (spawned, scheduled, external, artifact). A session belongs to exactly one agent.

## Shape

### On-disk layout

```
~/.residuum/
├── hub/                       # hub-level state; never an agent's workspace
│   ├── config.toml            # hub config (see Config split)
│   ├── secrets.toml.enc, secrets.key
│   ├── agent-keys.toml.enc, agent-keys.key
│   ├── a2a-keys.toml, a2a-keys.lock
│   ├── logs/, bin/, checkpoints/
│   ├── residuum.pid, residuum.lock, residuum.ready, update markers
│   └── *.last-known-good.toml
├── team/                      # shared team layer
│   ├── AGENTS.md              # team-wide rules
│   ├── USER.md                # the user's core facts
│   ├── wiki/                  # the OKF wiki, including wiki/agents/<name>.md role pages
│   ├── workbench/             # shared artifacts
│   ├── skills/                # team skills (bundled skills are installed here)
│   └── .index/, vectors.db    # the team wiki's search index (hidden, like memory indexes)
└── <agent-name>/              # one directory per agent; the agent's workspace root
    ├── config/                # config.toml, providers.toml, channels.toml, mcp.json, agent-card.json, a2a.json
    ├── SOUL.md, HEARTBEAT.yml, SUBCONSCIOUS.md
    ├── memory/, skills/, inbox/, archive/, a2a/
    └── per-agent state files (pulse, adapters, scheduled actions)
```

- A directory under `~/.residuum/` is an agent if it holds `config/config.toml`. The hub discovers agents by scanning, so it keeps no separate registry to drift out of sync.
- **Agent names** follow the relay's slug rules, because the name is also the agent's A2A path segment: 1–24 characters from `[a-z0-9-]`, with no leading or trailing hyphen.
  - Reserved names: `hub`, `team`, `agents` (reserved by the relay), and any name starting with `.`.
  - Agents are never renamed. The name is baked into addresses, role pages and remote callers' URLs, so renaming is out of scope.

### The team namespace in an agent's view

Each agent's file tools, the web file browser and the change feed see one logical tree: the agent's own directory, plus a `team/` prefix that maps onto `~/.residuum/team/`.

- An agent writes `team/wiki/people/sam.md` and reads its own `SOUL.md` without knowing the absolute layout.
- An agent directory can never contain its own `team` entry, so the prefix is unambiguous.
- Absolute paths keep working.
- A teammate's directory is reachable only by absolute path. Agents share files by handing them over (copy or move through the team area, or by absolute path). The system does not add cross-agent file APIs.

### Config split

**Hub config (`hub/config.toml`):**
- `timezone` (the user's timezone, shared by every agent)
- `[gateway]`: HTTP bind and port
- `[cloud]`: the relay tunnel
- `[a2a]`: listener `enabled`, `port`, `public_url`
- `[tracing]`: log level, OTEL, error reporting
- `[update]`
- `[background] max_concurrent`: the hub-wide session budget
- `[background]` hop soft and hard limits. These are hub-level because one message chain can cross several agents and needs one limit.

**Agent config (`<agent>/config/config.toml`):** every other section that exists today.
- Memory, pulse, subconscious, learning, idle, retry, tools, skills directories, web search, webhooks, agent abilities.
- Adapters: `[discord]`, `[telegram]`, `[teams]`.
- `[background]` per-agent knobs: idle timeouts, depth cap.
- Top-level model parameters: `timeout_secs`, `max_tokens`, `temperature`, `thinking`.
- `autostart` (default `true`).
- `[a2a] visibility` (`public` or `private`). A2A visibility is per agent.
  - An agent-created agent copies its creator's visibility.
  - A user-created agent gets the visibility the user picks, defaulting to `private`.
  - The onboarded first agent defaults to `public`, as today.
  - Either way the only unauthenticated thing a public agent exposes is its Agent Card. Every other A2A route requires a caller key or a sibling attestation, and a private agent answers `404` to a caller with neither.

**Other agent files:** each agent's `providers.toml` (providers and model assignments, including background tiers) lives in its `config/` directory. `channels.toml`, `mcp.json`, `agent-card.json` and `a2a.json` stay there too.

**Fields that no longer exist:**
- The top-level `name` field (the user's name). The user's name lives in `team/USER.md`.
- `workspace_dir`. The agent's directory *is* its workspace.

**Validation and last-known-good:**
- Both kinds of config are validated and hot-reloaded as today: polling, debounce, and a last-known-good fallback kept per file.
- If a hub config reload fails, the hub keeps running on its last-known-good config.
- If an agent's config fails to load at start, that agent enters `failed` with the error. The other agents are unaffected.
- A config that assigns the same port to two agents' Teams adapters is a validation error on the second agent.

**Environment overrides:**
- Hub-level overrides keep working: gateway bind and port, cloud token, timezone.
- Agent-scoped overrides are removed: model, provider URL, API key, observer and reflector overrides, channel tokens, and the workspace override. In a multi-agent process they would silently apply the same value to every agent (for example, one Discord token for all). A removed override that is still set produces a startup notice naming it.

**Secrets** are hub-level and shared. Every agent resolves `secret:<name>` references against the one store. Per-agent credentials are separate secrets with separate names.

**Agent keys** (credentials injected into the commands and MCP servers an agent runs) are hub-level and shared the same way. One store in `hub/`, visible to every agent. A key's `creator` becomes `user` or `agent:<name>`. When a notice fires on an overwrite or delete, it names the acting agent.

### Runtime components

```
Hub (process)
├── HTTP server ── /api/hub/*, /api/team/*, /api/agents/<name>/*  (+ SPA assets)
├── Relay tunnel (one connection, declares its agents)
├── A2A listener (one port, routes by agent)
├── Workbench server (serves team/workbench)
├── Team services: team change feed, team wiki index, team write coordinator
├── Shared session budget (one semaphore)
├── Tracing, updater, secrets, key stores
└── Agent host ── name → AgentRuntime
                    ├── own bus, event loop, agent, memory, sessions, messenger, router
                    ├── own adapters (Discord/Telegram/Teams/webhooks), pulse, subconscious, MCP
                    └── own workspace watcher and skill index
```

**Agent host.** It owns the set of agents and their lifecycle:
- **States:** `starting`, `running`, `stopped`, `failed` (with the last error and when it happened).
- **Operations:** start, stop, restart, create, delete.
- **Startup:** agents with `autostart = true` start at hub startup.
- **Failure isolation:** each agent's event loop runs as its own task. A panic or fatal error in one agent moves only that agent to `failed`. The hub and the other agents keep running. A `failed` agent restarts only when someone restarts it.

**AgentRuntime.** This is today's gateway runtime minus the process-scoped parts: the HTTP server, tunnel, A2A listener, workbench server, tracing, updater and secrets. Nothing in an agent depends on process-wide mutable state:
- **No working-directory changes.** Every relative path, and every child process an agent spawns (exec, MCP servers), is resolved or started against that agent's directory explicitly.
- **Hub-level log level.** There is one log level for the whole hub. Every log line and span an agent produces carries an `agent` field.
- **Explicit hub services.** Anything an agent reads from the hub (tunnel status, secrets, the shared budget, team services) is passed in as an explicit handle.

**Shared session budget.** One semaphore, sized by the hub's `[background] max_concurrent`, is shared by every agent's session runtime.
- Main turns still don't take a permit. Only session turns do, as today.
- A session waiting for a permit shows as `Queued` in its agent's session list.

### Addressing and teamwork

**Address forms** (all string-parsed like today's `a2a:` prefix):

| Form | Means |
|---|---|
| `main`, `<session-address>` | This agent's main, or one of its own sessions. Unchanged. |
| `agent:<name>` | Teammate `<name>`'s main. |
| `agent:<name>/<session-address>` | A session belonging to teammate `<name>`. Used mainly to reply to a teammate's session that messaged you. |
| `a2a:<name>` | A remote A2A agent. Unchanged, but see A2A below for how sibling names change. |

**Cross-agent delivery.** `message_agent` with an `agent:` address goes to the hub's team router. The team router registers each running agent's messenger at start and unregisters it when the agent's stop begins. It checks the target's state against the agent host and hands the message to the target agent's messenger, which delivers it exactly as today:
- to main: a mid-turn interrupt or a new turn;
- to a live session: an interrupt;
- to a completed session: a resume.

**Attribution and replies.**
- The receiver sees the message attributed to the sender's fully qualified address: `agent:<sender>` for a sender's main, or `agent:<sender>/<session>` for a session. It is labeled as coming from a teammate, not from an external caller.
- Replying is the same as today: the receiver calls `message_agent` back to that address. Delivery stays fire-and-forget.

**Hop counting across agents.** The hop counter travels with the message across agents. The hub's soft and hard limits (defaults 8 and 32) are checked at every hop against the chain's count, so a loop that bounces between two agents is caught the same way as a loop within one.

**Delivery failures are visible.** The sender gets a tool error it can act on or report in these cases:
- the teammate is `stopped` or `failed` (the error says so, and says the user can start it);
- the name doesn't exist;
- the target session doesn't exist.

Nothing queues for a stopped agent.

**`list_agents`** returns, in addition to what it returns today, every teammate with:
- its name;
- its state;
- its role line, taken from the `description` in its wiki role page.

**`stop_agent`** is unchanged. It covers the agent's own sessions and its A2A tasks. Teammates are stopped through the lifecycle controls.

**Team roster in the prompt.** Each agent's prompt includes a short `TEAM` block: every teammate's name, state and role line. That lets an agent know who to hand work to without spending a tool call. The block is built from the same data as `list_agents`, lists teammates only, and is omitted when the agent has none:

```
<TEAM>
You are "alpha". Teammates (message_agent to="agent:<name>"; only running ones receive):
- beta (running): Reviews drafts
- gamma (stopped): no role line yet
</TEAM>
```

### Agent lifecycle and creation

**Who can create an agent:**
- the user, from the web UI team view or with `residuum agent create <name> [--description ...]`;
- any agent, with the `agent_create` tool (`name`, optional `description`).

The CLI commands talk to the running hub. Creating an agent needs a running hub.

**Model config for a new agent:**
- Created by an agent: a copy of the creator's `providers.toml`.
- Created by the user: the model config the user picks. The form starts from a copy of an existing agent's.

Everything else starts from the blank agent template.

**Creation, in order:**
1. Validate the name. Refuse if a directory with that name already exists.
2. Write the agent directory from the blank template:
   - a default `SOUL.md` that names the agent;
   - default `HEARTBEAT.yml` (without `wiki_lint`) and `SUBCONSCIOUS.md`;
   - empty memory and inbox;
   - config with `autostart = true`.

   The model config is validated first, and the directory is assembled under a hidden staging directory (`.provision-<name>/`) and renamed into place only when complete, so a failed creation never leaves a discoverable half-agent.

   No `BOOTSTRAP.md` is written, and the bootstrapped marker is set. New agents never run the first-run interview.
3. Write the agent's role page `team/wiki/agents/<name>.md` (see Wiki). If a description was given, it becomes the page's `description`, otherwise a placeholder. Add the page's entry to `team/wiki/agents/index.md`, and append a line to the wiki log.
4. Start the agent. If it can't start, it is still created and reported `failed` with the reason.
5. If a description was given, deliver it to the new agent's main as its first message, attributed to the creator (`agent:<creator>` or the owner). The message asks the agent to:
   - turn the description into its own notes in `SOUL.md`;
   - fill in its role page with its role and responsibilities.
6. Publish an `agent_created` event naming who created it. The web UI shows it as a toast and the agent appears in the switcher. When another agent created it, that agent also gets an item in its user inbox.

**Deletion.** The user (UI or CLI) or any agent (`agent_delete` tool) can delete an agent. There is no approval gate. Deletion:
1. stops the agent;
2. takes a checkpoint of its directory and of its two config files;
3. renames the directory to a hidden `.deleting-<name>/` (which takes it out of discovery in one step) and removes it;
4. removes its role page and index entry, and logs it to the wiki log;
5. publishes an `agent_deleted` event naming who deleted it.

The agent's checkpoint history stays in `hub/checkpoints/agents/<name>/`, and checkpoints are never pruned. An agent that deletes itself starts the delete on its own task and returns at once, because stopping the agent cancels the running turn. Recreating an agent with the same name reuses the same checkpoint history.

**Start and stop.** Starting and stopping are runtime operations, available from the UI, the CLI and the hub API. Stopping an agent:
- ends its adapters and event loop;
- cancels its live sessions, which are recorded as interrupted the same way a restart records them.

Whether an agent starts with the hub is controlled by its `autostart` flag.

### Team layer

**Prompt assembly** keeps today's order. Each file now comes from a named source:

| Order | Block | Source |
|---|---|---|
| 1 | SOUL.md | the agent |
| 2 | AGENTS.md | team |
| 3 | HARNESS | constant |
| 4 | USER.md | team |
| 5 | WIKI_INDEX | team |
| 6 | TEAM roster | hub |
| 7 | OBSERVATION_LOG, RECENT_CONTEXT | the agent |
| 8 | SKILLS_INDEX, ACTIVE_SKILLS | layered, see Skills |

`BOOTSTRAP.md` is read from the agent's directory while it exists. The onboarded first agent has one until its interview finishes; created agents never do. The subconscious classifier reads the same sources.

**Wiki.**
- There is one OKF wiki, at `team/wiki/`, and no per-agent wikis. Every agent keeps all its durable knowledge there, including specialist knowledge. OKF's index structure lets each agent read only what it needs.
- `team/wiki/agents/` holds one role page per agent: frontmatter `type: Agent`, plus a `description` holding the one-line role. Its owning agent maintains it, describing its role, responsibilities and what to hand it. `team/wiki/agents/index.md` is the roster catalog.
- The hub keeps one search index for the team wiki: a tantivy index plus vectors, hidden like memory indexes.
- Each agent's memory search runs the existing hybrid pipeline (normalize, weighted merge, decay, `min_score`) separately against its own memory index and against the team wiki index, using the agent's `[memory.search]` settings for both.
- It then merges the two result lists by score into one ranking and applies the limit once.
- Wiki result IDs are `team/wiki/...` paths, ready for `read_file`. `source=wiki` searches only the team index.
- Agent memory indexes no longer contain wiki pages.

**Team write coordination.** Every write under `team/`, whether from an agent's file tools or the web UI, goes through the hub's team write coordinator:
- **Version tracking.** A file tool records the file's version (the same mtime-based token the web workspace API uses) when the tool reads it.
- **Conflict check.** Writes and edits to a team path take a per-path lock, compare the file's current version with the version this tool instance last read, and only write when they match.
- **Conflict.** If they differ, the write is refused with an error. The error names the path, says it was changed since the agent read it (by teammate X, the user, or an unknown writer), and tells the agent to read it again and reapply its change.
- **Atomic writes.** Team files are written atomically (temp file plus rename).
- **Web UI.** Web UI writes keep their `If-Match` precondition, checked under the same lock.
- **Scope.** Agent-private files keep today's behavior.

**Skills** are layered, and the first match by name wins:
1. the agent's own `skills/`;
2. `team/skills/`;
3. any `[skills] dirs` from the agent's config.

Bundled skills are installed into `team/skills/` when missing, the same "write if missing" behavior as today. A shadowed skill is logged at debug. The skills index, and the web UI's skill listing, show which layer each skill came from.

**Workbench.**
- The workbench lives in `team/workbench/`. The hub's single workbench server serves it, and the web UI lists it at the team level.
- **Artifact sessions name their agent explicitly.** The artifact SDK's session start takes a required agent name, and the session runs as a fork of that agent. A request without an agent, or naming an agent that isn't running, is refused with an error the artifact can show.
- Artifact state files and the change feed work as today, relative to the team root.

**Change feeds.**
- The hub watches `team/`, and each agent watches its own directory.
- A client that watches prefixes in an agent's namespace gets both feeds, merged, with `team/`-prefixed paths for team changes.
- The workbench reload watcher follows the team feed.

### HTTP surface

Everything the backend serves lives under `/api/`. The SPA fallback serves every non-`/api` path, as today.

| Prefix | Contents |
|---|---|
| `/api/hub/...` | Hub status, agent lifecycle (list, create, delete, start, stop, restart), hub config, secrets, cloud, update, tracing, shutdown, checkpoints for hub and team, and the hub WebSocket (`/api/hub/ws`). The hub WebSocket carries agent state changes, created and deleted notices, per-agent activity and unread counts, and the team change feed. |
| `/api/team/...` | Team files (the workspace file API scoped to `team/`), workbench info and artifacts. |
| `/api/agents/<name>/...` | Everything agent-scoped from today's API: agent config, providers, MCP, channels, sessions, scheduled actions, inbox, memory search, the agent's A2A settings, its workspace files. It also carries the agent's WebSocket (`/api/agents/<name>/ws`), which carries today's WS protocol unchanged. |

- Agents are created and deleted at runtime, so `/api/agents/<name>/` is resolved per request against the agent host. A request for an unknown agent gets `404`. A request for an agent that isn't running gets `409` with its state, except for the config and file routes, which work on a stopped agent so the user can fix a `failed` agent's config.
- The cross-site guard and the remote-control guard apply as before. Hub shutdown and cloud disconnect are remote-control guarded. Stopping *all* agents (`POST /api/hub/stop-all`) is not: the hub keeps running and every agent can be started again through the tunnel.

### Web UI

- **Agent switcher.** Always visible. It lists every agent with a state dot and an activity or unread indicator, fed by the hub WebSocket. Selecting an agent changes the URL.
- **Routes:**
  - `/agent/<name>/...` for agent pages: chat, sessions, scheduled, workspace, agent settings.
  - `/team/...` for team pages: the team view, the workbench, team files, hub settings.
  - `/` redirects to the last-used agent, kept in local storage, or else the first agent.
- **Team view.** Every agent with its state, last error, and counters. Controls to create (name, optional description, model config from a chosen agent), start, stop, restart and delete, plus the `autostart` toggle.
- **Settings split.** Hub settings (gateway, cloud, A2A listener, tracing, update, secrets, session budget) live under `/team/settings`. Agent settings (models, adapters, pulses, memory, skills, A2A visibility) live under `/agent/<name>/settings`.
- **Counters.** Each agent shows its current turn's elapsed time, tool-call count and tokens. Its sessions show the same. The tool-call count is new: turns start counting and exposing tool calls, which today are only tracked internally.

### Onboarding

The setup wizard (web and CLI) asks for:
- the user's name, written to `team/USER.md`;
- the first agent's name, validated as above;
- timezone, written to hub config;
- provider, API key (stored as a secret) and model, written to the first agent's `providers.toml`;
- the web search backend.

The first agent is written from the blank template, *with* `BOOTSTRAP.md`, so it runs the getting-started interview. The team layer is created with default `AGENTS.md`, `USER.md`, wiki skeleton and bundled skills.

### Hub notices

Some events belong to the hub rather than to one agent's background work: an agent created, deleted, or `failed`. These go on a hub-level bus, which is separate from each agent's own broker and is also what feeds the hub WebSocket. Each notice is delivered:
- as a web UI toast through the hub WebSocket;
- as a user inbox item in the inbox of the agent that acted (the creator or deleter), or of the affected agent for `failed`. When the user acted through the UI or CLI, the notice goes to the toast only.

Hub notices never go to push channels (`channels.toml`), because those are per agent and a hub event would otherwise fire once per agent.

Teammate delivery failures and team write conflicts are not notices. They are tool errors returned to the agent that hit them.

### Visibility and intervention

**What the user and agents can see:**
- each agent's state, and the last error of a `failed` agent;
- per-agent counters for turns, tool calls and tokens;
- per-agent activity in the switcher;
- created and deleted notices, naming who did it;
- teammate states in every agent's `TEAM` block and in `list_agents`;
- a teammate delivery failure, returned as a tool error;
- a team write conflict, returned as a tool error, and logged at info with both writers named;
- log lines tagged with their agent, filterable with `residuum logs --agent <name>`.

**How things degrade:**
- A failed agent never takes down the hub or its teammates.
- A bad agent config fails only that agent. A bad hub config reload keeps the last-known-good config.
- A stopped teammate turns messages to it into visible errors, not silent drops.

**How the user steps in or undoes:**
- stop or restart any agent from the UI or CLI;
- stop any turn or session with the existing controls;
- restore a deleted agent from its checkpoint history;
- restore team files from the team checkpoint repo;
- rely on the hop limits across agent chains.

## Reasoning & alternatives

- **Agents in one process instead of supervised child processes.** The in-process pieces already exist: the bus, the messenger, sessions and the concurrency budget. Teamwork is direct delivery between runtimes, not HTTP proxying between processes. A supervisor, reverse proxy and readiness protocol would be new machinery that one process doesn't need.
  - *Cost accepted:* isolation is at the task level. Task boundaries contain a panicking agent, but a hung blocking call or a memory blowup affects the whole process.
  - The earlier proposal to use child processes (`docs/archive/hub-architecture-design.md`) assumed per-agent daemons already existed. They no longer do.
- **Teammates talk directly, not over A2A.** A2A is an interop protocol for remote agents. It brings task objects, cards, auth and relay hops. Teammates need the same delivery semantics as sessions within one agent, so the team router reuses the messenger. A2A stays for agents outside the hub.
- **Peers, not a lead agent.** A lead would build a routing policy into the product. The user can already get one by talking mostly to one agent.
- **Directory scan instead of a registry file.** An agent is its files. Moving an agent's directory in or out is enough to add or remove it, and there is nothing to drift. The previous registry-file design was removed.
- **A `hub/` directory instead of reserved agent names.** Putting hub files in one directory reserves two names (`hub`, `team`) instead of a list that grows every time a new hub-level file is added.
- **The team wiki replaces private wikis.** OKF is already built for selective reading. One wiki means knowledge one agent learns about the user is available to all of them.
- **Optimistic write check instead of a held lock or merge.** It matches how the web API already handles concurrent edits. A losing writer rereads and reapplies, which an LLM agent does well, and no lock is ever held across a model call.
- **A per-agent A2A entry under the hub's instance, instead of one relay instance per agent.** The relay's instance stays the unit of browser access and tunnel connection (one per hub). Agents become entries *within* an instance. That keeps one tunnel, keeps the relay's instance switcher meaning "which machine," and gives each agent its own A2A identity and visibility.
- **Hub-wide timezone and log level.** Both are properties of the user or the process, not of an agent. A per-agent log level can't work with one global tracing filter.
- **Removed agent-scoped environment overrides.** With several agents, each of these overrides has no agent to target and would apply to all of them.

## External touchpoints

### Relay (separate repository, in production)

The relay's instance stays the hub. The relay gains **agents within an instance**.

Compatibility: no released Residuum client has used A2A, so the relay's A2A routes and directory change shape outright, with no legacy path. Released clients do use the tunnel for web UI access. That proxying (HTTP and WebSocket forwarding, the `Connected` frame, token auth) is unchanged, and the new frame is only sent by clients that declare the new capability.

- **Handshake.** A hub declares the `agents` capability in `x-residuum-capabilities`, next to the existing ones.
- **New frame, client to relay: `AgentsUpdate`.** It carries `agents: [{ name, display_name, a2a_enabled, a2a_private }]`, with the full list each time.
  - Sent after `Connected`, and again whenever an agent is created, deleted, started or stopped, or changes A2A visibility, or the hub's `[a2a] enabled` flips. `a2a_enabled` is true only while the agent is running and the hub's A2A listener is enabled.
  - The relay replaces its stored list for that instance in one transaction.
  - It is idempotent, so resending after a reconnect is always safe.
- **Storage.** A per-instance agent table (instance, name, display name, A2A enabled, A2A private), unique on instance plus name.
- **A2A routing.**
  - `ANY /a2a/{instance}/{agent}[/{rest}]` replaces the per-instance `/a2a/{slug}` route and routes to that instance's tunnel. The `HttpRequest` frame gains an optional `agent` field, set by the relay for this route, and the hub's A2A listener dispatches on it.
  - An agent that isn't in the stored list, or has A2A disabled, gets `404`. A private agent is listed only for the user's own installs (or a caller its own auth-check accepts), and the relay forwards every request to the hub, whose auth layer answers `404` to a caller with neither a key nor a sibling attestation.
- **Directory (`GET /a2a/agents`).**
  - It lists one entry per agent: `{ slug: "<instance>/<agent>", instance, agent, display_name, card_url, online }`, applying each agent's privacy the way instance privacy is applied today.
- **Sibling attestation.** It stays per connection, meaning per hub. `x-residuum-sibling` names the calling *instance*, and callers are attributed as "your own other Residuum instance," as today.
- **Browser and workbench proxying.** No change. They follow the active instance, which is the hub.
- **Errors and ordering.**
  - An A2A request that arrives before the first `AgentsUpdate` gets `503` with a retry hint.
  - An `AgentsUpdate` from a client that didn't declare `agents` is ignored and logged.
  - An instance that never sent `AgentsUpdate` (a released client without A2A) has no A2A entries, and its A2A paths return `404`.

### Residuum's A2A server and client

- **Listener.** One A2A listener per hub, on the hub's `[a2a]` port, routing by agent:
  - relay-forwarded requests use the frame's `agent` field;
  - local requests use a path prefix, `/agents/<name>/...`.
- **Per-agent identity.** Each agent has its own Agent Card (`<agent>/config/agent-card.json`) and its own public URL:
  - `public_url` plus `/agents/<name>`, if `public_url` is set;
  - otherwise `{origin}/a2a/{instance}/{name}` when the tunnel is connected;
  - otherwise the local fallback plus `/agents/<name>`.
- **Caller keys and the tunnel.** Caller keys stay in one hub-level store, and a valid key reaches every agent, private ones included. A private agent is hidden only from callers with no key and no sibling attestation. The tunnel nonce stays per process.
- **Remote tasks.** An inbound A2A task becomes a conversation session on the target agent.
- **Siblings.** Discovery runs once per hub, and every agent's A2A client registry gets the result. Discovery filters out the hub's own agents, since those are teammates and reachable directly. Remote siblings are named `<instance>/<agent>` in `a2a:` addresses.
- **Root paths.** The listener has no root-level agent. Anything outside `/agents/<name>/` gets `404`.
- **Per-agent client config.** `config/a2a.json`, the remote task tracker and outbound tasks stay per agent.

### Other clients

- **Mac app.** Its multi-agent code is removed: the registry file it reads no longer exists, and so do its per-port agent tabs. It keeps one connection, pointed at one agent's `/api/agents/<name>/ws`. Agent switching lives in the web UI.
- **Docker.** The volume still mounts `~/.residuum`. Docker documentation changes for the removed environment overrides and the new layout.
- **OTEL export.** It carries the `agent` attribute on every span.

## Integration with existing system

**Replaced:**
- the single-workspace config root;
- the single `GatewayRuntime` as the process;
- root-level API and WebSocket paths;
- the per-workspace wiki, workbench, `AGENTS.md` and `USER.md`;
- agent-scoped environment overrides;
- the top-level `name` config field;
- the single `TunnelStatus` instance identity, as far as A2A URLs go.

**Wrapped or reused unchanged:**
- session runtime, messenger delivery, conversation routing;
- adapters, pulses, subconscious, memory, observer and reflector;
- checkpoints, which keep the split between a workspace-style repo and a local-only config repo, all under `hub/checkpoints/`:
  - **hub config repo** (`hub-config.git`, local-only): hub config, the secrets store, the agent-key store, A2A keys;
  - **team repo** (`team.git`, workspace-style): `team/`;
  - per agent, under `agents/<name>/`, a **workspace repo** (`workspace.git`, workspace-style), which is the agent directory minus its `config/config.toml` and `config/providers.toml`;
  - per agent, under `agents/<name>/`, a **config repo** (`agent-config.git`, local-only), which is exactly those two files.

  A remote can never be added to the local-only repos, because config files can hold plaintext keys.
- last-known-good;
- the existing WebSocket protocol, now served per agent.

**Existing installs.** There is no migration. The hub reads only the new layout, and an install with nothing under `hub/` goes through onboarding. Any files from the old single-agent layout are ignored.

**Documentation.**
- `docs/systems-usage/` gains a hub and teams document.
- `config.md`, `background-tasks.md`, `a2a.md`, `wiki.md`, `workbench.md`, `skills.md` and `cloud-tunnel.md` are updated.
- The bundled `residuum-system` skill references are updated to match.
- This design moves to `docs/archive/` once built.

## Pulses on a shared wiki

The weekly `wiki_lint` pulse checks the shared wiki, so only one agent runs it. The onboarded first agent's `HEARTBEAT.yml` includes it. The blank template for created agents includes `memory_tending`, which files an agent's own memory into the wiki, but not `wiki_lint`. Moving `wiki_lint` to another agent is a matter of editing both agents' `HEARTBEAT.yml`.
