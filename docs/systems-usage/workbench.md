# Workbench

The workbench is where the agent builds interactive artifacts for the user: charts, dashboards, calculators, explorers, and pages for choosing between options. Each artifact is an HTML page (`workbench/<name>.html`) or a folder (`workbench/<name>/index.html` plus the files it loads) in the workspace. The web UI lists artifacts at `/workbench` and shows one at `/workbench/<name>`.

## Intended use

**Agent:** activates the bundled `workbench` skill, writes a page or a folder with `write_file`, and tells the user the artifact's title and path. Editing an existing artifact is `read_file` then `edit_file`; an open artifact reloads by itself when any of its files change. An artifact talks to Residuum only through the `residuum` object injected into every HTML page it serves (see below).

**User:** opens **Workbench** from the header menu. The list shows every artifact by its `<title>`, newest edit first; an artifact's seam glows while the agent writes to it. Opening an artifact shows it under a slim bar with back, full view, and reload. **Full view** (the bar's button, or `F`) hides the Residuum UI so the artifact fills the window; `Esc` or the corner button returns. Full view is part of the URL (`/workbench/<name>?full`), so it survives reloads and bookmarks. Deleting an artifact from the list removes its page or folder and its data files.

## Files

| Path | Holds |
|------|-------|
| `workbench/<name>.html` | A single-page artifact. |
| `workbench/<name>/` | A folder artifact: `index.html` is its page, and any other files are loaded by relative URL. When both a page and a folder share a name, the folder wins. |
| `workbench/<name>.<anything>` | That artifact's saved data (for example `<name>.state.json`), written through the workspace file API. Not part of the artifact, not watched for reloads, deleted with the artifact. |

`<name>` is lowercase letters, digits, and single hyphens, at most 64 characters; anything else is ignored. Files over 8 MiB are refused. Symlinks are ignored, and a file must resolve inside its artifact's folder; names starting with `.` are never served.

## The artifacts listener

Artifacts are served by their own listener, never by the gateway's main listener: at startup Residuum binds the first free port after the gateway's (`7702` by default), skipping Teams' configured port and its default `7701` so enabling Teams later can't collide. The port is stable across restarts while the machine's ports don't change, which keeps each artifact's origin, and so its browser storage, stable. The listener is read-only: `/<name>/` is the artifact's page, `/<name>/<path>` a file in a folder artifact, and `/<name>` redirects to `/<name>/`. If no port can be bound, Residuum runs without it, logs the reason at `error`, and the Workbench page shows it.

`GET /api/workbench/info` tells the web UI where artifacts are: the local port and, when connected to the cloud relay, the UI and artifacts origins the relay announced. The UI frames artifacts from the relay's artifacts origin when it is itself being viewed through the relay, and from its own host on the artifacts port otherwise.

## The `residuum` object

