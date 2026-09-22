# Workbench API Reference

What a workbench tool can reach through `residuum.fetch` and `residuum.on`. Read endpoints return JSON unless noted. Every `path` is relative to the web UI: `/api/...`.

## Endpoints Worth Calling

| Method and path | Returns / does |
|-----------------|----------------|
| `GET /api/workspace/files?path=<dir>` | Directory listing: `[{ name, entry_type: "file" \| "directory", size }]`. Omit `path` for the workspace root. |
| `GET /api/workspace/file?path=<file>` | The file's text (not JSON). 404 if missing, 413 over 1 MiB. |
| `PUT /api/workspace/file` | Body `{ path, content }` writes a text file. The parent directory must already exist. |
| `GET /api/inbox` | The user's inbox: `[{ id, title, body, source, timestamp, read, attachments }]`. |
| `PUT /api/inbox/<id>/read` | Marks an inbox item read. |
| `POST /api/inbox/<id>/archive` | Archives an inbox item. |
| `GET /api/sessions` | Agent sessions: `{ live, completed, next_cursor }`. Filters: `?category=scheduled\|external\|spawned`, `?address=<address>`, `?before=<next_cursor>`, `?limit=1..200` (default 50). |
| `GET /api/sessions/runs/<run_id>/transcript` | One session run's transcript. |
| `GET /api/chat/history` | Recent main-chat messages. |
| `GET /api/workbench/tools` | Every tool: `[{ name, title, modified_at, size }]`. |
| `GET /api/status` | `{ mode }`: `"running"` normally. |
| `GET /api/system/timezone` | The user's configured timezone. |

Paths under `workspace/` are relative to the workspace root, so a tool's data file is `workbench/<name>.state.json`.

## Blocked Routes

These answer `403` with `{ "error": "<reason>" }` and never reach Residuum:

- Writing secrets or agent keys (`/api/secrets`, `/api/agent-keys`; reading their names is allowed).
- Anything under `/api/config/raw`, `/api/providers/raw`, `/api/mcp/raw`, and `/api/config/complete-setup`.
- `/api/shutdown`, `/api/update/check|apply|restart`, `/api/cloud/disconnect`.
- Writes under `/api/tracing/`.
- Deleting workbench tools.

Paths outside `/api/` (including `/ws` and webhooks) are refused with `400`.

## Live Events

`residuum.on(type, handler)` receives the same frames the web UI does. The `type` values most useful to tools:

| `type` | Fields | Fires when |
|--------|--------|------------|
| `turn_started` / `turn_ended` | `reply_to` | You start or finish a turn. |
| `response` | `reply_to`, `content` | You reply in the main chat. |
| `broadcast_response` | `content` | You emit text alongside tool calls. |
| `notice` | `message` | A system notice appears. |
| `session_started`, `session_state_changed`, `session_completed` | `session` or `address`, `run_id`, … | A background session starts, changes state, or finishes. |
| `workbench_tool_updated` / `workbench_tool_removed` | `name` | A workbench tool page is written or deleted. |

`tool_call` and `tool_result` arrive only while the user has verbose mode on.

## Sending Messages

`residuum.send(text)` rejects when:

- it isn't called during a click or key press in the tool;
- `text` is empty or over 20,000 characters;
- Residuum is disconnected.

Catch the rejection and show its `message` in the page.
