# Workbench API Reference

What a workbench artifact can reach through `residuum.fetch`, `residuum.ask`, `residuum.on`, and `residuum.sessions`. Read endpoints return JSON unless noted. Every `path` is relative to the web UI: `/api/...`.

## Context

Three values are embedded into the page when it loads, not fetched: `residuum.artifact` is the artifact's own name, `residuum.version` is Residuum's version, and `residuum.features` is a frozen array of feature ids this build supports. Check a feature id before relying on the capability it names, since an older Residuum build won't have it.

## Artifact State

`residuum.state.get()` and `residuum.state.set(value)` are sugar over the workspace file API for one file, `workbench/<name>.state.json` — no separate endpoint. `get()` resolves to the parsed value, `null` when the file doesn't exist yet, and rejects with an `Error` if the saved content isn't valid JSON. `set(value)` writes `JSON.stringify(value)` unconditionally (last write wins); use `residuum.fetch` against `/api/workspace/file` directly with `If-Match` for conflict detection.

## Endpoints Worth Calling

| Method and path | Returns / does |
|-----------------|----------------|
| `GET /api/workspace/files?path=<dir>` | Directory listing: `[{ name, entry_type: "file" \| "directory", size, modified, version }]`. `modified` is Unix milliseconds; `version` is an opaque token for conditional writes. Omit `path` for the workspace root. Paths under `.index` or a `.db`/`.sqlite` file (and its `-wal`/`-shm`/`-journal` sidecars) are never listed. |
| `GET /api/workspace/file?path=<file>` | The file's text (not JSON), with an `ETag` header carrying its version. 404 if missing, 413 over 8 MiB, 415 if the file isn't valid UTF-8 (read it from `/api/workspace/raw` instead). |
| `PUT /api/workspace/file` | Body `{ path, content }` writes a text file, up to 8 MiB. Creates missing parent directories. `If-Match: <version>` answers `412` on a stale write; `If-None-Match: *` answers `412` if the file already exists. Returns `{ saved: true, version }`. |
| `GET /api/workspace/tree?path=<dir>&content=<bool>&glob=<pattern>&depth=<n>` | Recursively lists `path` (the workspace root when omitted) as a flat, path-sorted `{ path, entries, listing_truncated, content_truncated }`. Each entry carries `path`, `type: "file" \| "directory"`, `size` (files only), `modified`, `version`, and — with `content=true` — either `content` (UTF-8 files up to 1 MiB, response budget permitting) or `skipped: "binary" \| "too_large" \| "budget"`; a skipped entry still has its metadata. `glob` may repeat: a pattern without `/` matches a file name at any depth, one with `/` matches the path relative to `path`; with any `glob`, only matching files are returned and directories are omitted. `depth` limits recursion (`1` = direct children only; unlimited when omitted). Symlinks below `path` and blocked paths never appear. `403`/`404` for a blocked or missing `path`, `400` if it isn't a directory or a `glob` pattern doesn't parse. Needs the `workspace-tree` feature. |
| `POST /api/workspace/read` | Body `{ paths: [...] }` (at most 1,000, else `400`) reads exactly those files, in order: `{ files: [{ path, size, modified, version, content } \| { path, size?, modified?, version?, error }], content_truncated }`. `error` is `not_found`, `blocked`, `is_directory`, `binary`, `too_large`, or `budget`; a path that escapes the workspace counts as `blocked`. One bad path never fails the whole request. Needs the `workspace-read-batch` feature. |
| `GET /api/workspace/raw?path=<file>` | The file's raw bytes (use for anything binary — images, audio, non-UTF-8 data), with a `Content-Type` guessed from its extension and an `ETag` header carrying its version. 404 if missing, 413 over 8 MiB. |
| `PUT /api/workspace/raw?path=<file>` | Body is the file's exact bytes, up to 8 MiB — send an `ArrayBuffer`, a typed array, or a `Blob` as `residuum.fetch`'s `body` and it goes over unchanged, not JSON-encoded. Same write rules as `PUT /api/workspace/file` (conditional headers, parent creation, atomic write). Returns `{ saved: true, version }`. |
| `DELETE /api/workspace/file?path=<p>&recursive=<bool>` | Deletes a file, or a directory when `recursive=true` (a directory without it answers `409`). The workspace root can't be deleted (`400`), nor can a directory holding Residuum's memory index or database files (`403`). `If-Match` applies to files. Returns `{ deleted: true }`. |
| `POST /api/workspace/dir` | Body `{ path }` creates a directory and its missing parents. Idempotent — creating one that already exists succeeds. Returns `{ created: true }`. |
| `POST /api/workspace/move` | Body `{ from, to, overwrite? }` moves or renames a file or directory, creating `to`'s missing parents. `409` if `to` already exists and `overwrite` isn't `true`. A directory holding Residuum's memory index or database files can't be moved or overwritten (`403`). `If-Match` applies to `from` when it's a file. Returns `{ moved: true, version }` (`version` is `null` for a moved directory). |
| `GET /api/inbox` | The user's inbox: `[{ id, title, body, source, timestamp, read, attachments }]`. |
| `PUT /api/inbox/<id>/read` | Marks an inbox item read. |
| `POST /api/inbox/<id>/archive` | Archives an inbox item. |
| `POST /api/agent-inbox` | Body `{ title?, body }` adds an item to the agent's own inbox (what the `/inbox` command and `inbox_list`/`inbox_read` work from) — there's no equivalent for the user's inbox. `title` defaults to the body's first line, cut to 60 characters. Blank `body` answers `400`. Returns `{ id }`. |
| `GET /api/memory/search?q=<query>&limit=<1..50, default 10>&source=observations\|episodes\|wiki&date_from=&date_to=` | The same hybrid search the `memory_search` tool runs. Returns `{ results: [{ id, source, episode_id, date, line_start, line_end, snippet, score }], semantic }`, where `semantic` says whether vector search contributed. Blank `q`, an unrecognized `source`, or a malformed date answers `400`. |
| `GET /api/sessions` | Agent sessions: `{ live, completed, next_cursor }`. Filters: `?category=scheduled\|external\|spawned\|artifact`, `?address=<address>`, `?artifact=<name>` (sessions that artifact started; use `residuum.artifact` for your own), `?before=<next_cursor>`, `?limit=1..200` (default 50). |
| `POST /api/sessions` | Starts an agent session for this artifact; use `residuum.sessions.start` (see Agent Sessions). |
| `POST /api/sessions/<address>/messages` | Body `{ content }` messages a session. `200` with `{ outcome: "live" \| "queued" \| "resumed" }`; failures are `{ error, code }` (see Agent Sessions). |
| `POST /api/sessions/<address>/stop` | Stops a live session: `202`, `404` when it isn't running. |
| `GET /api/sessions/runs/<run_id>/transcript` | One session run's transcript. |
| `GET /api/chat/history` | Recent main-chat messages. |
| `GET /api/workbench/artifacts` | Every artifact: `[{ name, title, modified_at, size }]`. |
| `GET /api/status` | `{ mode, version, features }`: `mode` is `"running"` normally, `version` and `features` match `residuum.version` and `residuum.features`. |
| `GET /api/system/timezone` | The user's configured timezone. |
| `POST /api/model/complete` | One-shot small-model call — see "Model Calls" below. |

