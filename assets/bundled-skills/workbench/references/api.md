# Workbench API Reference

What a workbench artifact can reach through `residuum.fetch`, `residuum.ask`, `residuum.on`, `residuum.watch`, `residuum.agent`, and `residuum.sessions`. Read endpoints return JSON unless noted. Every `path` is on the artifact page's own origin, which forwards `/api/...` to Residuum, and names its scope: `/api/team/...`, `/api/hub/...`, or `/api/agents/<agent>/...`.

## Context

Three values are embedded into the page when it loads, not fetched: `residuum.artifact` is the artifact's own name, `residuum.version` is Residuum's version, and `residuum.features` is a frozen array of feature ids this build supports. Check a feature id before relying on the capability it names, since an older Residuum build won't have it.

## Artifact State

`residuum.state.get()` and `residuum.state.set(value)` are sugar over the team file API for one file, `team/workbench/<name>.state.json` (`workbench/<name>.state.json` in the team API's paths) — no separate endpoint. `get()` resolves to the parsed value, `null` when the file doesn't exist yet, and rejects with an `Error` if the saved content isn't valid JSON. `set(value)` writes `JSON.stringify(value)` unconditionally (last write wins); use `residuum.fetch` against `/api/team/workspace/file` directly with `If-Match` for conflict detection.

## Endpoints Worth Calling

The team's files (the shared wiki, workbench, skills) are under `/api/team/workspace/...`, with every path relative to the team folder (`wiki/people/sam.md`, not `team/wiki/people/sam.md`). The rows for everything an agent owns (sessions, inbox, memory, status, model calls) are under `/api/agents/<agent>/...`, where `<agent>` is the name of the agent that answers them; `GET /api/hub/agents` lists the agents. An unknown agent answers `404`, and an agent that isn't running answers `409` with its `state` on every route except the ones that only read or write its files: `chat/history`, `usage`, the user-inbox routes (`inbox...`, not `agent-inbox`), and the workspace routes, which also answer for a stopped agent.

An artifact belongs to no agent, so a path that belongs to an agent but names none (`/api/status`, `/api/inbox`) is not sent: `residuum.fetch` resolves to a `400` whose `error` gives the `/api/agents/<agent>/...` form. The unscoped spellings of hub routes (`/api/secrets`, `/api/system/timezone`, …) and `/api/workbench/...` still reach `/api/hub/...` and `/api/team/workbench/...`.

| Method and path | Returns / does |
|-----------------|----------------|
| `GET /api/team/workspace/files?path=<dir>` | Directory listing: `[{ name, entry_type: "file" \| "directory", size, modified, version }]`. `modified` is Unix milliseconds; `version` is an opaque token for conditional writes. Omit `path` for the workspace root. Paths under `.index`, or Residuum's own `memory/vectors.db` (and its `-wal`/`-shm`/`-journal` sidecars), are never listed — a user's own `.db`/`.sqlite` file elsewhere in the workspace is visible like any other file. |
| `GET /api/team/workspace/file?path=<file>` | The file's text (not JSON), with an `ETag` header carrying its version. 404 if missing, 413 over 8 MiB, 415 if the file isn't valid UTF-8 (read it from `/api/team/workspace/raw` instead). |
| `PUT /api/team/workspace/file` | Body `{ path, content }` writes a text file, up to 8 MiB. Creates missing parent directories. `If-Match: <version>` answers `412` on a stale write; `If-None-Match: *` answers `412` if the file already exists. Returns `{ saved: true, version }`. |
| `GET /api/team/workspace/tree?path=<dir>&content=<bool>&glob=<pattern>&depth=<n>` | Recursively lists `path` (the workspace root when omitted) as a flat, path-sorted `{ path, entries, listing_truncated, content_truncated }`. Each entry carries `path`, `type: "file" \| "directory"`, `size` (files only), `modified`, `version`, and — with `content=true` — either `content` (UTF-8 files up to 1 MiB, response budget permitting) or `skipped: "binary" \| "too_large" \| "budget"`; a skipped entry still has its metadata. `glob` may repeat: a pattern without `/` matches a file name at any depth, one with `/` matches the path relative to `path`; with any `glob`, only matching files are returned and directories are omitted. `depth` limits recursion (`1` = direct children only; unlimited when omitted). Symlinks below `path` and blocked paths never appear. `403`/`404` for a blocked or missing `path`, `400` if it isn't a directory or a `glob` pattern doesn't parse. Needs the `workspace-tree` feature. |
| `POST /api/team/workspace/read` | Body `{ paths: [...] }` (no limit on how many) reads exactly those files, in order: `{ files: [{ path, size, modified, version, content } \| { path, size?, modified?, version?, error }], content_truncated }`. `error` is `not_found`, `blocked`, `is_directory`, `binary`, `too_large`, or `budget`; a path that escapes the workspace counts as `blocked`. One bad path never fails the whole request. Needs the `workspace-read-batch` feature. |
| `GET /api/team/workspace/raw?path=<file>` | The file's raw bytes (use for anything binary — images, audio, non-UTF-8 data), with a `Content-Type` guessed from its extension and an `ETag` header carrying its version. 404 if missing, 413 over 8 MiB. |
| `PUT /api/team/workspace/raw?path=<file>` | Body is the file's exact bytes, up to 8 MiB — send an `ArrayBuffer`, a typed array, or a `Blob` as `residuum.fetch`'s `body` and it goes over unchanged, not JSON-encoded. Same write rules as `PUT /api/team/workspace/file` (conditional headers, parent creation, atomic write). Returns `{ saved: true, version }`. |
| `DELETE /api/team/workspace/file?path=<p>&recursive=<bool>` | Deletes a file, or a directory when `recursive=true` (a directory without it answers `409`). The workspace root can't be deleted (`400`), nor can a directory holding Residuum's memory index or database files (`403`). `If-Match` applies to files. Returns `{ deleted: true }`. |
| `POST /api/team/workspace/dir` | Body `{ path }` creates a directory and its missing parents. Idempotent — creating one that already exists succeeds. Returns `{ created: true }`. |
| `POST /api/team/workspace/move` | Body `{ from, to, overwrite? }` moves or renames a file or directory, creating `to`'s missing parents. `409` if `to` already exists and `overwrite` isn't `true`. A directory holding Residuum's memory index or database files can't be moved or overwritten (`403`). `If-Match` applies to `from` when it's a file. Returns `{ moved: true, version }` (`version` is `null` for a moved directory). |
| `GET /api/agents/<agent>/inbox` | The user's inbox: `[{ id, title, body, source, timestamp, read, attachments }]`. |
| `PUT /api/agents/<agent>/inbox/<id>/read` | Marks an inbox item read. |
| `POST /api/agents/<agent>/inbox/<id>/archive` | Archives an inbox item. |
| `POST /api/agents/<agent>/agent-inbox` | Body `{ title?, body }` adds an item to the agent's own inbox (what the `/inbox` command and `inbox_list`/`inbox_read` work from) — there's no equivalent for the user's inbox. `title` defaults to the body's first line, in full. Blank `body` answers `400`. Returns `{ id }`. |
| `GET /api/agents/<agent>/memory/search?q=<query>&limit=<1..50, default 10>&source=observations\|episodes\|wiki&date_from=&date_to=` | The same hybrid search the `memory_search` tool runs. Returns `{ results: [{ id, source, episode_id, date, line_start, line_end, snippet, score }], semantic }`, where `semantic` says whether vector search contributed. Blank `q`, an unrecognized `source`, or a malformed date answers `400`. |
| `GET /api/agents/<agent>/sessions` | Agent sessions: `{ live, completed, next_cursor }`. Filters: `?category=scheduled\|external\|spawned\|artifact`, `?address=<address>`, `?artifact=<name>` (sessions that artifact started; use `residuum.artifact` for your own), `?before=<next_cursor>`, `?limit=<at least 1>` (default 50, no upper cap). |
| `POST /api/agents/<agent>/sessions` | Starts an agent session for this artifact; use `residuum.sessions.start` (see Agent Sessions). |
| `POST /api/agents/<agent>/sessions/<address>/messages` | Body `{ content }` messages a session. `200` with `{ outcome: "live" \| "queued" \| "resumed" }`; failures are `{ error, code }` (see Agent Sessions). |
| `POST /api/agents/<agent>/sessions/<address>/stop` | Stops a live session: `202`, `404` when it isn't running. |
| `GET /api/agents/<agent>/sessions/runs/<run_id>/transcript` | One session run's transcript. |
| `GET /api/agents/<agent>/chat/history` | Recent main-chat messages. |
| `GET /api/team/workbench/artifacts` | Every artifact: `[{ name, title, modified_at, size }]`. |
| `DELETE /api/team/workbench/artifacts/<name>` | Deletes an artifact's page (or folder) and its `<name>.*` data files, after checkpointing the team folder. Returns `{ removed: [...], checkpoint_id }`: what was deleted, and the team checkpoint that restores it (`null` when none could be recorded). `404` if it no longer exists. |
| `GET /api/agents/<agent>/status` | `{ mode, version, features }`: `mode` is `"running"` normally, `version` and `features` match `residuum.version` and `residuum.features`. |
| `GET /api/hub/system/timezone` | The system's timezone, as detected. |
| `POST /api/agents/<agent>/model/complete` | One-shot small-model call — see "Model Calls" below. |

The same file routes exist for each agent's own folder under `/api/agents/<agent>/workspace/...`: there, unprefixed paths are that agent's workspace and `team/`-prefixed paths are the shared team layer, so an artifact's data file is `team/workbench/<name>.state.json` in that namespace and `workbench/<name>.state.json` in the team API's.

`GET /api/team/workspace/tree` and `POST /api/team/workspace/read` load a whole subtree, or a chosen set of paths, in one request instead of one request per file — useful for a large folder, or for refreshing a known list of files. Both share the same budgets: content is dropped (`skipped`/`error`: `"budget"`) once it would push the response past 8 MiB serialized, and any file over 1 MiB never gets content regardless of budget (`"too_large"`); the entry keeps its metadata either way. `GET /api/team/workspace/tree` also stops at 20,000 entries and sets `listing_truncated`.

## Blocked Routes

These answer `403` with `{ "error": "<reason>" }`, whatever the method: shutting Residuum down, stopping every agent, updating it, and finishing setup stay with the Residuum app.

- `/api/hub/shutdown` and `/api/hub/stop-all`.
- `/api/hub/update/check`, `/api/hub/update/apply`, and `/api/hub/update/restart`.
- `/api/hub/config/complete-setup`.

Everything else the web UI can call, an artifact can call, including starting or stopping one agent and writing config. Two kinds of path are answered by `residuum.fetch` itself with `400`, without being sent: one that belongs to an agent but names none (`POST /api/sessions` included: start sessions with `residuum.sessions.start({ agent, prompt })`), and one outside `/api/`.

## Requests

At most 8 requests and 4 model calls (`residuum.ask`, or `/api/agents/<agent>/model/complete`) run at once per page; the rest wait their turn in order, so firing many at once is safe. Through Residuum Cloud, a request the relay refuses as `agent overloaded` (`503`) is retried up to 3 times for you, and a response over 10 MB or a call over 25 seconds fails. A request that never reaches Residuum rejects with an `Error`; every other answer is a normal `Response`, so check `ok` or `status`.

## Live Events

Two places hear live events.

`residuum.on(type, handler)` hears Residuum's own events, which need no agent running:

| `type` | Fields | Fires when |
|--------|--------|------------|
| `artifact_updated` / `artifact_removed` | `name` | A workbench artifact's page or folder is written or deleted. The page reloads itself when its own artifact changes, unless it registered an `artifact_updated` handler, which is then called instead. |
| `connection` | `state`: `"connected"` \| `"disconnected"` | The page's live connection to Residuum opens, drops, or comes back. A handler registered later hears the current state first. |

`"*"` hears all three. Any other `type` throws a `TypeError`: an agent's events come from that agent's handle.

`residuum.agent(name).on(type, handler)` hears one agent's events over its own connection, opened the first time you use the handle and reopened if it drops; `connection` follows that connection. The `type` values most useful to artifacts:

| `type` | Fields | Fires when |
|--------|--------|------------|
| `turn_started` / `turn_ended` | `reply_to` | The agent starts or finishes a turn in its main chat. |
| `response` | `reply_to`, `content` | The agent replies in the main chat. |
| `broadcast_response` | `content` | The agent emits text alongside tool calls. |
| `tool_call` / `tool_result` | `name`, `arguments` / `output`, `is_error` | The agent calls a tool, and gets its result. |
| `notice` | `message` | A system notice appears. |
| `session_started`, `session_state_changed`, `session_completed` | `session` or `address`, `run_id`, … | One of the agent's background sessions starts, changes state, or finishes. For a session this artifact started, use its handle's `on` instead (see Agent Sessions). |

`"*"` hears every frame. An agent that doesn't exist or isn't running refuses the connection: the handle hears `connection` `"disconnected"` and keeps trying, so it connects once the agent runs.

## Change Feed

`residuum.watch(prefix, handler)` follows team files and needs no agent running: `prefix` is `"team"` or a path under it (`"team/wiki"`, `"team/workbench/<name>.state.json"`), and change paths carry `team/` too. `residuum.agent(name).watch(prefix, handler)` follows that agent's workspace: its own files are unprefixed (`"memory"`), the team's carry `team/`, and `""` is all of it. Both return a function that stops watching. A prefix that is absolute or contains `..` throws a `TypeError`, and so does a `residuum.watch` prefix outside `team/`.

- Prefixes match whole path segments: `"team/wiki"` covers `team/wiki` and everything under `team/wiki/`, never `team/wikipedia/`. A prefix naming a file covers only that file, and a prefix that doesn't exist yet starts matching once it appears. A change to a folder that contains the prefix (renaming `projects` when watching `projects/alpha`) is delivered too.
- The handler receives `{ type: "workspace_changed", changes: [{ path, kind }] }` with only the changes under its own prefix, sorted by path. `kind` is `created`, `modified`, or `removed`. Treat `created` and `modified` alike: re-read the path. A rename is `removed` for the old path and `created` for the new one. A folder's `created` or `removed` stands for everything inside it.
- Changes arrive in batches: a batch closes once the workspace is quiet for 300 ms, or 2 s after its first change while writes continue.
- `{ type: "workspace_resync", reason }` means changes were missed and the artifact should load what it shows again. `reason` is `"overflow"` (too many changes at once: more than 500 under the page's prefixes in one batch, or the system dropped notifications), `"watcher_restarted"`, or `"reconnected"` (the page's connection dropped and came back). Every handler receives it.
- `{ type: "workspace_watch_unavailable", message }` means Residuum can't watch the workspace at all, so no change frames will come: show the message, and offer a refresh button.
- Paths under `.index`, database files and their sidecars, and in-flight atomic-write temporaries never appear. Reading a file never produces a change.

Load with `GET /api/team/workspace/tree`, start watching before that first load, and refresh changed files with one `POST /api/team/workspace/read` per batch (see the skill's "Keep workspace data current" step).

## Agent Sessions

`await residuum.sessions.start({ agent, prompt, context, skill, model })` starts a session: a full fork of the agent named by `agent`, with its tools and memory, working on `prompt`. A session runs on one agent, so `agent` is required: a call without it rejects with a `TypeError` before anything is sent. `context` is extra text it reads first, `skill` a skill to run as, `model` one of `"small"`, `"medium"` (default), `"large"`. The session knows which artifact started it. Its output comes back to the page only: it never posts in the main chat, never files an inbox item on its own, and can't message the main agent. The page hears it from its first frame, whichever agent runs it. It shows in that agent's Activity in the web UI, as From a workbench page, where the user can watch or stop it. It keeps running if the page closes or reloads; a reloaded page finds its sessions with `GET /api/agents/<agent>/sessions?artifact=<name>`.

It resolves to a handle:

| Member | Does |
|--------|------|
| `agent` | The agent the session runs on. |
| `address` | The session's address. |
| `on(type, handler)` | This session's frames only (`"*"` for all of them). Returns an unsubscribe function. Frames that arrived before `start` resolved (such as `session_started`) are delivered when you register. |
| `await send(text)` | Messages the session; it sees the message as coming from this artifact and answers with a `session_response`. Resolves to `"live"`, `"queued"`, or `"resumed"` (a finished session starts a new run at the same address). |
| `await stop()` | Stops the session. |

The session's frames, all carrying `address` and `run_id`:

| `type` | Fields | Fires when |
|--------|--------|------------|
| `session_started` | `session` (with `run_id`, `state`, `purpose`, …) | The run starts. |
| `session_state_changed` | `state`: `running`, `idle`, `completing` | It starts or finishes a turn, or starts wrapping up. `idle` means it's waiting for a message. |
| `session_broadcast_response` | `content` | It emits text alongside tool calls. |
| `session_tool_call` / `session_tool_result` | `name`, `arguments` / `output`, `is_error` | It calls a tool, and gets its result. |
| `session_response` | `turn_id`, `content` | A turn's final answer. |
| `session_error` | `message` | A turn failed. |
| `session_completed` | `status`: `completed`, `cancelled`, `failed`; `error` | The run is over (after its idle timeout, default 10 minutes, or a stop). |
| `resync` | `session` (as the sessions route lists it now, or `null`), `error` when that couldn't be read | Frames were lost (the connection fell behind or dropped and came back). Show the session from `session` rather than from the frames you have. |

`start`, `send`, and `stop` reject with an `Error` whose `message` is plain language; `send` and `stop` failures also carry `code`: `invalid_request`, `unknown_address`, `not_live` (nothing to stop), `busy` (try again shortly), `delivery_failed`. A blank prompt or message rejects with a `TypeError` before anything is sent. `start` also rejects for an unknown `skill` or `model`, for an agent that doesn't exist (`404`, "no agent named …") or isn't running (`409`, with the agent's state in the error's `state`), and with `code: "no_live_connection"` when the page's live connection to Residuum isn't available within 10 seconds, in which case no session is started.

## Model Calls

`POST /api/agents/<agent>/model/complete` (`residuum.ask` wraps it) sends one request to the named agent's background `small` model and gets one answer back. The agent goes in the request as `{ agent, prompt }` or as `residuum.ask(prompt, { agent })`; a call that names none rejects with a `TypeError`, and the `agent` field is not sent on to the model. No tools, no memory, no identity files — the model sees only what the request contains.

Request, either shorthand or the full form:

```json
{ "prompt": "..." }
```

```json
{
  "system": "...",
  "messages": [{ "role": "user", "content": "...", "images": [{ "media_type": "image/png", "data": "<base64>" }] }],
  "schema": { "...": "JSON Schema" },
  "max_tokens": 1024,
  "temperature": 0.2
}
```

`schema`, when given, asks for structured output; the response's `json` is the parsed result. `max_tokens` and `temperature` override the small tier's defaults when given.

Response:

```json
{ "content": "...", "json": { "...": "only when schema was given" }, "model": "provider/model", "usage": { "input_tokens": 0, "output_tokens": 0 } }
```

`residuum.ask` rejects with an `Error` on any non-2xx: `400` for a malformed request (no `prompt`/`messages`, a message role other than `user`/`assistant`, empty content), `502` when the provider fails or returns unparsable structured output, `504` on a provider timeout. Catch the rejection and show its `message` in the page.
