# Workbench API Reference

What a workbench artifact can reach through `residuum.fetch` and `residuum.on`. Read endpoints return JSON unless noted. Every `path` is relative to the web UI: `/api/...`.

## Context

Three values are embedded into the page when it loads, not fetched: `residuum.artifact` is the artifact's own name, `residuum.version` is Residuum's version, and `residuum.features` is a frozen array of feature ids this build supports. Check a feature id before relying on the capability it names, since an older Residuum build won't have it.

## Endpoints Worth Calling

| Method and path | Returns / does |
|-----------------|----------------|
| `GET /api/workspace/files?path=<dir>` | Directory listing: `[{ name, entry_type: "file" \| "directory", size, modified, version }]`. `modified` is Unix milliseconds; `version` is an opaque token for conditional writes. Omit `path` for the workspace root. Paths under `.index` or a `.db`/`.sqlite` file (and its `-wal`/`-shm`/`-journal` sidecars) are never listed. |
| `GET /api/workspace/file?path=<file>` | The file's text (not JSON), with an `ETag` header carrying its version. 404 if missing, 413 over 8 MiB, 415 if the file isn't valid UTF-8 (read it from `/api/workspace/raw` instead). |
| `PUT /api/workspace/file` | Body `{ path, content }` writes a text file, up to 8 MiB. Creates missing parent directories. `If-Match: <version>` answers `412` on a stale write; `If-None-Match: *` answers `412` if the file already exists. Returns `{ saved: true, version }`. |
| `GET /api/workspace/tree?path=<dir>&content=<bool>&glob=<pattern>&depth=<n>` | Recursively lists `path` (the workspace root when omitted) as a flat, path-sorted `{ path, entries, listing_truncated, content_truncated }`. Each entry carries `path`, `type: "file" \| "directory"`, `size` (files only), `modified`, `version`, and — with `content=true` — either `content` (UTF-8 files up to 1 MiB, response budget permitting) or `skipped: "binary" \| "too_large" \| "budget"`; a skipped entry still has its metadata. `glob` may repeat: a pattern without `/` matches a file name at any depth, one with `/` matches the path relative to `path`; with any `glob`, only matching files are returned and directories are omitted. `depth` limits recursion (`1` = direct children only; unlimited when omitted). Symlinks below `path` and blocked paths never appear. `403`/`404` for a blocked or missing `path`, `400` if it isn't a directory or a `glob` pattern doesn't parse. Needs the `workspace-tree` feature. |
| `POST /api/workspace/read` | Body `{ paths: [...] }` (at most 1,000, else `400`) reads exactly those files, in order: `{ files: [{ path, size, modified, version, content } \| { path, size?, modified?, version?, error }], content_truncated }`. `error` is `not_found`, `blocked`, `is_directory`, `binary`, `too_large`, or `budget`; a path that escapes the workspace counts as `blocked`. One bad path never fails the whole request. Needs the `workspace-read-batch` feature. |
| `GET /api/inbox` | The user's inbox: `[{ id, title, body, source, timestamp, read, attachments }]`. |
| `PUT /api/inbox/<id>/read` | Marks an inbox item read. |
| `POST /api/inbox/<id>/archive` | Archives an inbox item. |
| `GET /api/sessions` | Agent sessions: `{ live, completed, next_cursor }`. Filters: `?category=scheduled\|external\|spawned`, `?address=<address>`, `?before=<next_cursor>`, `?limit=1..200` (default 50). |
| `GET /api/sessions/runs/<run_id>/transcript` | One session run's transcript. |
| `GET /api/chat/history` | Recent main-chat messages. |
| `GET /api/workbench/artifacts` | Every artifact: `[{ name, title, modified_at, size }]`. |
| `GET /api/status` | `{ mode, version, features }`: `mode` is `"running"` normally, `version` and `features` match `residuum.version` and `residuum.features`. |
| `GET /api/system/timezone` | The user's configured timezone. |

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
| `session_started`, `session_state_changed`, `session_completed` | `session` or `address`, `run_id`, … | A background session starts, changes state, or finishes. |
| `artifact_updated` / `artifact_removed` | `name` | A workbench artifact page is written or deleted. |

`tool_call` and `tool_result` arrive only while the user has verbose mode on.

## Sending Messages

`residuum.send(text)` rejects when:

- it isn't called during a click or key press in the artifact;
- `text` is empty or over 20,000 characters;
- Residuum is disconnected.

Catch the rejection and show its `message` in the page.