Paths under `workspace/` are relative to the workspace root, so an artifact's data file is `workbench/<name>.state.json`.

`GET /api/workspace/tree` and `POST /api/workspace/read` load a whole subtree, or a chosen set of paths, in one request instead of one request per file — useful for a large folder, or for refreshing a known list of files. Both share the same budgets: content is dropped (`skipped`/`error`: `"budget"`) once it would push the response past 8 MiB serialized, and any file over 1 MiB never gets content regardless of budget (`"too_large"`); the entry keeps its metadata either way. `GET /api/workspace/tree` also stops at 20,000 entries and sets `listing_truncated`.

## Blocked Routes

These answer `403` with `{ "error": "<reason>" }` and never reach Residuum:

- Writing secrets or agent keys (`/api/secrets`, `/api/agent-keys`; reading their names is allowed).
- Anything under `/api/config/raw` and `/api/providers/raw`.
- `/api/config/complete-setup`.
- `/api/shutdown`, `/api/update/check|apply|restart`, `/api/cloud/disconnect`.
- Writes under `/api/tracing/`.

Paths outside `/api/` (including `/ws` and webhooks) are refused with `400`.

## Live Events

`residuum.on(type, handler)` receives the same frames the web UI does. The `type` values most useful to artifacts:

| `type` | Fields | Fires when |
|--------|--------|------------|
| `turn_started` / `turn_ended` | `reply_to` | You start or finish a turn. |
| `response` | `reply_to`, `content` | You reply in the main chat. |
| `broadcast_response` | `content` | You emit text alongside tool calls. |
| `notice` | `message` | A system notice appears. |
| `session_started`, `session_state_changed`, `session_completed` | `session` or `address`, `run_id`, … | A background session starts, changes state, or finishes. For a session this artifact started, use its handle's `on` instead (see Agent Sessions). |
| `artifact_updated` / `artifact_removed` | `name` | A workbench artifact page is written or deleted. |

