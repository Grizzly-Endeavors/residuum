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

Stopping the hub (SIGTERM, `POST /api/hub/shutdown`, or a restart for an update) first stops accepting starts, then stops every agent gracefully, then stops the servers. From the moment shutdown begins, `start`, `restart`, and `create` are refused with the error `Residuum is shutting down` (HTTP `503`, since the refusal is expected and temporary rather than a real lifecycle failure), so a request arriving during the shutdown can't start an agent that nothing would stop. Stopping an agent still works.

## Agent states

| State | Meaning |
|---|---|
| `starting` | The runtime is being built. |
| `running` | Adapters, the event loop, and HTTP routes are live. |
| `stopped` | Not running, by choice: never started, or stopped. |
| `failed` | Start-up failed, or the event loop died. Carries the plain-language error, its kind and underlying reason, and when it happened. |

An agent's event loop runs as its own task. A panic or fatal error in it moves that agent, and only that agent, to `failed`. The hub and the other agents keep running. The failure is logged at `error` with the agent's name, auto-reported through `TracingService::on_error` (when auto error reporting is on; the report's text names the agent), announced on the hub bus as an `agent_state` event, which carries the failure's message and kind, recorded as an `agent_failed` [team event](#team-event-log), and sent as an `agent_failed` [push](notifications.md#web-push). The hub files nothing in any user inbox for it. The agent's live sessions are recorded as interrupted and its adapters and MCP servers are stopped. A `failed` agent starts again only when someone starts or restarts it.

A failure records four things: the `message` for the user (what happened and what to do next), the underlying `reason` text alone, the time, and a `kind` chosen where the failure was recorded:

| Kind | Recorded for |
|---|---|
| `config` | A start that failed because the agent's configuration was rejected. |
| `port_conflict` | A start refused because another agent already holds the agent's Teams port. |
| `crash` | A panic while starting, and a running agent whose task panicked or whose event loop ended on its own. |
| `other` | Any other start-up failure. |

## Operations

- **start**: does nothing for an agent that is already `running` or `starting`. A start-up failure leaves the agent `failed` and is returned to the caller.
- **stop**: ends the agent's adapters and event loop and cancels its live sessions, which are recorded as interrupted the way a process restart records them. A turn in progress is stopped the way a user stop stops it. If the agent hasn't wound down after 150 seconds its task is aborted. From the moment the stop begins until it has finished, the agent is in the hub's **stopping set**: its state still reads `running`, an `agent_stopping` event is published, and `GET /api/hub/agents` and the hub WebSocket's snapshot list its name under `stopping`. The team router refuses teammate messages for it and the relay's agent list stops advertising it from the same moment.
- **restart**: stop, then start.
- **patch**: writes `autostart` and/or `[a2a] visibility` to the agent's `config/config.toml` with the same in-place edit the Settings page uses, checkpointing the agent's config repository first. The patched file is validated before anything is written. A running agent is then reloaded, and the call returns once the reload has finished (or after 30 seconds).
- **create**, **delete** and **restore**: see [Agent creation, deletion and restore](agent-lifecycle.md). Creation starts the agent and, when a description was given, hands it to the new agent's main conversation as its first message (from the owner for the user; from the creating agent as a teammate message otherwise, carrying the creator's outgoing hop count so a chain of agents creating agents meets the hop limit). If the agent can't start, it is still created and returned `failed`. Deletion stops the agent first and keeps its checkpoint history. Restoring a deleted agent (from the web UI's Home, `residuum agent restore`, or `POST /api/hub/agents/restore`) writes it back from that history, adopts it, and starts it when its `autostart` is on. `GET /api/hub/agents/deleted` lists what can be restored. The hub event names the acting agent when an agent did it, and the user hears of it as a toast and a team event. The hub files nothing in any agent's user inbox for these. Agents create and delete teammates with the `agent_create` and `agent_delete` tools, described in [Agent creation, deletion and restore](agent-lifecycle.md#agent-tools); they have no restore tool.

