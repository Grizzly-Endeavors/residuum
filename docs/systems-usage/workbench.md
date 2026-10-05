# Workbench

The workbench is where the agent builds interactive artifacts for the user: charts, dashboards, calculators, explorers, and pages for choosing between options. The workbench belongs to the team, so every agent sees the same artifacts, and an artifact belongs to no agent. Each artifact is an HTML page (`team/workbench/<name>.html`) or a folder (`team/workbench/<name>/index.html` plus the files it loads) in the team layer. Every artifact is a page of its own on the artifacts origin (see [The artifacts listener](#the-artifacts-listener)), and talks to Residuum directly through the `residuum` object injected into it. The web UI lists artifacts at `/team/workbench` and opens each in a tab of its own; it never shows one inside itself.

## Intended use

**Agent:** activates the bundled `workbench` skill, writes a page or a folder with `write_file`, and tells the user the artifact's title and where to open it. Editing an existing artifact is `read_file` then `edit_file`; an open artifact reloads by itself when any of its files change. Every call about one agent names that agent: `residuum.ask(prompt, { agent })`, `residuum.sessions.start({ agent, prompt })`, `residuum.agent(name)` for its live frames and files, and `/api/agents/<name>/...` for its routes.

**User:** opens **Workbench** from the rail, under Team. The list shows every artifact by its `<title>`, with its path (`/team/workbench/<name>`) and when it was last edited, newest edit first, and how many sessions it has running on any agent. For a moment after an agent writes to an artifact, its row's seam lights and it reads "updating now". The list follows the hub socket's artifact events, so it stays current with no agent running, and reads everything again when that socket reconnects. **Open** opens the artifact in a new tab at its address on the artifacts origin (`http://<host>:<port>/<name>/` locally, `https://<user>.workbench.agent-residuum.com/<name>/` through Residuum Cloud); in the installed app, the platform decides whether that is the browser or an in-app browser. **Copy link**, in the row's menu, copies that address. Selecting a row (`/team/workbench/<name>`) opens it in place to its address and its running sessions, each with its purpose, agent, state and Stop, and each opening in the context panel. Deleting an artifact from the row's menu removes its page or folder and its data files immediately, after checkpointing the team directory (see [checkpoints.md](checkpoints.md)); the toast's Undo restores them all. When artifacts can't open, a banner above the list says why (see [API forwarding](#api-forwarding)).

## Files

| Path | Holds |
|------|-------|
| `team/workbench/<name>.html` | A single-page artifact. |
| `team/workbench/<name>/` | A folder artifact: `index.html` is its page, and any other files are loaded by relative URL. When both a page and a folder share a name, the folder wins. |
| `team/workbench/<name>.<anything>` | That artifact's saved data (for example `<name>.state.json`), written through the workspace file API. Not part of the artifact, not watched for reloads, deleted with the artifact. |

`<name>` is lowercase letters, digits, and single hyphens, at most 64 characters, and not `api`, which is the path the artifacts listener serves the API on; anything else is ignored. The listener serves files of any size; through Residuum Cloud a response over 10 MB fails (see [Through the relay](#through-the-relay)). Symlinks are ignored, and a file must resolve inside its artifact's folder. A path segment of `.` or `..` is refused, while a dot-prefixed name such as `.well-known/` or `.env.example` is an ordinary file and is served.

## The artifacts listener

Artifacts are served by their own listener, never by the gateway's main listener: at startup Residuum binds the first free port after the gateway's (`7702` by default), skipping Teams' configured port and its default `7701` so enabling Teams later can't collide. The port is stable across restarts while the machine's ports don't change, which keeps each artifact's origin, and so its browser storage, stable. Artifact files are read-only: `/<name>/` is the artifact's page, `/<name>/<path>` a file in a folder artifact, and `/<name>` redirects to `/<name>/`. `/api` and everything under it is the API (see [API forwarding](#api-forwarding)). If no port can be bound, Residuum runs without it, logs the reason at `error`, and the Workbench page shows it.

### API forwarding

The listener answers `/api` and everything below it, HTTP requests of every method and WebSocket upgrades, with the same hub router the gateway serves (see [hub-http.md](hub-http.md)). A page opened from the artifacts origin therefore calls Residuum on its own origin, with relative URLs like `/api/agents/scout/status`, and opens the hub socket (`/api/hub/ws`) and an agent's socket (`/api/agents/{name}/ws`) the same way. The listener hands each request to the router in-process, before any routing of its own, so the router sees it exactly as it arrived: its routes' path parameters are the only ones on the request, and a WebSocket upgrade passes through whole. Nothing outside `/api` is forwarded: the router's embedded web app, webhooks and relay callback are not reachable through this port, and an unknown `/api` path answers `404` rather than a page. Because `/api` is the API, no artifact is named `api`, and an `api` page or folder in `team/workbench/` is neither listed nor served.

Every forwarded request carries an internal marker saying it arrived through the artifacts origin. The marker is a request extension, not a header, so nothing a client sends can set or forge it. It has two effects:

- **Block list.** A marked request is refused with `403` and `{ "error": "..." }`, whatever its method, on `/api/hub/shutdown`, `/api/hub/stop-all`, `/api/hub/update/check`, `/api/hub/update/apply`, `/api/hub/update/restart` and `/api/hub/config/complete-setup`: shutting Residuum down, stopping every agent, updating it and finishing setup stay with the Residuum app. The refusal is logged at `warn`. Everything else the web UI can call, an artifact can call, including starting and stopping a single agent and writing config.
- **No client count.** An agent socket opened through the artifacts origin is not a client: it does not reset the agent's unread chat count and does not count as someone connected to the chat. The user's own socket still does.

The `X-Residuum-Artifact` header the SDK sends with every request names the calling artifact for session starts and inbox attribution. A page can send any valid name, so it identifies, it doesn't authenticate.

`GET /api/team/workbench/info` tells the web UI where artifacts are: the local port and, when connected to the cloud relay, the UI and artifacts origins the relay announced. The UI opens artifacts on the relay's artifacts origin when it is itself being viewed through the relay, and on its own host at the artifacts port otherwise — but only when the UI page itself is plain HTTP, since the artifacts listener has no TLS of its own and an `https://host:port` origin never loads. Otherwise artifacts can't open, and the Workbench's banner says why instead of guessing at an origin: the listener isn't running (with the reason Residuum gave), or the page is served over HTTPS and Residuum Cloud hasn't reported a workbench address yet. A user's own HTTPS reverse proxy is in the second case for good, and the banner says so. While the banner shows, the Workbench reads where artifacts are again every 10 seconds, so it clears once Residuum Cloud finishes connecting.

## The `residuum` object

The listener injects the SDK (`assets/workbench/sdk.js`) into every HTML page it serves, ahead of the page's own scripts. The SDK talks to Residuum on the page's own origin, so it needs nothing around the page.

| Member | Does |
|--------|------|
| `residuum.fetch(path, init)` | Calls Residuum's API and returns a `Response` (see [Requests](#requests)). Takes `fetch`'s options; a plain object or array `body` is sent as JSON, a string, `ArrayBuffer`, typed array, or `Blob` unchanged. |
| `residuum.ask(promptOrRequest, { agent }?)` | One-shot call to the background small model of one agent: `POST /api/agents/{name}/model/complete`. A string is shorthand for `{ prompt }`. The agent is named in the request (`{ agent, prompt }`) or in the second argument, and a call that names none rejects with a `TypeError`. Resolves to the response body; rejects with an `Error` on any non-2xx. |
| `residuum.on(type, handler)` | Residuum's own events, which need no agent: `artifact_updated`, `artifact_removed`, `connection`, or `"*"` for all three (see [The hub socket](#the-hub-socket)). Any other type throws a `TypeError` that says to use `residuum.agent(name).on`. Returns a function that removes the handler. |
| `residuum.watch(prefix, handler)` | Follows team files: `team` or a path under it, like `team/wiki` (see [Change feed](#change-feed)). Any other prefix, `""` included, throws a `TypeError` that says to use `residuum.agent(name).watch`; so do an absolute path and one with `..`. Returns a function that stops watching. |
| `residuum.agent(name)` | The handle for one agent, `{ name, on(type, handler), watch(prefix, handler) }` (see [Agent handles](#agent-handles)). The same name gives the same handle; a missing or empty name throws a `TypeError`. |
| `residuum.sessions.start({ agent, prompt, context?, skill?, model? })` | Starts a session for this artifact on the named agent and resolves to a handle `{ agent, address, on(type, handler), send(text), stop() }` (see [Agent sessions](#agent-sessions)). A call that names no agent rejects with a `TypeError`. |
| `residuum.sessions.follow(agent, address)` | Follows a session already running, or finished, on the named agent, such as one from before a reload: the same handle shape as `start`, without starting anything (see [Agent sessions](#agent-sessions)). A call missing either argument throws a `TypeError`. |
| `residuum.state.get()` / `residuum.state.set(value)` | Sugar over the team file API (`/api/team/workspace/file`) for the artifact's own `workbench/<name>.state.json`, which is `team/workbench/<name>.state.json` in the team folder: `get()` resolves to the parsed value or `null` before the first `set()` and rejects on invalid JSON; `set(value)` writes `JSON.stringify(value)` unconditionally. |
| `residuum.artifact` | This artifact's own name, embedded when the artifacts listener serves the page. |
| `residuum.version` | Residuum's version, embedded the same way. Matches `GET /api/agents/{name}/status`'s `version`. |
| `residuum.features` | A frozen array of feature ids this build supports, embedded the same way. Matches `GET /api/agents/{name}/status`'s `features`. |

The endpoint and event catalogue the agent works from is the skill's `references/api.md`.

### The hub socket

As the page loads, the SDK opens the hub WebSocket (`/api/hub/ws`) on the page's origin. It carries live reload, artifact events, team watches and the page's sessions, so none of them needs an agent running. It reconnects with backoff, from 1 second up to 15. `residuum.on("connection")` handlers hear `{ "type": "connection", "state": "connected" | "disconnected" }` when it opens or drops, and a handler registered later hears the current state first. A drop is logged to the browser console. After a reconnect the SDK sends its team watch and session subscription again, and gives every `residuum.watch` handler `workspace_resync` with `reason: "reconnected"`, since changes during the gap are lost.

**Live reload.** When `artifact_updated` names the page's own artifact, the SDK reloads the page. A page that registers its own `artifact_updated` handler takes that over: its handler is called and the SDK doesn't reload. A `"*"` handler doesn't count. `artifact_removed` for the page's own artifact reaches handlers only.

### Agent handles

`residuum.agent(name)` returns the handle for one agent. Its socket (`/api/agents/<name>/ws`) opens the first time the handle's `on` or `watch` is used and stays open while the page does, reconnecting with backoff like the hub socket. It turns verbose mode on as it connects, so tool calls and results arrive.

- `on(type, handler)` receives that agent's frames of `type`, `"*"` for all of them: `turn_started`, `tool_call`, `response`, the `session_*` frames of its sessions, and the rest. `connection` follows this socket, and a handler registered later hears the current state first. Keepalive answers and the change feed's frames are not passed on.
- `watch(prefix, handler)` follows that agent's workspace in the file API's namespace: unprefixed paths are its own files, `team/...` the team's, and `""` all of it. Otherwise it behaves like `residuum.watch`, and after a reconnect its handlers get `workspace_resync` with `reason: "reconnected"`.

An unknown agent, or one that isn't running, refuses the socket. The handle reports `disconnected`, logs it to the console, and keeps trying, so it connects once the agent runs.

### Requests

`residuum.fetch` sends every request to the page's own origin with `X-Residuum-Artifact: <name>`, which the SDK sets over any value the page gave.

- **Paths.** A path is `/api` or below on the page's origin. One that names its scope (`/api/hub/...`, `/api/team/...`, `/api/agents/<name>/...`) is sent as it is. The unscoped spellings of hub routes map onto `/api/hub/...`: `/api/secrets`, `/api/agent-keys`, `/api/a2a/keys`, `/api/cloud/...`, `/api/update/...`, `/api/tracing/...`, `/api/shutdown`, `/api/system/timezone`, `/api/mcp-catalog`, and `/api/checkpoints` for the `hub` and `team` repositories. `/api/workbench/...` maps onto `/api/team/workbench/...`. Every other unscoped path, such as `/api/status`, belongs to one agent, and nothing tells the SDK which, so it isn't sent: the call resolves to a `400` response whose `error` says to use `/api/agents/<name>/...`, or, for `POST /api/sessions` and `POST /api/model/complete`, `residuum.sessions.start` or `residuum.ask`. A path outside `/api` resolves to a `400` the same way. Each of these is also logged to the console.
- **Lanes.** At most 8 ordinary requests and 4 model calls (`/api/agents/<name>/model/complete`) run at once per page; the rest wait in the order they were made. Model calls have a lane of their own, so slow ones never hold up the page's other requests, and one page stays well under the relay's limit of 50 requests in flight per instance.
- **Overload.** A `503` whose body is exactly the relay's `agent overloaded` is retried up to 3 times, with backoff starting near 500 ms and doubling, with jitter. The relay refuses these before forwarding them, so a retry never repeats a write. A request still refused after that returns the `503`, and the console says so. Any other `503` is returned as it is.
- **Failures.** Every response, a block-list `403` included, is returned as Residuum sent it. A request that never reaches Residuum rejects with an `Error` saying so, and one the page aborted with its own `signal` rejects with the `AbortError`.

## Model calls

`POST /api/agents/{name}/model/complete` is a one-shot request/response call to the background `small` model of the named agent (that agent's configured fallback chain applies when `small` is unset), so the agent must be running: an unknown agent answers `404` and one that isn't running answers `409` with its state. There is no agent loop, no tools, no memory, no identity files: the model sees only what the artifact sends. The request is `{ "prompt": "..." }` as shorthand for one user message, or `{ "system", "messages", "schema", "max_tokens", "temperature" }` for a full conversation; `schema` asks for structured output, returned as parsed `json` alongside the model's raw `content`. The artifact's own `max_tokens`/`temperature` win over the small tier's defaults when given. A malformed request answers `400`; a provider failure answers `502` with a plain-language error; a timeout answers `504`. Every call logs at `info` with the calling artifact's identity, the model, and token usage. `residuum.ask` is the SDK's wrapper around this endpoint.

## Agent sessions

An artifact that needs an agent to do work (write files, research, use tools) starts a session with `residuum.sessions.start({ agent, prompt })`, which calls `POST /api/agents/{name}/sessions`. A session runs on one agent, so the start names it: a call with no `agent` rejects in the SDK before any request, `POST /api/sessions` answers `400` with that explanation (and the SDK doesn't send it), an unknown agent answers `404`, and an agent that isn't running answers `409` with its state, which the rejected `Error` carries as `state`. The `X-Residuum-Artifact` header is what makes it an artifact session: the endpoint refuses a start without it. The session is a full fork of that agent in the `artifact` category, labelled `artifact:<name>`, and it runs, idles, and completes like any other session (see [background-tasks.md](background-tasks.md#artifact-sessions)). Its output stays with the artifact: it never reaches the main chat, the inbox, or notification channels on its own, and it cannot message the main agent (it can still file a user-inbox item when its task calls for one). It appears in that agent's Activity as From a workbench page, with `artifact:<name>` as its source.

Before the first start, the SDK subscribes to the artifact's sessions on the hub socket (`subscribe_artifact_sessions`, see [Session relay](hub-http.md#session-relay)) and waits for the `subscribed` answer; the subscription covers every later start, and is sent again after every reconnect. With no answer within 10 seconds, the start rejects with an `Error` whose `code` is `no_live_connection`, saying Residuum's live connection isn't available, and no session is started. Once subscribed, the page hears every event of its sessions as `session_frame`s, from the first, whichever agent runs them and whether or not the page has a socket to that agent.

A page that reloads, or opens while one of its artifact's sessions is already running, has no handle for it: `residuum.sessions.follow(agent, address)` gets one back. It subscribes to that one session directly (`subscribe_session`, not the artifact-wide `subscribe_artifact_sessions` a start registers) and is sent again after every reconnect, the same as a start's subscription. It returns the handle at once, without waiting for the subscription to be acknowledged, since nothing raced it the way a start's `POST` can.

The handle:

- A session is identified by its agent and its address together, because two agents can hold sessions at the same address (`artifact-chart-0001`). The SDK routes each relayed frame by (`agent`, `address`), so another agent's session never reaches this handle.
- `on(type, handler)` gets only this session's `session_*` frames (`session_started`, `session_state_changed`, `session_turn_started`, `session_tool_call`, `session_tool_result`, `session_broadcast_response`, `session_response`, `session_error`, `session_completed`, and the rest; `"*"` for all of them). Tool frames always arrive. Frames that arrived before the start request answered, such as `session_started` with the run id, are handed to each handler registered for their type when it is registered. It returns a function that removes the handler.
- `resync` reaches `on` too, when frames were lost: the hub socket fell behind the relay (`session_relay_lagged`) or reconnected, or a `follow` just created the handle, which has no frames of its own to go on. The SDK reads `GET /api/agents/{name}/sessions?artifact=<name>` for each agent the page has sessions on, and each handle gets `{ "type": "resync", "session": ... }` with its session as listed now (live, else its latest finished run, `null` when it isn't listed), or `{ "type": "resync", "session": null, "error": "..." }` when the read failed.
- `send(text)` messages the session through `POST /api/agents/{name}/sessions/{address}/messages`; the session sees it as a message from this artifact, and its reply arrives as a `session_response`. It resolves to the delivery outcome (`live`, `queued`, or `resumed`: a message to a finished session starts a new run at the same address).
- `stop()` stops the session through `POST /api/agents/{name}/sessions/{address}/stop`.

Failures reject with an `Error` carrying the gateway's plain-language message, plus `code` and `status` where the gateway gave them. `GET /api/agents/{name}/sessions?artifact=<name>` lists the sessions an artifact started, live and finished. Closing or reloading the page does not stop its sessions; a reloaded page has no handles for them, and finds them with that route, then gets a handle back for one still running with `residuum.sessions.follow(agent, address)`.

## Visibility and stop controls

Nothing limits how many model calls or sessions an artifact runs; instead the user can see what it's doing and stop it. Stopping is granular: nothing stops every session at once, and stopping the page is separate from stopping its sessions.

Every session an artifact starts is listed in its agent's Activity, where every run in a stoppable state has its own Stop, and its details in the session panel link to the artifact that started it. The Workbench shows the same runs on the artifact's row: how many it has running, on any agent, and, with the row selected, each with its purpose, agent and state, a Stop for a run in a stoppable state (forking, queued, running, or idle), and a way to open it in the context panel. They come from the team overview's live sessions whose source label is `artifact:<name>`, so they never drift from Activity. Finished runs are in each agent's Activity.

The page runs in its own tab, and closing that tab unloads it, ending any loop running in the page. That does not touch the artifact's sessions, which keep running and stay listed for individual stopping.

## Security model

The web UI has no login of its own (the relay authenticates remote access), so anything running on the UI's origin can call every endpoint. Artifacts are agent-written pages that load third-party scripts, so they never get that origin:

- **Separate origin.** Artifacts run on the artifacts listener's origin (`localhost:7702` locally, `<user>.workbench.agent-residuum.com` remotely), each in a tab of its own; the web UI never embeds one. An artifact gets that origin's browser storage and relative files, never the UI's. The browser won't let a page read the gateway's responses, since the gateway sends no CORS headers.
- **Cross-site guard.** The gateway and the artifacts listener both reject state-changing requests and WebSocket upgrades whose `Sec-Fetch-Site` is anything but `same-origin` or `none` (or, without fetch metadata, whose `Origin` is `null`). An artifact's origin is the *same site* as the UI's (same host, or a sibling subdomain), so a request from an artifact page to the gateway arrives as `same-site` and is refused, as are any other website's, on either listener. A page calling its own origin's `/api` (see [API forwarding](#api-forwarding)) is `same-origin` and passes.
- **Artifacts reach the API through their own origin.** The artifacts listener forwards `/api` to the same hub router, so a page can call everything except the block list's routes. The block list is the only limit on what a page calls. The SDK's path mapping and lanes shape its own requests; they are not a boundary.
- **Artifacts share one origin.** Every artifact runs on the same artifacts origin, so artifacts can read each other's browser storage. Anything that must stay private to an artifact belongs in the workspace, not the browser.

## Change feed

One watcher covers the whole workspace recursively, using the operating system's file notifications (inotify, FSEvents, ReadDirectoryChangesW through the `notify` crate). If native notifications can't start (the Linux inotify watch limit, an unusual filesystem), Residuum logs a `warn` naming the cause and polls the workspace every 2 seconds instead; hitting the watch limit later, as new folders appear, switches to polling the same way. If neither can start, Residuum logs an `error` and any view that starts watching is told live updates are off, which the web UI shows as an error notice. Symlinks are not followed.

A second watcher, built the same way, covers the team directory and publishes its changes with `team/`-prefixed paths (`team/workbench/tool.html`, `team/wiki/a.md`); see [Team files](team-files.md). Everything below applies to both.

Notifications are debounced into batches: a batch closes once 300 ms pass with no new notification, or 2 s after its first one while writes continue. Each changed path appears once per batch as `created`, `modified`, or `removed`, decided from what the batch saw and what is at the path when the batch closes:

- A rename is `removed` for the old path and `created` for the new one. Residuum's own atomic writes (a temporary file renamed over the target) show up as `created` or `modified` for the target, never as the temporary file.
- A path missing when the batch closes is `removed`, even one created and deleted inside the batch.
- A folder's `created` or `removed` stands for everything inside it; a rename or removal of a whole folder is reported for the folder alone. A folder whose only change is its own timestamp is left out, since its files report the real changes.
- Paths the workspace access policy hides (any `.index` segment, database files and their sidecars, atomic-write temporaries) never appear.
- Reads never produce changes, so an artifact that rereads a file on each change can't feed its own loop.

When the OS reports lost notifications (queue overflow, rescan), notifications back up past Residuum's buffer, or one batch touches more than 10,000 paths, the batch becomes a resync instead of a change list. Restarting the watcher (after the watch limit, or when the workspace folder itself disappears) sends a resync too.

Each WebSocket connection has its own watch set of prefixes, empty by default, so the main chat never receives change frames:

- The client frame `{ "type": "watch_workspace", "prefixes": ["team/wiki", "inbox/user", "team/workbench"] }` replaces the set. Prefixes are in the file API's namespace, so team files are watched, and reported, with a `team/` prefix. `[]` stops watching; `""` watches the whole workspace. A prefix that is absolute or contains `..` is refused with an `error` frame and the set is left unchanged. There is no limit on how many prefixes a connection watches or how long one is.
- A `team/...` prefix (`team/workbench`, `team/wiki`) receives changes to the team directory; a plain prefix receives changes to the agent's own directory; `""` receives both.
- Prefixes match by path segment: `team/wiki` matches `team/wiki` and anything under `team/wiki/`, never `team/wikipedia/`. A prefix naming a file matches only that file, and a prefix that doesn't exist yet matches once it appears. A change to a folder that contains a prefix (for example `projects` for the prefix `projects/alpha`) matches too, since renaming or removing that folder carries the prefix with it.
- `{ "type": "workspace_changed", "changes": [{ "path": "team/wiki/a.md", "kind": "created" }] }` carries a batch's changes under the connection's prefixes, sorted by path. A connection with no matching changes gets nothing.
- `{ "type": "workspace_resync", "reason": "overflow" | "watcher_restarted" }` means the connection's view may be stale. It replaces `workspace_changed` when more than 500 of a batch's changes match the connection, and goes to every watching connection when the watcher loses notifications (`overflow`) or restarts (`watcher_restarted`).
- `{ "type": "workspace_watch_unavailable", "message": "..." }` tells a watching connection no watcher is running.

The web UI shares each socket's watch set between the file trees that follow changes. Each registers as an owner with a watch registry, one for the bound agent's socket and one for the hub socket's team watch, and sets only its own prefixes. The registry sends the union of every owner's prefixes, sends it again after every reconnect, and hands each owner only the changes under its own prefixes. Resync and unavailable frames go to every owner. No owner can replace or clear another's watches. An owner tied to one agent watches only while that agent is bound; on a bound-agent switch the registry sends the prefixes that still apply to the new agent's connection. The file trees watch their whole tree, an agent's on its own socket and the team tree through the hub's `watch_team`, so a file appearing or disappearing shows up without reopening its folder.

An artifact page watches over its own sockets the same way. `residuum.watch` sends the union of the page's team prefixes as the hub socket's `watch_team`, and an agent handle's `watch` sends its prefixes as that agent socket's `watch_workspace`. The SDK hands each handler only the changes under its own prefix, and every `workspace_resync` and `workspace_watch_unavailable`. An artifact that loads a folder once and then follows `residuum.watch` stays current, including with changes made by background sessions.

## Live reload

Artifact reloads come from the same change feed. When a batch touches `team/workbench/`, or the feed asks for a resync, the team workbench is rescanned and compared with the last scan; a file named `workbench/...` in an agent's own workspace does not count; an artifact counts as changed when its page, or any file in its folder, changed. Two watchers do this, and both send `artifact_updated` and `artifact_removed` frames, whatever the client watches:

- **The hub's own watcher** serves the hub WebSocket (see [Artifact events](hub-http.md#artifact-events)). It runs for as long as the hub does, so every connection hears about an artifact being written, changed or deleted with no agent running.
- **Each running agent's watcher** serves that agent's WebSocket, the connection the web UI keeps to the agent it has open.

An artifact page reloads itself on its hub socket's `artifact_updated` (see [The hub socket](#the-hub-socket)), unless it handles that event itself. The web UI's Workbench reads its list again on the same hub events, and marks the changed artifact "updating now". Saved data files sit beside artifacts and are not part of them, so an artifact saving its own state never reloads itself.

## Through the relay

The relay serves each user's artifacts at `<user>.workbench.agent-residuum.com`. Requests for that host bypass the relay's own routes entirely and go through the tunnel tagged `surface: "workbench"`; Residuum's tunnel client forwards them to the artifacts listener, and answers `503` with an explanation when the listener isn't running rather than falling back to the main listener. A socket open tagged the same way connects to the artifacts listener, which forwards `/api` sockets to the hub router as it does locally; when the listener isn't running, the open fails with a reason saying so. The tunnel advertises `workbench-sockets` for this, and a relay sends workbench socket opens only to hubs that advertise it (see [cloud-tunnel.md](cloud-tunnel.md#sockets-through-the-tunnel)). Requests of every method are forwarded with their bodies. The host is owner-only like the UI host: a signed-out page load redirects to sign-in and returns to the artifact afterwards, a signed-out script request or socket gets `401`, and another user's session gets `403`. It never gets the relay's instance switcher. The relay only forwards workbench requests to Residuum versions that advertise the `workbench-surface` capability, and workbench sockets only to versions that advertise `workbench-sockets`; older versions get a message asking to update. The relay announces both origins in the tunnel handshake, which is how the web UI learns the artifacts origin remotely.

Two limits apply through the relay and not locally: a response over 10 MB fails, and so does an HTTP call that takes longer than 25 seconds, a slow model call included. Sockets aren't limited this way. When the relay refuses a page's socket (a Residuum too old for workbench sockets, or a relay that doesn't forward them), the browser can't read why, so the SDK can only report `disconnected`: live reload, watches and session frames stop, and `residuum.sessions.start` rejects after 10 seconds, while HTTP calls keep working.
