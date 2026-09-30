# Hub

One Residuum process, the **hub**, hosts every agent under `~/.residuum/`. The hub owns what exists once per process. Each agent owns everything that is its own.

| Hub (one per process) | Agent (one per agent) |
|---|---|
| HTTP server, relay tunnel, A2A listener, workbench server | Message bus, event loop, and the agent itself |
| Tracing, log level, updater, auto error reports | Memory, sessions, messenger, pulses, subconscious |
| Secrets and the agent-key and A2A-key stores | Discord, Telegram, Teams and webhook adapters |
| Team write coordinator, team wiki index, shared checkpoint repositories, team router | MCP servers, skills, workspace watcher |
| Session budget (`[background] max_concurrent`) | Its own `config.toml` and `providers.toml` |

Each agent loads its own config against the hub config, and keeps its own last-known-good copies, so a bad agent config fails only that agent, and a bad hub config reload keeps the hub on its last-known-good config.

## Start-up and shutdown

`residuum serve` loads the hub config, starts the servers, builds the shared services, and scans `~/.residuum/` for agents (any directory holding `config/config.toml`). Every agent whose `autostart` is on (the default) then starts, concurrently. The readiness marker `hub/residuum.ready` is written once the servers are bound and every autostart agent has either started or been recorded as failed. 

With no agent on disk the hub still starts and serves the web app, which runs onboarding. When `POST /api/hub/config/complete-setup` has written the first agent, the hub applies the hub config it wrote, rescans, and starts the agent, with no process restart. `residuum serve --setup` runs the same hub on an empty temporary root. `residuum setup` writes the same files from the command line, before or without a running hub.

Stopping the hub (SIGTERM, `POST /api/hub/shutdown`, or a restart for an update) first stops accepting starts, then stops every agent gracefully, then stops the servers. From the moment shutdown begins, `start`, `restart`, and `create` are refused with the error `Residuum is shutting down` (HTTP `500`, like other lifecycle failures), so a request arriving during the shutdown can't start an agent that nothing would stop. Stopping an agent still works.

## Agent states

| State | Meaning |
|---|---|
| `starting` | The runtime is being built. |
| `running` | Adapters, the event loop, and HTTP routes are live. |
| `stopped` | Not running, by choice: never started, or stopped. |
| `failed` | Start-up failed, or the event loop died. Carries the plain-language error and when it happened. |

An agent's event loop runs as its own task. A panic or fatal error in it moves that agent, and only that agent, to `failed`. The hub and the other agents keep running. The failure is logged at `error` with the agent's name, auto-reported through `TracingService::on_error` (when auto error reporting is on; the report's text names the agent), announced on the hub bus as an `agent_state` event, and left as an item in the failed agent's user inbox. The agent's live sessions are recorded as interrupted and its adapters and MCP servers are stopped. A `failed` agent starts again only when someone starts or restarts it.

## Operations

- **start**: does nothing for an agent that is already `running` or `starting`. A start-up failure leaves the agent `failed` and is returned to the caller.
- **stop**: ends the agent's adapters and event loop and cancels its live sessions, which are recorded as interrupted the way a process restart records them. A turn in progress is stopped the way a user stop stops it. If the agent hasn't wound down after 150 seconds its task is aborted.
- **restart**: stop, then start.
- **patch**: writes `autostart` and/or `[a2a] visibility` to the agent's `config/config.toml` with the same in-place edit the Settings page uses, checkpointing the agent's config repository first. The patched file is validated before anything is written. A running agent is then reloaded, and the call returns once the reload has finished (or after 30 seconds).
- **create** and **delete**: see [Agent creation and deletion](agent-lifecycle.md). Creation starts the agent and, when a description was given, hands it to the new agent's main conversation as its first message (from the owner for the user, from the creating agent's address otherwise). If the agent can't start, it is still created and returned `failed`. Deletion stops the agent first. An agent that creates or deletes another finds an item about it in its user inbox; the user acting through the web UI or CLI sees the hub event only.

Two agents can't run with the same Teams adapter port: the second one to start is `failed` with a message naming the agent that holds the port. An agent reserves its port when it begins to start, so agents starting at the same moment can't both take it; the reservation is released when the agent stops, fails, crashes, or is deleted, and moves to the new port when a reload changes `[teams] port`. If the Teams adapter can't bind its port (another program or agent holds it), the hub publishes a `notice` naming the agent and the port, both at start and after a reload.

Operations on one agent are serialized. Once an agent is deleted, any `start`, `stop`, `restart`, or `patch` that was already waiting on it answers `404`; a deleted agent's directory is never recreated.

