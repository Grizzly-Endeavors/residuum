# Hub

One Residuum process, the **hub**, hosts every agent under `~/.residuum/`. The hub owns what exists once per process. Each agent owns everything that is its own.

| Hub (one per process) | Agent (one per agent) |
|---|---|
| HTTP server, relay tunnel, A2A listener, workbench server | Message bus, event loop, and the agent itself |
| Tracing, log level, updater, auto error reports | Memory, sessions, messenger, pulses, subconscious |
| Secrets and the agent-key and A2A-key stores | Discord, Telegram, Teams and webhook adapters |
| Team write coordinator, team wiki index, shared checkpoint repositories | MCP servers, skills, workspace watcher |
| Session budget (`[background] max_concurrent`) | Its own `config.toml` and `providers.toml` |

Each agent loads its own config against the hub config, and keeps its own last-known-good copies, so a bad agent config fails only that agent, and a bad hub config reload keeps the hub on its last-known-good config.

## Start-up and shutdown

`residuum serve` loads the hub config, starts the servers, builds the shared services, and scans `~/.residuum/` for agents (any directory holding `config/config.toml`). Every agent whose `autostart` is on (the default) then starts, concurrently. The readiness marker `hub/residuum.ready` is written once the servers are bound and every autostart agent has either started or been recorded as failed. With no agent on disk, `serve` runs onboarding first.

Stopping the hub (SIGTERM, `POST /api/shutdown`, or a restart for an update) stops every agent gracefully before the servers.

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

Two agents can't run with the same Teams adapter port: the second one to start is `failed` with a message naming the agent that holds the port.

## Shared services

- **Team write coordinator**: one per hub; each agent takes its own view with its name.
- **Team wiki index**: opened once, with the embedding model of the first agent (by name) that configures one, or text-only when none does. Every agent's memory search holds a clone of the same handle.
- **Session budget**: one semaphore sized by the hub's `[background] max_concurrent`, taken by every agent's session turns. Main turns don't take a permit. A session waiting for a permit shows as `queued` in its agent's session list. Changing `max_concurrent` takes effect on the next restart.
- **Checkpoints**: the team and hub-config repositories are shared; each agent has its own workspace and config repositories. See [Checkpoints](checkpoints.md).
- **Tunnel status, secrets, key stores, tracing**: one of each, passed to every agent.
- **Relay agent list**: the hub keeps the relay's copy of its agent list current from the hub bus (created, deleted, state and visibility changes), so each agent is reachable at `{origin}/a2a/{instance}/{agent}` while it runs. See [Cloud tunnel](cloud-tunnel.md#agents-on-the-relay).
- **Sibling discovery**: one per hub, fanned out to every running agent's A2A client. See [A2A](a2a.md#siblings).

## Hub config reloads

`hub/config.toml` is watched. A change is applied where the hub owns it: a new `[gateway]` address rebinds the HTTP server (a failed bind keeps the current server), `[cloud]` restarts the tunnel (a changed `[a2a] enabled` is passed to the relay's agent list too), `[tracing]` updates the tracing service and the log level, and `[a2a]` restarts the A2A listener. Every running agent then reloads against the new hub config for what it reads from it (the timezone, its A2A card, and the hop limits).

## Activity

The host tracks two things per agent for the switcher: `busy`, true while a main turn runs, and `unread`, the number of main-conversation replies published while no web client was connected to that agent's `/ws`. Connecting a client resets `unread` to zero. Changes are published on the hub bus as `agent_activity` events.

## Hub bus events

Every change to an agent's state, autostart, or visibility is published in order as an `agent_state` event carrying the agent's summary. `agent_created` and `agent_deleted` carry who did it (`user` or `agent:<name>`). `agent_activity` carries `busy` and `unread`. `notice` events carry hub-level messages: a fallback to the last-known-good hub config, hub config notices, removed environment overrides that are still set, and hub config reload outcomes.

## HTTP

Agent routes are served under `/api/agents/{name}/`, resolved per request. An unknown agent answers `404 { "error": "no agent named '<name>'" }`. An agent that isn't running answers `409 { "error": "<name> is <state>", "state": "<state>" }`, except for its config, providers, MCP, channels, workspace-file, and checkpoint routes, which answer on a stopped or failed agent so it can be repaired. Other paths are served by the first running agent, so the hub-level routes and the app shell answer at the root.

## Logging

There is one log level for the whole hub. Every log line and span an agent produces carries an `agent` field: the agent's tasks, and everything they spawn, run inside a root span named `agent`. `residuum logs --agent <name>` filters on it. The span buffer that feeds bug reports and trace exports gives every span inside an agent's root span the same `agent` field.