| Call | Does |
|------|------|
| `residuum.fetch(path, init)` | Calls Residuum's API (`/api/...`) and returns a `Response`. A plain-object body is sent as JSON; an `ArrayBuffer`, typed array, or `Blob` is sent unchanged, not JSON-encoded. |
| `residuum.ask(promptOrRequest)` | One-shot call to the background small model: `POST /api/model/complete`. A string is shorthand for `{ prompt }`. Resolves to the response body; rejects with an `Error` on any non-2xx. |
| `residuum.on(type, handler)` | Streams the same live frames the web UI receives (`"*"` for all), except keepalives and the change feed's own frames, plus the bridge's `{ "type": "connection", "state": "connected" \| "disconnected" }` when the web UI's socket drops or returns. |
| `residuum.watch(prefix, handler)` | Follows workspace changes under `prefix` (see [Change feed](#change-feed)). The handler receives `workspace_changed` frames holding only the changes under its prefix, and every `workspace_resync`. Returns a function that stops watching. Throws a `TypeError` for an absolute path or one with `..`. |
| `residuum.sessions.start({ prompt, context?, skill?, model? })` | Starts an agent session for this artifact (see [Agent sessions](#agent-sessions)) and resolves to a handle `{ address, on(type, handler), send(text), stop() }`. |
| `residuum.embedded` | `false` when the page is opened outside the web UI; `fetch`, `ask`, and `sessions.start` then reject. |
| `residuum.artifact` | This artifact's own name, embedded when the artifacts listener serves the page. |
| `residuum.version` | Residuum's version, embedded the same way. Matches `GET /api/status`'s `version`. |
| `residuum.features` | A frozen array of feature ids this build supports, embedded the same way. Matches `GET /api/status`'s `features`. |
| `residuum.state.get()` / `residuum.state.set(value)` | Sugar over the workspace file API for the artifact's own `workbench/<name>.state.json`: `get()` resolves to the parsed value or `null` before the first `set()` and rejects on invalid JSON; `set(value)` writes `JSON.stringify(value)` unconditionally. |

The endpoint and event catalogue the agent works from is the skill's `references/api.md`.

## Model calls

`POST /api/model/complete` is a one-shot request/response call to the background `small` model (its configured fallback chain applies when `small` is unset). No agent, no tools, no memory, no identity files: the model sees only what the artifact sends. The request is `{ "prompt": "..." }` as shorthand for one user message, or `{ "system", "messages", "schema", "max_tokens", "temperature" }` for a full conversation; `schema` asks for structured output, returned as parsed `json` alongside the model's raw `content`. The artifact's own `max_tokens`/`temperature` win over the small tier's defaults when given. A malformed request answers `400`; a provider failure answers `502` with a plain-language error; a timeout answers `504`. Every call logs at `info` with the calling artifact's identity, the model, and token usage. `residuum.ask` is the SDK's wrapper around this endpoint.

## Agent sessions

An artifact that needs the agent to do work (write files, research, use tools) starts a session with `residuum.sessions.start`, which calls `POST /api/sessions` through the bridge. The bridge's `X-Residuum-Artifact` header is what makes it an artifact session: the endpoint refuses a start without it. The session is a full fork of the main agent in the `artifact` category, labelled `artifact:<name>`, and it runs, idles, and completes like any other session (see [background-tasks.md](background-tasks.md#artifact-sessions)). Its output stays with the artifact: it never reaches the main chat, the inbox, or notification channels on its own, and it cannot message the main agent (it can still file a user-inbox item when its task calls for one). It appears in the sessions sidebar under Artifacts, showing the artifact's name.

The handle follows the session through the live frames the artifact already receives:

- `on(type, handler)` gets only this session's `session_*` frames (`session_started`, `session_state_changed`, `session_broadcast_response`, `session_response`, `session_error`, `session_completed`, and the rest; `"*"` for all of them). Frames that arrived before the start request answered, such as `session_started` with the run id, are handed to each handler registered for their type when it is registered. It returns a function that removes the handler.
- `send(text)` messages the session through `POST /api/sessions/{address}/messages`; the session sees it as a message from this artifact, and its reply arrives as a `session_response`. It resolves to the delivery outcome (`live`, `queued`, or `resumed`: a message to a finished session starts a new run at the same address).
- `stop()` stops the session through `POST /api/sessions/{address}/stop`.

Failures reject with an `Error` carrying the gateway's plain-language message, plus `code` and `status` where the gateway gave them. `GET /api/sessions?artifact=<name>` lists the sessions an artifact started, live and finished. Closing the artifact does not stop its sessions.

## Security model

The web UI has no login of its own (the relay authenticates remote access), so anything running on the UI's origin can call every endpoint. Artifacts are agent-written pages that load third-party scripts, so they never get that origin:

- **Separate origin.** Artifacts run on the artifacts listener's origin (`localhost:7702` locally, `<user>.workbench.agent-residuum.com` remotely), framed with `sandbox="allow-scripts allow-same-origin allow-forms allow-modals allow-popups allow-downloads"`. `allow-same-origin` gives an artifact its own origin (browser storage, relative files), not the UI's. The browser won't let it read the gateway's responses, since the gateway sends no CORS headers.
- **Cross-site guard.** The gateway rejects state-changing requests and WebSocket upgrades whose `Sec-Fetch-Site` is anything but `same-origin` or `none` (or, without fetch metadata, whose `Origin` is `null`). An artifact's origin is the *same site* as the UI's (same host, or a sibling subdomain), so its direct requests arrive as `same-site` and are refused, as are any other website's.
- **Bridge.** `residuum.fetch` and `residuum.ask` are relayed by the web UI (`web/src/lib/workbench-bridge.ts`), which accepts messages only from the artifact's frame on the artifacts origin and refuses with `403` and a reason: writing secrets or agent keys; anything under `/api/config/raw` and `/api/providers/raw`; `/api/config/complete-setup`; `/api/shutdown`, `/api/update/check|apply|restart`, `/api/cloud/disconnect`; and writes under `/api/tracing/`. Paths outside `/api/` are refused. Every relayed request carries `X-Residuum-Artifact: <name>`, which the bridge sets itself, overwriting any value the artifact's own request supplied. The bridge relays at most 8 ordinary requests at a time per bridge (the web UI shows one artifact at a time, so one bridge), queueing the rest in the order they arrived; a `503` whose body is exactly the relay's `agent overloaded` text is retried up to 3 times with exponential backoff and jitter starting near 500 ms, since the relay refuses these before forwarding them, so a retry never duplicates a write. Any other `503` is returned to the artifact as-is. Model calls (`POST /api/model/complete`) get their own lane of at most 4 at a time, identified by route, so a burst of slow calls never holds up the artifact's ordinary requests; each gets its own abort signal, tracked per bridge, aborted when the bridge is torn down (the frame closes or navigates away).
- **Artifacts share one origin.** Every artifact runs on the same artifacts origin, so artifacts can read each other's browser storage. Anything that must stay private to an artifact belongs in the workspace, not the browser.

## Change feed

One watcher covers the whole workspace recursively, using the operating system's file notifications (inotify, FSEvents, ReadDirectoryChangesW through the `notify` crate). If native notifications can't start (the Linux inotify watch limit, an unusual filesystem), Residuum logs a `warn` naming the cause and polls the workspace every 2 seconds instead; hitting the watch limit later, as new folders appear, switches to polling the same way. If neither can start, Residuum logs an `error` and any view that starts watching is told live updates are off, which the web UI shows as an error notice. Symlinks are not followed.

Notifications are debounced into batches: a batch closes once 300 ms pass with no new notification, or 2 s after its first one while writes continue. Each changed path appears once per batch as `created`, `modified`, or `removed`, decided from what the batch saw and what is at the path when the batch closes:

- A rename is `removed` for the old path and `created` for the new one. Residuum's own atomic writes (a temporary file renamed over the target) show up as `created` or `modified` for the target, never as the temporary file.
- A path missing when the batch closes is `removed`, even one created and deleted inside the batch.
- A folder's `created` or `removed` stands for everything inside it; a rename or removal of a whole folder is reported for the folder alone. A folder whose only change is its own timestamp is left out, since its files report the real changes.
- Paths the workspace access policy hides (any `.index` segment, database files and their sidecars, atomic-write temporaries) never appear.
- Reads never produce changes, so an artifact that rereads a file on each change can't feed its own loop.

When the OS reports lost notifications (queue overflow, rescan), notifications back up past Residuum's buffer, or one batch touches more than 10,000 paths, the batch becomes a resync instead of a change list. Restarting the watcher (after the watch limit, or when the workspace folder itself disappears) sends a resync too.

Each WebSocket connection has its own watch set of workspace-relative prefixes, empty by default, so the main chat never receives change frames:

- The client frame `{ "type": "watch_workspace", "prefixes": ["wiki", "inbox/user"] }` replaces the set. `[]` stops watching; `""` watches the whole workspace. A prefix that is absolute or contains `..` is refused with an `error` frame and the set is left unchanged. A connection may watch up to 256 prefixes of up to 1 KiB each.
- Prefixes match by path segment: `wiki` matches `wiki` and anything under `wiki/`, never `wikipedia/`. A prefix naming a file matches only that file, and a prefix that doesn't exist yet matches once it appears. A change to a folder that contains a prefix (for example `projects` for the prefix `projects/alpha`) matches too, since renaming or removing that folder carries the prefix with it.
- `{ "type": "workspace_changed", "changes": [{ "path": "wiki/a.md", "kind": "created" }] }` carries a batch's changes under the connection's prefixes, sorted by path. A connection with no matching changes gets nothing.
- `{ "type": "workspace_resync", "reason": "overflow" | "watcher_restarted" }` means the connection's view may be stale. It replaces `workspace_changed` when more than 500 of a batch's changes match the connection, and goes to every watching connection when the watcher loses notifications (`overflow`) or restarts (`watcher_restarted`).
- `{ "type": "workspace_watch_unavailable", "message": "..." }` tells a watching connection no watcher is running.

The web UI shows one artifact at a time, so its connection's watch set is the open artifact's watched prefixes, empty when no artifact is open, and it sends the set again after every reconnect. The bridge delivers `workspace_changed` and `workspace_resync` frames to the artifact only for the prefixes it watches, and after a reconnect sends a watching artifact `workspace_resync` with `reason: "reconnected"`, since changes during the gap are lost. An artifact that loads a folder once and then follows `residuum.watch` stays current, including with changes made by background sessions.

## Live reload

Artifact reloads come from the same change feed. When a batch touches `workbench/`, or the feed asks for a resync, the gateway rescans the workbench; an artifact counts as changed when its page, or any file in its folder, changed. Web UI clients receive `artifact_updated` and `artifact_removed` frames, whatever they watch. An open artifact reloads in place, keeping full view. Saved data files sit beside artifacts and are not part of them, so an artifact saving its own state never reloads itself.

When a page loads in the artifact's frame, the bridge forgets what the previous page subscribed to and watched: a page with the SDK announces itself before its own scripts run, so what it sets up while loading is kept, and a page without the SDK starts with nothing.

## Through the relay

The relay serves each user's artifacts at `<user>.workbench.agent-residuum.com`. Requests for that host bypass the relay's own routes entirely and go through the tunnel tagged `surface: "workbench"`; Residuum's tunnel client forwards them to the artifacts listener, and answers `503` with an explanation when the listener isn't running rather than falling back to the main listener. The host is owner-only like the UI host, read-only, and never gets the relay's instance switcher. The relay only forwards workbench requests to Residuum versions that advertise the `workbench-surface` capability; older versions get a page asking to update. The relay announces both origins in the tunnel handshake, which is how the web UI learns the artifacts origin remotely.