Two agents can't run with the same Teams adapter port: the second one to start is `failed` with a message naming the agent that holds the port. An agent reserves its port when it begins to start, so agents starting at the same moment can't both take it; the reservation is released when the agent stops, fails, crashes, or is deleted, and moves to the new port when a reload changes `[teams] port`. If the Teams adapter can't bind its port (another program or agent holds it), the hub publishes a `notice` naming the agent and the port, both at start and after a reload.

Operations on one agent are serialized. Once an agent is deleted, any `start`, `stop`, `restart`, or `patch` that was already waiting on it answers `404`; a deleted agent's directory is never recreated by them. Only a restore or a create of that name writes it again, and those two run one at a time under one lock, so a restore and a create (or a delete) of the same name can't interleave: one wins and the other answers `409`.

The `autostart` and `a2a_visibility` an agent's summary reports come from the last `config/config.toml` that loaded. For a running agent that is the config it last loaded or reloaded; for a stopped one, the file as read on each request. A file that can't be read or parsed, or that was emptied after a config had loaded, leaves the last loaded values in place and logs a warning once. An agent whose config has never loaded is reported `private`, so the A2A listener never serves the card of an agent whose visibility can't be established without a key.

## Shared services

- **Team write coordinator**: one per hub; each agent takes its own view with its name.
- **Team wiki index**: one instance for the hub, opened with the embedding model of the first agent (by name) that configures one, or text-only when none does. Every agent's memory search holds a clone of the same handle. The embedder is re-evaluated when an agent starts, finishes a config reload, or is deleted: when the choice changes (the first agent with an embedding model appears, the providing agent's embedding config changes, or it stops providing one), the same instance swaps its embedder in place, clears its vector store if the model differs, and refills it from the wiki pages on the next search. Every agent's searcher sees the new embedder at once. If the new embedder can't be built or its vector store can't be opened, wiki search stays text-only and the hub publishes a `notice`.
- **Team router**: one per hub. Each running agent registers its messenger at start and unregisters when its stop begins. A `message_agent` call to `agent:<name>` (or `agent:<name>/<session>`) goes to the router, which checks the target's state against the agent host and hands the message to the target's messenger; nothing queues for an agent that isn't running. The router also supplies the teammate roster that `list_agents` and every agent's `TEAM` prompt block show. See [Teammates](background-tasks.md#teammates).
- **Session budget**: one semaphore sized by the hub's `[background] max_concurrent`, taken by every agent's session turns. Main turns don't take a permit. A session waiting for a permit shows as `queued` in its agent's session list. Changing `max_concurrent` takes effect on the next restart.
- **Checkpoints**: the team and hub-config repositories are shared; each agent has its own workspace and config repositories. See [Checkpoints](checkpoints.md).
- **Tunnel status, secrets, key stores, tracing**: one of each, passed to every agent.
- **Team change feed**: one watcher over the team directory, publishing `team/...` paths on its own bus. The hub WebSocket, every agent's `/ws` (for clients that watch `team/...` prefixes), and every agent's artifact reload watcher read it, so a change to a team file is watched once however many agents run. The hub also watches the team workbench from this feed itself, with no agent involved, and publishes an event for every artifact added, changed or removed, which the hub WebSocket sends as `artifact_updated` and `artifact_removed` (see [Live reload](workbench.md#live-reload)). Each agent also has a feed over its own directory.
- **Relay agent list**: the hub keeps the relay's copy of its agent list current from the hub bus (created, deleted, state and visibility changes), so each agent is reachable at `{origin}/a2a/{instance}/{agent}` while it runs. See [Cloud tunnel](cloud-tunnel.md#agents-on-the-relay).
- **Sibling discovery**: one per hub, fanned out to every running agent's A2A client. See [A2A](a2a.md#siblings).

## Hub config reloads

`hub/config.toml` is watched. A change is applied where the hub owns it: a new `[gateway]` address rebinds the HTTP server (a failed bind keeps the current server), `[cloud]` restarts the tunnel (a changed `[a2a] enabled` is passed to the relay's agent list too), `[tracing]` updates the tracing service and the log level, `[a2a]` restarts the A2A listener, and `[push]` changes the contact the next push is signed with. Every running agent then reloads against the new hub config for what it reads from it (the timezone, its A2A card, and the hop limits).

Every reload attempt ends with a `hub_config_reloaded` event, published beside the `notice` that tells the user how it went. `ok` is false when the file couldn't be loaded, in which case the hub keeps its previous config and the notice gives the reason. `changed` is true when the loaded config differs from the one the hub was running; a reload that found nothing to apply sends `ok: true, changed: false` and no notice. `message` is the text of the notice, or `null` when there is none.

The hub's reload queue carries two signals. `Hub` (from the config watcher, hub config writes, key stores, and the cloud settings) reloads the hub config as above. `Workspace`, sent when an edit through `/api/team/workspace/...` changes the team's `AGENTS.md` or `USER.md`, makes every running agent reload its workspace so it picks up the new identity files.

## Activity

The host tracks two things per agent for the switcher: `busy`, true while a main turn runs, with `busy_since`, when that turn began, and `unread`, the number of main-conversation replies published while no web client was connected to that agent's `/ws`. Connecting a client resets `unread` to zero. A socket opened through the artifacts origin by a workbench page is not a client, so it neither resets `unread` nor counts as connected (see [workbench.md](workbench.md#api-forwarding)). Changes are published on the hub bus as `agent_activity` events, which the hub WebSocket (`/api/hub/ws`) forwards. A change in activity is never an `agent_state` event. The snapshot a hub WebSocket connection starts with, and `GET /api/hub/agents`, carry every agent's current activity, so a client that connects mid-turn knows which agents are busy and since when.

## Watching running agents

The hub learns what happens inside a running agent as it happens. Each agent has a **watcher**: a task the host starts with the agent, before its event loop runs, and stops once the agent's run has ended, after the agent's own shutdown, so what the agent published while winding down still gets through. Nothing arrives from a watcher once it has stopped. If the watcher can't subscribe to the agent's bus, the agent still runs and the hub logs an error.

The watcher reads the agent's own bus and publishes **agent changes** on one hub-wide feed, in order per agent. It reads every subscription continuously, whether or not anything consumes the feed. A consumer gets its own unbounded queue, so none are lost, and one that lets 10,000 changes pile up is logged as stuck. A consumer hears changes from the moment it subscribes; nothing is replayed.

| Source | Change |
|---|---|
| Sessions | A session run started, moved to another state, or completed. |
| Outbound A2A tasks | A task the agent sent to a remote agent was recorded or changed. The task tracker publishes one when an unreachable streak starts, one when the streak passes its 10-minute notice threshold (the moment it sends its notice), and one when the streak ends. |
| `UserInbox` topic | The `user_inbox_add` tool saved an item; the change carries the item's id. Only this tool publishes it. Items that appear any other way, and every change to the inbox's files, arrive as file changes. |
| The agent's own change feed | A file the hub follows changed: a top-level `*.json` file in `inbox/user/`, `scheduled_actions.json`, `HEARTBEAT.yml`, `pulse_state.json`, or a file in `config/`. A batch of file changes is one change per kind of file it touched. |
| Resync | Everything about the agent may have changed. Sent when the agent finishes starting, when the agent's change feed says it may have missed changes, and every 60 seconds while that feed is down, until its next batch or resync. The hub logs one warning when the feed goes down. |

A consumer answers a resync by recomputing everything it shows for that agent, from disk and the agent's session registry.

Every session event, lifecycle or turn (tool calls and responses included), is also relayed on a broadcast of its own, tagged with the agent's name and the source label the session started with. A consumer that falls more than 1,024 events behind is told how many it missed. The hub WebSocket reads this broadcast to carry sessions to the clients that subscribed to them (see [Session relay](hub-http.md#session-relay)).

**Turn hook.** The agent runtime calls its activity tracker exactly once when a main turn ends, whatever the outcome, after the turn's replies are published and counted as unread. The hook puts the turn on the same feed: the user's message text, if a user started the turn; the last reply text, if there was one; the time; whether the turn had `user` or `background` visibility; and whether any client had the agent's WebSocket open. Empty text counts as no text. Unread counting is unchanged.

The changes themselves are not exposed over HTTP or the hub WebSocket. The [team event log](#team-event-log) and the [team overview](#team-overview) read them and are.

## Team event log

The hub keeps a log of what has happened across the team since the process started. It lives in memory, holds up to 500 entries, and starts empty on every boot, so its first entry, `hub_started`, marks where this process began. An entry is `{ id, at, agent, kind, level, summary, target }`. `id` increases by one for every entry within a boot. `at` is an RFC 3339 time. `agent` is the agent the entry is about, or `null`. `summary` is one plain sentence that names the agent, such as `atlas finished a session` or `brittle couldn't start: <reason>`. `target` says where a client takes the user to see it, and is `null` when there is no such place: `{ kind: "agent_place", agent, place }` for an agent's chat, `{ kind: "session", agent, run_id }` for a session run, or `{ kind: "inbox_item", agent, item_id }` for an item in the user inbox.

| Kind | Recorded when | Level | Target |
|---|---|---|---|
| `hub_started` | The hub process starts, before any agent does. Its summary is `Residuum started`. | `info` | none |
| `agent_started` | An agent's state becomes `running`: `atlas started`. | `info` | the agent's chat |
| `agent_stopped` | A `running` or `starting` agent's state becomes `stopped`, on request or because the hub is shutting down: `atlas stopped`. | `info` | the agent's chat |
| `agent_failed` | An agent's state becomes `failed`. A start that failed reads `atlas couldn't start: <reason>`, with the failure's underlying reason on one line. A running agent that crashed reads `atlas stopped unexpectedly`, and a crash while starting reads `atlas couldn't start because of an internal error`. | `error` | the agent's chat |
| `agent_created`, `agent_restored` | The matching hub event: `nova was created`, or `nova was created by atlas` when an agent did it. The hub announces an agent once it has started, so the agent's `agent_started` entry comes first. | `info` | the agent's chat |
| `agent_deleted` | The matching hub event: `nova was deleted`, or `nova was deleted by atlas`. | `info` | none |
| `agent_replied` | The turn hook reports a main turn that ended with a reply and `user` visibility: `atlas replied in your conversation`. Once per turn, and never for a turn with no reply or a `background` turn. | `info` | the agent's chat |
| `session_started` | A session run registered, unless it is scheduled (a pulse or a scheduled action): `atlas started a session: <purpose>`. | `info` | the run |
| `session_finished` | A session run that was not scheduled ended. A completed run reads `atlas finished a session: <purpose>`, a stopped one `atlas's session was stopped: <purpose>`, and a failed one `atlas's session failed: <reason>`. | `info` when completed, `warn` when stopped, `error` when failed | the run |
| `scheduled_run_finished` | A pulse or scheduled action's run ended. It is the only entry for such a run. `atlas finished the pulse "email_check"`, `atlas's scheduled action "nightly digest" was stopped`, or `atlas's pulse "email_check" failed: <reason>`. | `info`, or `error` when it failed | the run |
| `inbox_item_added` | The `user_inbox_add` tool saved an item: `atlas added an item to your inbox`. Items that appear any other way, and changes to the inbox's files, add nothing. | `info` | the item |
| `hub_notice` | The hub published a `notice` to every client. The summary is the notice's text on one line, and the entry has the notice's level and agent. A warning the hub sends to one WebSocket connection alone, about a message that connection sent, is not one. | the notice's | none |

When the log is full, recording a new entry evicts the oldest entry that is not protected, whatever its level. The newest 100 `warn` and `error` entries are protected, so a run of routine entries can't push a failure out. Ids are not reused, so evicted entries leave gaps in a page.

A recorder reads the hub bus and the feed of agent changes (see [Watching running agents](#watching-running-agents)) and writes the entries. The hub starts it before the startup notices and before any agent starts, because neither source replays. The two sources are separate streams, so entries from one can interleave with entries from the other in an order that differs from the order things happened. A slow reader of the hub bus can lose events once it falls 256 behind, and the recorder logs a warning when it does. A session's end is worded from what the recorder saw when that run started, so a run it never saw start records nothing and is logged at `debug`.

The log is read with `GET /api/hub/events`, and each new entry is sent on the hub WebSocket as `team_event`, both described in [Hub HTTP Surface](hub-http.md#team-events).

## Team overview

The hub keeps what Home shows about each agent beyond its state, activity and summary, and tells clients as it changes. An agent's **overview** is `{ name, last_message, live_sessions, upcoming, inbox_unread, outbound_problems }`. Nothing about it is stored: each part is read from where it is kept, and the hub holds only what it last told clients.

**`last_message`** is the newest message of the agent's main conversation that has `user` visibility and text, from the user or the agent. An assistant message that only calls tools has no text and is passed over, and a background turn is not part of it. It is `{ role, preview, at, at_precision }`, or `null` when there is none.
- `preview` is the text as plain text on one line: Markdown syntax removed (links and images keep their text, code keeps its characters, raw HTML goes), every run of whitespace one space, and cut to at most 200 characters, the `…` that marks a cut included. A message with nothing to show once stripped is passed over too.
- `at` is RFC 3339 with the offset of the hub's timezone. `at_precision` is `minute`, or `day` for a message read from an episode, which keeps the date and not the time; `at` is then the start of that day.
- A running agent's comes from the turn hook, for each main turn with `user` visibility: its reply when that has text, otherwise what the user said. Until a turn ends it is read from disk as a stopped agent's is, so it starts from what the agent's history held when it started.
- A stopped, starting or failed agent's is read from its recent history on disk. When that holds no such message, it is the newest message of the newest main-conversation episode that has one. A session's episode is not the main conversation, and an episode keeps neither who saw each message nor its time, so every user or assistant message in it counts. A history or episode file that can't be read is logged at `warn` and read past.

**`live_sessions`** are the session runs in the agent's session registry, oldest first, each `{ address, run_id, category, source_label, purpose, state, started_at }`. It is empty for an agent that isn't running, whatever its registry still holds.

**`inbox_unread`** is how many items in the agent's active user inbox the user hasn't opened, counted from its files whatever state the agent is in. An inbox that can't be read counts as none and is logged at `warn`.

**`upcoming`** are the next runs of the agent's pulses and one-off scheduled actions, soonest first and at most three, each `{ kind, name, at }` with `kind` `pulse` or `action` and `at` an RFC 3339 time with the offset of the hub's timezone. They are read from the agent's files whatever state it is in, so Home can say that a run won't happen while the agent is stopped.
- A pulse's time is the first moment at or after the later of now and the end of its `schedule` since its last run (from `pulse_state.json`) that falls inside its `active_hours`. A pulse that has never run counts from now. A pulse that is due already is told at the start of the current minute, because the scheduler decides once a minute. A pulse that is disabled, whose schedule or active hours can't be read, or whose active hours never open is left out, and so is every pulse when the agent's `[pulse] enabled` is `false`. The Scheduled view's `next_fire_at` for a pulse is the same calculation (see [heartbeats.md](heartbeats.md#scheduled-view)).
- An action's time is the time it was set for. An action that is overdue (its agent was stopped when it came due) keeps that time. Runs at the same moment list pulses before actions.
- An agent whose `config.toml` can't be read lists no runs, because the file is what says whether its pulses run. A `scheduled_actions.json` that can't be read costs only the actions. Each is logged at `warn` once for as long as it stays unreadable, and noted when it reads again. A `HEARTBEAT.yml` with a problem lists the pulses that loaded; the agent's own scheduler reports the problem.

**`outbound_problems`** are the open tasks the agent sent to remote agents (see [a2a.md](a2a.md)) whose current unreachable streak has passed the tracker's notice threshold of 10 minutes, the longest unreachable first, each `{ task_id, remote_agent, status_text, unreachable_since }`. `status_text` is the last status the remote agent reported, or `null`. A task that answers again, finishes, or is stopped is no longer one. They are read from the agent's `a2a/outbound.json` and are empty for an agent that isn't running, because nothing watches the tasks of one that isn't. A file that can't be read is logged at `warn` once and counts as no problems.

A **tracker** reads the hub bus and the feed of agent changes (see [Watching running agents](#watching-running-agents)) and marks the part of an agent's overview that a change may have affected: session changes mark `live_sessions`; `user_inbox_added` or a change to the user inbox's files marks `inbox_unread`; a main turn with `user` visibility sets `last_message`; a change to `scheduled_actions.json`, `HEARTBEAT.yml`, `pulse_state.json` or a file in the agent's `config/` directory marks `upcoming`; an outbound task change marks `outbound_problems`; and a resync, or any change to the agent's state, marks every part. The hub's own inbox actions (`PUT`, `POST .../archive` and `POST .../restore` under `/api/hub/inbox/`) mark that agent's `inbox_unread`, whatever its state, since nothing watches a stopped agent's files. Answering a request for the overview counts every agent's inbox again and reads a stopped agent's parts from its files, so a file placed by hand in a stopped agent's inbox, or an edit to its schedule, shows up then. The hub starts the tracker before the startup notices and before any agent starts, because neither source replays.

An agent's overview is sent to clients as an `agent_overview` frame carrying the whole overview, whenever any of it differs from what clients were last told. Changes are gathered: the first change after a frame starts a one-second wait, and the frame that ends it shows the agent as it is then. An agent therefore gets at most one frame per second, and its last state is always sent. A request that finds an agent differing from what clients were told starts the same wait. A created or restored agent is sent at once, and a deleted agent gets no frame after its deletion. A tracker that falls 256 events behind the hub bus logs a warning and reads every agent again.

A task's streak passing the notice threshold is announced by the outbound task tracker on its next failed poll, which can be a minute after the threshold. The overview does not wait for that: when it reads an agent's outbound tasks it also notes when the next streak will pass the threshold, and reads them again then, so the problem appears at the threshold and goes out in the next frame.

The overview is read with `GET /api/hub/overview` and sent on the hub WebSocket as `agent_overview`, both described in [Hub HTTP Surface](hub-http.md#team-overview).

## Hub bus events

Every change to an agent's state, autostart, or visibility is published in order as an `agent_state` event carrying the agent's summary. `agent_stopping` carries the name of an agent whose stop has begun. `agent_created`, `agent_restored` and `agent_deleted` carry who did it (`user` or `agent:<name>`); the first two carry the agent's summary. `agent_activity` carries `busy`, `busy_since` and `unread`. `hub_config_reloaded` reports each hub config reload attempt (see [Hub config reloads](#hub-config-reloads)). `notice` events carry hub-level messages: a fallback to the last-known-good hub config, hub config notices, removed environment overrides that are still set, and hub config reload outcomes.

The hub generates a random **boot id** when it starts. Each hub WebSocket connection is sent it first, as `hub_boot`, before the agent snapshot, so a client can tell a restarted hub from a dropped connection to the same one. The team event log carries the same id, in every page it serves and every `team_event` frame, so a client that sees it change discards the events it holds.

## HTTP

One router serves everything the hub offers: the hub, team and per-agent routes, the hub WebSocket, and the web app. Agent routes are resolved per request under `/api/agents/{name}/`; an unknown agent answers `404` and an agent that isn't running answers `409`, except on its file routes (config, workspace files, chat history, inbox, and the other routes that only read or write its files). No agent's router serves a root path. The routes, their layout and the request guards are in [Hub HTTP Surface](hub-http.md).

## Logging

There is one log level for the whole hub. Every log line and span an agent produces carries an `agent` field: the agent's tasks, and everything they spawn, run inside a root span named `agent`. `residuum logs --agent <name>` filters on it. The span buffer that feeds bug reports and trace exports gives every span inside an agent's root span the same `agent` field.

## Running in Docker

The images (`docker/Dockerfile`, `docker/Dockerfile.release`) run `residuum serve --foreground` as the non-root `residuum` user with `RESIDUUM_GATEWAY_BIND=0.0.0.0`, and expose the gateway port `7700`. The volume `/home/residuum/.residuum` holds the whole layout: `hub/`, `team/`, and one directory per agent. On an empty volume the hub starts without an agent and the web app runs onboarding, so the first agent is created from the browser. The A2A listener (`7702`) is not published by `docker/docker-compose.yml`; publish it only for callers that should reach it directly.

Only hub-level environment overrides apply in a container: `RESIDUUM_GATEWAY_BIND`, `RESIDUUM_GATEWAY_PORT`, `RESIDUUM_CLOUD_TOKEN` and `RESIDUUM_TIMEZONE`. Agent-scoped variables have no effect (see [Config](config.md#environment-overrides)); provider key variables such as `OPENAI_API_KEY` and `${ENV_VAR}` references inside an agent's config files still read the container's environment.
