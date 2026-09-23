# Workbench Artifacts — Design

> **Status:** design, not built. Phases: [`workbench-artifacts-phases.md`](./workbench-artifacts-phases.md).

> Systems level only. No file or line references. This document stands on its own: it is implemented in fresh sessions that have only this doc, the phases doc, and the codebase.

## Goal & context

The workbench is where the agent builds interactive pages for the user: charts, dashboards, calculators, explorers, pickers. Today those pages are called "tools", which collides with the agent's own tools (`write_file`, `exec`, ...). In the agent's skill, in the code, and in conversation, "tool" means two different things. The pages become **artifacts**.

Artifacts show themselves well but work poorly with the user's data:

- **Loading is slow and fragile.** Reading a folder takes one request per directory and one per file. Through the cloud relay each request crosses the tunnel, and the relay refuses an instance's requests past 50 in flight (`503 agent overloaded`), so an artifact that loads a 300-page wiki partly fails remotely (issue #189).
- **Artifacts go stale.** An artifact can't learn that the agent changed the files it shows. It can only refetch after each of the agent's turns, which misses background sessions (issue #190).
- **The file API is thin.** No delete, rename, or folder creation, no binary files, no protection against an artifact and the agent overwriting each other's edits. Reading a non-UTF-8 file fails with a 500.
- **Talking to the agent is one-way and gated.** `residuum.send` drops a message into the main chat, only right after a click, and the artifact never sees the reply.

This work renames tools to artifacts and gives artifacts a coherent API: bulk reads plus a change feed so an artifact can load a subtree once and stay current; full, safe file handling; one-shot model calls; agent runs as sessions the artifact can follow; memory search; inbox items; and feature detection. Instead of gating what artifacts may do, the user gets visibility and a stop control over everything an artifact starts.

## Terms

- **Workbench** — the place: the web UI section at `/workbench`, the workspace folder `workbench/`, the relay's `*.workbench.*` host, and the bundled `workbench` skill. These keep their names.
- **Artifact** — one page the agent built: `workbench/<name>.html`, or a folder `workbench/<name>/` with an `index.html`. `<name>` rules are unchanged. An artifact's data files (`workbench/<name>.<anything>`) are not part of the artifact.
- **Artifacts listener** — the separate, read-only HTTP listener that serves artifacts on their own origin (today's "tools listener").
- **SDK** — the `residuum` object injected into every HTML page the artifacts listener serves.
- **Bridge** — the web UI component that receives SDK requests from an artifact's frame over `postMessage` and makes them on the artifact's behalf. It is the only way an artifact reaches the gateway.
- **Artifact identity** — the artifact name the bridge attaches to every request it relays (see "Artifact identity" below).
- **Version token** — an opaque string identifying one version of a workspace file, used for conditional writes.
- **Workspace access policy** — the single rule deciding which workspace paths the HTTP file API and the change feed never expose.

## Shape

### 1. Naming

Everything that names a workbench page "tool" is renamed to "artifact": Rust types and functions, HTTP routes, WebSocket frames, generated TypeScript types, web UI components, routes' parameter names, CSS classes, SDK comments, user-facing strings, the `workbench` skill and its API reference, the `residuum-system` skill, the bootstrap `AGENTS.md`, systems-usage docs, `web/CONTRIBUTING.md`, and the mock server. Wire-level renames:

| Before | After |
|---|---|
| `GET /api/workbench/tools` | `GET /api/workbench/artifacts` |
| `DELETE /api/workbench/tools/{name}` | `DELETE /api/workbench/artifacts/{name}` |
| frame `workbench_tool_updated` / `workbench_tool_removed` | `artifact_updated` / `artifact_removed` |
| `WorkbenchToolSummary` | `ArtifactSummary` |
| `WorkbenchRelayOrigins.tools_origin` | `artifacts_origin` |
| "tools listener" | "artifacts listener" |

Things that name the *place* stay: `/workbench` UI routes, `workbench/` folder, the `workbench` skill name, the tunnel's `Surface::Workbench`, the `workbench-surface` capability, the `workbench_origin` handshake field, and the relay's workbench host. The relay repository only changes user-facing copy that says "workbench tools". The workbench is unreleased, so no compatibility aliases are kept.

### 2. Workspace access policy

One module owns the rule for which workspace paths are hidden from the HTTP file API and the change feed. It matches on **path segments**, not substrings: any segment named `.index`, database files (`.db`, `.sqlite`) plus their sidecar files (`-wal`, `-shm`, `-journal`), and the temporary files Residuum's atomic writes create and rename away (`.<name>.<random>.residuum-tmp`). Every file endpoint (listing, read, write, raw, tree, batch read, delete, mkdir, move) and the change feed consult it. A blocked path answers `403` on direct access and is silently absent from listings, trees, and change events. The index and databases are Residuum's own data, open while it runs, so a recursive delete, a directory move, or a move that would overwrite a directory answers `403` when the directory holds any of them at any depth; everything else in the workspace stays open.

Every path a request names (including the root `path` of a tree request) keeps today's rule: it is canonicalized and must stay inside the workspace. Recursive walks (tree) never follow symlinks they encounter below that root and omit them.

The policy does not protect `config/mcp.json` or any other workspace file. Artifacts may read and write MCP configuration like any other file.

### 3. Workspace file API

All handlers do filesystem work off the async runtime (blocking pool or async fs). All writes are atomic: write a temporary file in the target directory, then rename over the target. Writes create missing parent directories inside the workspace. Existing post-write behavior applies to every write path, including raw writes and moves: writing an identity file (`SOUL.md`, `AGENTS.md`, `USER.md`, `HEARTBEAT.yml`) sends the workspace reload signal, and `HEARTBEAT.yml` content is validated before it is accepted.

**Version tokens.** Every file read, listing entry, tree entry, and batch-read entry carries `version`, derived from the file's modification time (nanosecond precision where the platform has it) and size. Single-file reads also return it as the `ETag` header. Listings also gain `modified` (Unix milliseconds).

**Conditional writes.** Every write-type request (text write, raw write, delete of a file, move source) accepts `If-Match: <version>`; a mismatch answers `412` with `{ "error": "...", "current_version": "<version>" | null }` (null when the file no longer exists). `If-None-Match: *` on a write means "create only" and answers `412` if the file exists. Without either header, writes are unconditional. Successful writes return the new `version`.

**Endpoints** (paths are workspace-relative; empty means the workspace root):

| Method and path | Contract |
|---|---|
| `GET /api/workspace/files?path=<dir>` | Unchanged shape plus `modified` and `version` per entry. |
| `GET /api/workspace/file?path=<file>` | Text read. `404` missing, `413` over 8 MiB, `415` when the file is not valid UTF-8 (message points to the raw endpoint). |
| `PUT /api/workspace/file` | Body `{ path, content }`, content up to 8 MiB (the route's body limit is raised to match; larger answers `413`). Atomic, creates parents, conditional headers honored. Returns `{ saved: true, version }`. |
| `GET /api/workspace/raw?path=<file>` | Bytes with a `Content-Type` guessed from the extension and an `ETag`. `413` over 8 MiB. |
| `PUT /api/workspace/raw?path=<file>` | Request body is the file's bytes, up to 8 MiB (the route's body limit is raised to match). Same write rules. Returns `{ saved: true, version }`. |
| `DELETE /api/workspace/file?path=<p>&recursive=<bool>` | Deletes a file, or a directory when `recursive=true` (a directory without it answers `409`). The workspace root cannot be deleted (`400`). `If-Match` applies to files. |
| `POST /api/workspace/dir` | Body `{ path }`. Creates the directory and its parents. Idempotent. |
| `POST /api/workspace/move` | Body `{ from, to, overwrite?: bool }`. Moves a file or directory, creating `to`'s parents. `409` if `to` exists and `overwrite` is not true. `If-Match` applies to `from` when it is a file. |
| `GET /api/workspace/tree` | See "Bulk reads". |
| `POST /api/workspace/read` | See "Bulk reads". |

### 4. Bulk reads (#189)

**Tree.** `GET /api/workspace/tree?path=<dir>&content=<bool>&glob=<pattern>&depth=<n>` walks a directory recursively and returns a flat list:

```
{
  "path": "wiki",
  "entries": [
    { "path": "wiki/a.md", "type": "file", "size": 120, "modified": 1790000000000,
      "version": "...", "content": "..." },
    { "path": "wiki/img.png", "type": "file", "size": 90000, "modified": ..., "version": "...",
      "skipped": "binary" },
    { "path": "wiki/sub", "type": "directory", "modified": ... }
  ],
  "listing_truncated": false,
  "content_truncated": false
}
```

- Entries are sorted by path. Paths are workspace-relative.
- `glob` may repeat. A pattern without `/` matches a file name at any depth (`*.md`); a pattern with `/` matches the path relative to `path` (`notes/**/*.md`). With any `glob`, only matching files are returned and directory entries are omitted.
- `depth` limits recursion (1 = direct children). Default unlimited.
- With `content=true`, each file includes `content` when it is valid UTF-8, at most 1 MiB, and fits the response budget. Otherwise it carries `skipped: "binary" | "too_large" | "budget"` and still has its metadata.
- The listing stops at 20,000 entries and sets `listing_truncated`. Any `skipped: "budget"` sets `content_truncated`.
- The whole serialized response stays under 8 MiB, leaving headroom under the relay tunnel's 10 MB response limit.
- Blocked paths and symlinks are absent. A blocked or missing `path` answers `403`/`404`.

**Batch read.** `POST /api/workspace/read` with `{ "paths": [...] }` (at most 1,000) reads a chosen set of files, typically the paths from a change event:

```
{ "files": [ { "path": "wiki/a.md", "size": 120, "modified": ..., "version": "...", "content": "..." },
             { "path": "wiki/gone.md", "error": "not_found" } ],
  "content_truncated": false }
```

Order matches the request. Per-file `error` is one of `not_found`, `blocked`, `is_directory`, `binary`, `too_large`, `budget`. The same 1 MiB per-file and 8 MiB response limits apply. One bad path never fails the request.

### 5. Change feed (#190)

**Watcher.** One filesystem watcher covers the whole workspace recursively, using OS change notifications (the `notify` crate with its debouncer; pin the current stable release, not a release candidate, after checking the project is maintained). If the native watcher cannot start (for example the Linux watch limit), Residuum falls back to notify's polling watcher and logs a `warn` naming the cause. If the watcher fails entirely, it logs an `error` and the web UI shows a notice that live updates are off. Raw events are debounced: changes coalesce until 300 ms pass with no new event, or 2 s after the first event during continuous writes. Paths hidden by the workspace access policy are dropped. A rename becomes `removed` for the old path and `created` for the new one. When the OS reports lost events (queue overflow, rescan), the watcher emits a resync instead of a change list. Batches are published on the bus.

**Subscriptions.** Each WebSocket connection has its own watch set of path prefixes, empty by default:

- Client frame `{ "type": "watch_workspace", "prefixes": ["wiki", "inbox/user"] }` replaces the connection's set. `[]` turns watching off. `""` means the whole workspace. Prefixes are workspace-relative directory paths; one with `..` or an absolute path is refused with an `error` frame and the set is left unchanged.
- Server frame `{ "type": "workspace_changed", "changes": [{ "path": "wiki/a.md", "kind": "created" | "modified" | "removed" }] }` carries only the batch's changes under the connection's prefixes. A connection with no matching changes gets nothing.
- Server frame `{ "type": "workspace_resync", "reason": "overflow" | "watcher_restarted" }` tells the connection its view may be stale. It is sent *instead of* `workspace_changed` (the batch's changes are dropped for that connection) when a batch has more than 500 matching changes for that connection, and to every watching connection when the watcher reports lost events or restarts.
- **Prefix matching is by path segment.** Prefix `wiki` matches `wiki` itself and anything under `wiki/`, never `wikipedia/`. A prefix naming a file matches only that file. A prefix that doesn't exist yet is allowed and matches once it appears.

Connections that never send `watch_workspace` receive no workspace frames, so the main chat UI pays nothing.

**Artifact reloads.** The 1-second polling watcher over `workbench/` is removed. Artifact reload events derive from the same change stream: a change under `workbench/` that belongs to an artifact (its page, or any file in its folder, per the existing artifact rules) produces `artifact_updated { name }` or `artifact_removed { name }`, broadcast to every connection as today. Data files beside artifacts still never trigger reloads.

**In the web UI.** The web UI shows one artifact at a time, so there is at most one bridge. The WebSocket coordinator owns the connection's watch set: the open artifact's watched prefixes (empty when no artifact is open), re-sent whenever it changes and after every reconnect. The bridge delivers `workspace_changed` and `workspace_resync` frames to an artifact only for prefixes that artifact watches. After a reconnect the bridge sends each watching artifact `workspace_resync { reason: "reconnected" }`, since events during the gap are lost.

**Connection state.** The bridge also sends artifacts a bridge-generated frame `{ "type": "connection", "state": "connected" | "disconnected" }` when the web UI's socket changes state, so artifacts can pause and resync.

### 6. Artifact identity

The SDK knows its own artifact's name: the artifacts listener injects the SDK and embeds the artifact name (and the Residuum version and feature list, see "Feature detection") into the injected script. It is exposed as `residuum.artifact`.

The bridge knows which artifact its frame shows, and attaches `X-Residuum-Artifact: <name>` to every request it relays, replacing any value the artifact supplied. Gateway endpoints that attribute work to an artifact read this header. The gateway's existing cross-site guard means only the web UI's own origin can make state-changing requests, so the header is set by web UI code, not by the artifact.

### 7. Model calls

`residuum.send` is removed. Its replacement for one-shot work is a model call: one request to the **small** background model, one answer back. No agent, no tools, no memory, no identity files. The model sees only what the artifact sends.

`POST /api/model/complete`:

```
request:  { "prompt": "..." }                      // shorthand for one user message
       or { "system": "...",
            "messages": [ { "role": "user" | "assistant", "content": "...",
                            "images": [ { "media_type": "image/png", "data": "<base64>" } ] } ],
            "schema": { ...JSON Schema... },       // optional: structured output
            "max_tokens": 1024, "temperature": 0.2 }
response: { "content": "...", "json": { ... },     // json only when schema was given
            "model": "provider/model",
            "usage": { "input_tokens": 0, "output_tokens": 0 } }
```

- **Model:** the background `small` tier with its existing fallback (small → medium → large → main) and the `bg_small` temperature/thinking overrides. The artifact's own `temperature` and `max_tokens` win when given. The resolution follows config reloads: a call made after a providers change uses the new models.
- **Structured output:** `schema` maps to the providers' native JSON-schema output. The response's `json` is the parsed content; if the provider returns unparsable content despite a schema, the call answers `502`.
- **Errors:** `400` for a malformed request (no prompt/messages, bad role, empty content); `502` with a plain-language `error` when the provider fails after its built-in retries; `504` when the provider times out. Raw provider errors go to logs, not to the artifact.
- **Observability:** every call logs at `info` with the artifact identity (or `web-ui` without one), model, and token usage.
- **Cancellation:** closing the HTTP request cancels the call where the transport allows it. Locally, dropping the request cancels the provider request; through the relay, a call already in flight may run to completion.

SDK: `residuum.ask(promptOrRequest)` resolves to the response object and rejects with an `Error` carrying the `error` message on any non-2xx.

### 8. Artifact sessions

Agent work goes through the existing sessions system. An artifact starts a session and follows it through the live stream it already receives.

**New session category `artifact`.** A session an artifact starts has trigger `Artifact(<name>)`, category `artifact`, source label `artifact:<name>`, no spawner, and depth 1. It gets the same tool registry and fork contents as a spawned session. Its idle timeout is a new per-category setting beside the existing three, defaulting to 10 minutes. It shares the existing background concurrency limit.

**Results stay with the artifact, never the chat.** An artifact session's turn output is never relayed to the main agent, and its completion is not routed to the inbox or notification channels automatically. Its output reaches the artifact through the session frames (`session_response`, `session_broadcast_response`, `session_completed`, ...) and the transcript endpoint, and it appears in the sessions list like any other session. The session keeps its normal tools with one exception: it cannot message the main agent (`message_agent` to `main` fails with a tool error saying artifact sessions can't reach the main conversation, and to file an inbox item instead). It can add items to the user's inbox with its inbox tool when its task calls for it. Sessions it spawns itself relay to it as usual, never to main.

**Endpoints:**

| Method and path | Contract |
|---|---|
| `POST /api/sessions` | Body `{ prompt, context?, skill?, model?: "small" \| "medium" \| "large" }` (default `medium`). Requires the artifact identity header; without it `400` (only this endpoint requires the header). Answers `202` with `{ address }`. The run id arrives in the `session_started` frame for that address. |
| `POST /api/sessions/{address}/stop` | Same semantics as the `session_stop` WebSocket command. `202` when stopping; `404` when the session is not live; `400` for `main`. |
| `POST /api/sessions/{address}/messages` | Body `{ content }`. Same delivery semantics as `session_send_message`: `200` with `{ outcome: "live" \| "queued" \| "resumed" }`. Failures answer `{ error, code }` with the WebSocket command's code and a status per code: `invalid_request` `400`, `unknown_address` and `not_live` `404`, `busy` `409`, `delivery_failed` `502`. **Sender:** with the artifact identity header, the message is attributed to the artifact (sender `artifact:<name>`), and the session sees it as a message from that artifact, not from the owner. Without the header it is attributed to the owner, as the WebSocket command is. |
| `GET /api/sessions?artifact=<name>` | New filter on the existing listing: sessions whose trigger is that artifact. |

The HTTP stop and message endpoints serve any non-main session, not only artifact sessions.

The new idle timeout is a configuration value, so it appears in the web UI's settings beside the other per-category idle timeouts.

SDK: `residuum.sessions.start({ prompt, context?, skill?, model? })` resolves to a handle `{ address, on(type, handler), send(text), stop() }`. `on` receives the session frames for that address only.

### 9. Visibility and stop controls

Nothing limits how many model calls or sessions an artifact runs. Instead, the user can always see what an artifact is doing and stop it.

- **Sessions list.** Every session row whose state is stoppable (forking, running, idle) gets a stop button, in every category. Artifact sessions are grouped under their own category and show which artifact started them, linking to it.
Stopping is granular: nothing stops every session at once, and stopping the page is separate from stopping its sessions.

- **Artifact activity panel.** The bar above an open artifact shows an activity indicator (live sessions it started, model calls in flight). Opening it lists each live session the artifact started (from `GET /api/sessions?artifact=<name>` kept current by session frames) with its purpose, state, a link to its session view, and its own stop button. Model calls in flight (tracked by the bridge) are shown as a count with a **Cancel calls** action; calls are short and interchangeable, so they are cancelled together.
- **Stop page.** A separate control on the bar unloads the artifact's frame into a stopped state with a **Restart** button, aborting its in-flight model calls. It does not stop the artifact's sessions; they stay listed in the activity panel and the sessions list for individual stopping. Unloading ends any loop running in the page, and a looping page can no longer start new work.
- **Closing an artifact does not stop its sessions.** Background runs outlive the page by design. Model calls from a frame that closes are aborted, since nothing is left to receive them.

### 10. Bridge

- **Relay-safe request flow.** The bridge relays at most 8 ordinary requests at a time and queues the rest in order. Model calls (any relayed request to `/api/model/complete`) use a separate limit of 4, so slow calls never hold up ordinary requests; the two limits together stay well under the relay's 50. A `503` whose body is the relay's `agent overloaded` is retried up to 3 times with exponential backoff and jitter (starting near 500 ms); the relay refuses those requests before forwarding them, so a retry never duplicates a write.
- **Binary bodies.** Requests may carry `ArrayBuffer`, typed-array, or `Blob` bodies; the SDK's `fetch` accepts them alongside strings and plain objects.
- **Blocked routes.** Still refused: writing secrets or agent keys; `/api/config/raw` and `/api/providers/raw`; `/api/config/complete-setup`; shutdown, update, and restart; cloud disconnect; writes under `/api/tracing/`. No longer refused: `/api/mcp/raw`, and deleting artifacts (the file API can already delete their files).
- **No gesture requirement.** Nothing in the bridge depends on user activation.
- **Model-call tracking.** The bridge identifies model calls by route (`/api/model/complete`), gives each an abort signal, and tracks them per frame. It aborts them when the user cancels calls or stops the page (§9), or when the bridge is torn down because the frame closes or navigates away.

### 11. Inbox, memory search, feature detection, state

- **Inbox.** `POST /api/agent-inbox` with `{ title?, body }` adds an item to the agent's inbox (the same place the `/inbox` command writes). The title defaults to the body's first line, cut to 60 characters. Blank bodies answer `400`. The source is `artifact:<name>` (or `web` without the identity header). It answers `{ id }`. Artifacts cannot add to the user's inbox directly and have no way to post into the main conversation.
- **Memory search.** `GET /api/memory/search?q=<query>&limit=<1..50, default 10>&source=observations|episodes|wiki&date_from=YYYY-MM-DD&date_to=YYYY-MM-DD` runs the same hybrid search the agent's `memory_search` tool uses and answers `{ results: [{ id, source, episode_id, date, line_start, line_end, snippet, score }], semantic: bool }`, where `semantic` says whether vector search contributed. Blank `q` answers `400`.
- **Feature detection.** One list of feature ids is defined in one place. `GET /api/status` answers `{ mode, version, features }`, and the SDK exposes the same `residuum.version` and `residuum.features` (embedded at injection). Initial ids: `workspace-tree`, `workspace-read-batch`, `workspace-watch`, `workspace-raw`, `workspace-conditional-write`, `workspace-file-ops`, `model-complete`, `artifact-sessions`, `memory-search`, `inbox-add`, `artifact-state`. A phase that ships a capability adds its id.
- **Artifact state.** SDK sugar over the text file endpoints; no new route. `residuum.state.get()` reads `workbench/<artifact>.state.json` and resolves to the parsed value, `null` when the file is absent, and rejects with an `Error` when the file holds invalid JSON. `residuum.state.set(value)` writes `JSON.stringify(value)` unconditionally (no `If-Match`; last write wins). Artifacts needing conflict detection use the file API with `If-Match`.

### SDK surface when done

| Member | Does |
|---|---|
| `embedded`, `artifact`, `version`, `features` | Context. |
| `fetch(path, init)` | Relayed API call; bodies may be strings, plain objects/arrays (JSON), or binary. |
| `on(type, handler)` | Live frames, including bridge-generated `connection` frames. |
| `watch(prefix, handler)` | Change feed for a prefix; handler receives `workspace_changed` and `workspace_resync` frames. Returns an unsubscribe function. |
| `ask(promptOrRequest)` | Model call. |
| `sessions.start(options)` | Agent run; returns a session handle. |
| `state.get()`, `state.set(value)` | The artifact's own saved state. |

## Reasoning & alternatives

- **"Artifact" and keeping "workbench".** "Artifact" is the word agent users already know for a page an agent built, and it fits Residuum's stone-and-relic imagery. The workbench remains the place artifacts live, so the relay host, tunnel surface, and folder keep their names and the relay needs only copy changes.
- **One watcher, per-connection filters.** Watching per subscription would need reference counting across connections and start/stop churn. One recursive OS watcher over the workspace is cheap; filtering per connection keeps unsubscribed clients free of traffic. Native notifications replace polling because polling a whole workspace every second doesn't scale. The polling fallback covers watch limits and unusual filesystems.
- **Resync over replay.** Replaying missed events needs an event log and cursors. A resync signal plus a cheap tree or batch read gives the same result with far less machinery, and it covers overflow, watcher restarts, and reconnects with one mechanism.
- **Truncate, don't fail.** A bulk read that hits a limit returns everything that fit plus markers naming what didn't, so an artifact fetches the rest instead of starting over.
- **Batch read beside the tree.** A change event names paths; one batch read refreshes exactly those, keeping change handling to one request.
- **Modification time and size as the version.** It is free to compute for listings and trees, unlike a content hash. The residual risk (two writes of equal size within one timestamp tick) needs coarse-timestamp filesystems to matter.
- **Model calls on the small model.** The primary model is chosen for being a good agent; summarizing, classifying, and extracting don't need that. The small tier's fallback chain means a call always has a model.
- **Sessions for agent work.** Sessions already stream progress, accept messages, stop, persist transcripts, and appear in the UI. Artifact sessions add a start path and a category rather than a second agent-run mechanism.
- **A new category over reusing `spawned`.** Spawned sessions relay results to their spawner and are dropped by the inbox router because of that relay. Artifact sessions have different delivery (stay with the artifact) and different UI grouping; a category states that honestly instead of relying on a missing spawner.
- **Stop controls over caps.** The agent guardrail stance is default-open. Caps on concurrent calls or sessions would block legitimate batch work. Granular stops (each session on its own, the page separately) let the user end a runaway loop without losing the sessions that were doing useful work; all-or-nothing stopping would force that trade.
- **Header identity set by the bridge.** Putting the artifact name in paths would let one artifact act as another by changing a URL; the bridge knows which frame is talking and stamps it.
- **Bridge-side request limiting.** Every artifact benefits without code changes, and it prevents the relay's 50-request limit from turning a big load into partial failure.

## External touchpoints

- **Relay (separate repository).** Unchanged protocol. Relayed responses are buffered and capped at 10 MB, and the relay answers `503 agent overloaded` past 50 in-flight requests per instance after a 5 s wait; bulk-read limits and bridge retry are sized against these. Only user-facing copy mentioning "workbench tools" changes.
- **Model providers.** Model calls use the existing provider abstraction with no tools and optional JSON-schema output; all four provider adapters support structured output. Calls are non-streaming. Provider retries are the adapters' built-in retries.
- **Operating system file notifications.** inotify on Linux, FSEvents on macOS, ReadDirectoryChangesW on Windows, via `notify`. Linux watch limits and network filesystems are the known failure modes, covered by the polling fallback. All four release targets must build; the change is dependency-bearing, so the cross-compile workflow runs on its PR.
- **Browser.** Artifacts run in a sandboxed frame on the artifacts origin; SDK ↔ bridge messages use `postMessage` with the existing tag and origin checks.
- **Generated TypeScript protocol types.** New and renamed protocol types are exported through the existing ts-rs export test and added to the web UI's hand-maintained protocol barrel.

## Integration with existing system

- **Replaces:** the workbench polling watcher (by the workspace watcher); `residuum.send` and the gesture check (by model calls and artifact sessions); the substring blocked-path check (by the workspace access policy); "tool" naming.
- **Extends:** the workspace file API (versions, conditional writes, atomic writes, new operations); the sessions system (category, trigger, HTTP start/stop/message, artifact filter); `/api/status`; the inbox API; the bridge and SDK.
- **Leaves untouched:** the artifacts listener's serving rules and security model (separate origin, sandbox, cross-site guard); the relay protocol; the main chat; the agent's own file tools and their write policy; session categories other than the new one.
- **Documentation.** `docs/systems-usage/workbench.md` describes the resulting system, and each phase updates the systems-usage pages whose behavior it changes (workbench, sessions/background, inbox, memory as relevant) along with the bundled `workbench` skill's API reference and the `residuum-system` skill's mirrors. When the work ships, this document and the phases document move to `docs/archive/`.