The `autostart` and `a2a_visibility` an agent's summary reports come from the last `config/config.toml` that loaded. For a running agent that is the config it last loaded or reloaded; for a stopped one, the file as read on each request. A file that can't be read or parsed, or that was emptied after a config had loaded, leaves the last loaded values in place and logs a warning once. An agent whose config has never loaded is reported `private`, so the A2A listener never serves the card of an agent whose visibility can't be established without a key.

## Shared services

- **Team write coordinator**: one per hub; each agent takes its own view with its name.
- **Team wiki index**: one instance for the hub, opened with the embedding model of the first agent (by name) that configures one, or text-only when none does. Every agent's memory search holds a clone of the same handle. The embedder is re-evaluated when an agent starts, finishes a config reload, or is deleted: when the choice changes (the first agent with an embedding model appears, the providing agent's embedding config changes, or it stops providing one), the same instance swaps its embedder in place, clears its vector store if the model differs, and refills it from the wiki pages on the next search. Every agent's searcher sees the new embedder at once. If the new embedder can't be built or its vector store can't be opened, wiki search stays text-only and the hub publishes a `notice`.
- **Team router**: one per hub. Each running agent registers its messenger at start and unregisters when its stop begins. A `message_agent` call to `agent:<name>` (or `agent:<name>/<session>`) goes to the router, which checks the target's state against the agent host and hands the message to the target's messenger; nothing queues for an agent that isn't running. The router also supplies the teammate roster that `list_agents` and every agent's `TEAM` prompt block show. See [Teammates](background-tasks.md#teammates).
- **Session budget**: one semaphore sized by the hub's `[background] max_concurrent`, taken by every agent's session turns. Main turns don't take a permit. A session waiting for a permit shows as `queued` in its agent's session list. Changing `max_concurrent` takes effect on the next restart.
- **Checkpoints**: the team and hub-config repositories are shared; each agent has its own workspace and config repositories. See [Checkpoints](checkpoints.md).
- **Tunnel status, secrets, key stores, tracing**: one of each, passed to every agent.
- **Team change feed**: one watcher over the team directory, publishing `team/...` paths on its own bus. The hub WebSocket, every agent's `/ws` (for clients that watch `team/...` prefixes), and every agent's artifact reload watcher read it, so a change to a team file is watched once however many agents run. Each agent also has a feed over its own directory.

## Hub config reloads

`hub/config.toml` is watched. A change is applied where the hub owns it: a new `[gateway]` address rebinds the HTTP server (a failed bind keeps the current server), `[cloud]` restarts the tunnel, `[tracing]` updates the tracing service and the log level, and `[a2a]` restarts the A2A listener. Every running agent then reloads against the new hub config for what it reads from it (the timezone, its A2A card, and the hop limits).

The hub's reload queue carries two signals. `Hub` (from the config watcher, hub config writes, key stores, and the cloud settings) reloads the hub config as above. `Workspace`, sent when an edit through `/api/team/workspace/...` changes the team's `AGENTS.md` or `USER.md`, makes every running agent reload its workspace so it picks up the new identity files.

## Activity

The host tracks two things per agent for the switcher: `busy`, true while a main turn runs, and `unread`, the number of main-conversation replies published while no web client was connected to that agent's `/ws`. Connecting a client resets `unread` to zero. Changes are published on the hub bus as `agent_activity` events, which the hub WebSocket (`/api/hub/ws`) forwards to the web UI's agent switcher.

## Hub bus events

Every change to an agent's state, autostart, or visibility is published in order as an `agent_state` event carrying the agent's summary. `agent_created` and `agent_deleted` carry who did it (`user` or `agent:<name>`). `agent_activity` carries `busy` and `unread`. `notice` events carry hub-level messages: a fallback to the last-known-good hub config, hub config notices, removed environment overrides that are still set, and hub config reload outcomes.

## HTTP

One router serves everything the hub offers: the hub, team and per-agent routes, the hub WebSocket, and the web app. Agent routes are resolved per request under `/api/agents/{name}/`; an unknown agent answers `404` and an agent that isn't running answers `409`, except for its repair routes. No agent's router serves a root path. The routes, their layout and the request guards are in [Hub HTTP Surface](hub-http.md).

## Logging

There is one log level for the whole hub. Every log line and span an agent produces carries an `agent` field: the agent's tasks, and everything they spawn, run inside a root span named `agent`. `residuum logs --agent <name>` filters on it. The span buffer that feeds bug reports and trace exports gives every span inside an agent's root span the same `agent` field.