`tool_call` and `tool_result` arrive only while the user has verbose mode on.

## Agent Sessions

`await residuum.sessions.start({ prompt, context, skill, model })` starts a session: a full fork of the agent, with its tools and memory, working on `prompt`. `context` is extra text it reads first, `skill` a skill to run as, `model` one of `"small"`, `"medium"` (default), `"large"`. The session knows which artifact started it. Its output comes back to the page only: it never posts in the main chat, never files an inbox item on its own, and can't message the main agent. It shows in the web UI's sessions sidebar under Artifacts, where the user can watch or stop it, and it keeps running if the page closes.

It resolves to a handle:

| Member | Does |
|--------|------|
| `address` | The session's address. |
| `on(type, handler)` | Like `residuum.on`, but only this session's frames (`"*"` for all of them). Returns an unsubscribe function. Frames that arrived before `start` resolved (such as `session_started`) are delivered when you register. |
| `await send(text)` | Messages the session; it sees the message as coming from this artifact and answers with a `session_response`. Resolves to `"live"`, `"queued"`, or `"resumed"` (a finished session starts a new run at the same address). |
| `await stop()` | Stops the session. |

The session's frames, all carrying `address` and `run_id`:

| `type` | Fields | Fires when |
|--------|--------|------------|
| `session_started` | `session` (with `run_id`, `state`, `purpose`, …) | The run starts. |
| `session_state_changed` | `state`: `running`, `idle`, `completing` | It starts or finishes a turn, or starts wrapping up. `idle` means it's waiting for a message. |
| `session_broadcast_response` | `content` | It emits text alongside tool calls. |
| `session_response` | `turn_id`, `content` | A turn's final answer. |
| `session_error` | `message` | A turn failed. |
| `session_completed` | `status`: `completed`, `cancelled`, `failed`; `error` | The run is over (after its idle timeout, default 10 minutes, or a stop). |
| `session_tool_call` / `session_tool_result` | `name`, `arguments` / `output`, `is_error` | Only while the user has verbose mode on. |

`start`, `send`, and `stop` reject with an `Error` whose `message` is plain language; `send` and `stop` failures also carry `code`: `invalid_request`, `unknown_address`, `not_live` (nothing to stop), `busy` (try again shortly), `delivery_failed`. A blank prompt or message rejects with a `TypeError` before anything is sent. `start` also rejects for an unknown `skill` or `model`, and outside the web UI.

## Model Calls

`POST /api/model/complete` (`residuum.ask` wraps it) sends one request to the background `small` model and gets one answer back. No tools, no memory, no identity files — the model sees only what the request contains.

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
