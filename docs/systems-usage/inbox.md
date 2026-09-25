# Inbox

The inbox is a capture system for items the agent or background tasks want to save for later triage. There are **two separate inboxes**, each stored as individual JSON files under the workspace root — they have different paths, different tools, and different consumers.

| | Agent inbox | User inbox |
|---|---|---|
| Path | `inbox/agent/` | `inbox/user/` |
| Archive path | `archive/inbox/agent/` | `archive/inbox/user/` |
| Populated by | Notification router (`inbox` channel target) for `scheduled` and webhook-triggered `external` results, the WS `/inbox` command, `POST /api/agent-inbox` | `user_inbox_add` tool |
| Read/manage tools | `inbox_list`, `inbox_read`, `inbox_archive` | *(none for the agent)* |
| Consumed by | The agent, via the tools above | The user, via the web UI/HTTP API |
| Attachments | Populated only when a chat attachment is saved to the agent inbox as a companion item; not attachable via `inbox_list`/`inbox_read`/`inbox_archive` | Populated by `user_inbox_add`'s optional `attachments` parameter, served at `GET /api/inbox/{id}/attachments/{index}` |

The agent inbox is a queue for the agent itself to triage — it's where the `inbox` notification-routing target delivers results. The user inbox is a one-way delivery channel *to* the user: the agent (often a background sub-agent, e.g. the built-in `introspection` skill) writes to it with `user_inbox_add`, and the user reads and archives items through the web UI. The agent has no tool to list, read, or archive the user inbox — only to add to it.

## How Items Arrive

- **Agent inbox**: the notification router files every substantive `scheduled` result and every substantive webhook-triggered `external` result here. A conversation-triggered `external` result (A2A, or a non-owner Discord/Telegram/Teams chat) never reaches the inbox — its output already went back to the conversation it came from, and its observations are merged into memory as an episode — nor does an `artifact` or `spawned` session's result, both of which are relayed elsewhere (see [background-tasks.md](background-tasks.md#result-routing)). A workbench artifact can also add an item directly with `POST /api/agent-inbox` (body `{ title?, body }`) — the same place the WS `/inbox` command writes to. `title` defaults to the body's first line cut to 60 characters; a blank `body` is refused with `400`. The item's `source` is `artifact:<name>` when the request carries the workbench bridge's artifact-identity header, `web` otherwise. There is no equivalent HTTP endpoint for the user inbox.
- **User inbox**: the agent calls `user_inbox_add`; nothing else writes here.

## Item Format

Each item is a JSON file. There is **no `id` field in the JSON body** — the ID is the filename stem (e.g. `20260227_deploy-completed.json` has ID `20260227_deploy-completed`):

```json
{
  "title": "Deploy completed",
  "body": "Production deployment finished successfully. 3 services updated.",
  "source": "deploy-watcher",
  "timestamp": "2026-02-27T14:30",
  "read": false,
  "attachments": []
}
```

A user inbox item created with `user_inbox_add`'s `attachments` parameter records each copied file's path under `inbox/user/attachments/{item id}/`, relative to the workspace root:

```json
{
  "title": "Weekly export ready",
  "body": "Attached the CSV export for this week.",
  "source": "agent",
  "timestamp": "2026-02-27T14:30",
  "read": false,
  "attachments": ["inbox/user/attachments/20260227_weekly_export_ready/export.csv"]
}
```

- Filenames are auto-generated from date and sanitized title; the filename stem *is* the ID used by `inbox_read`/`inbox_archive`, and the directory attachments are copied into.
- Only the final path component (the filename) of each `attachments` entry is meaningful — the directory portion can go stale once an item is archived, since archiving physically moves the item's attachment directory alongside its JSON file. Consumers (the web UI, the HTTP serving endpoint) resolve attachments by filename against the item's *current* location, not by trusting the stored path literally.
- There is no unread-count surfaced anywhere in the agent's context or status line — the agent has to call `inbox_list` (with `unread_only: true`) to find out.

## Tools (Agent Inbox Only)

| Tool | Parameters | Notes |
|------|-----------|-------|
| `inbox_list` | `unread_only` (bool, optional, default false) | Lists agent inbox items |
| `inbox_read` | `id` (string — filename stem) | Reads item content, marks as read as a side effect. Cannot be unmarked. |
| `inbox_archive` | `ids` (string[] — filename stems) | Moves items from `inbox/agent/` to `archive/inbox/agent/`. This is a move, not a copy. |

## User Inbox Attachments

`user_inbox_add` accepts an optional `attachments` parameter: an array of paths to files the agent has already written to disk (an export, a report, a screenshot). Each file is copied — not moved or linked — into the item's own directory, so the item keeps working even if the original file is later moved or deleted.

- **Copy, not reference**: files land at `inbox/user/attachments/{item id}/{filename}`. Source filenames are reduced to their final path component before use, so a traversal-style source path can't place a copy outside the item's directory, and same-name collisions within one call get a `_2`, `_3`, ... suffix rather than clobbering.
- **No size cap**: these are already-local files, not something arriving over a platform with its own upload limit.
- **Partial failure is not fatal**: a file that fails to copy (missing, unreadable) is skipped and logged; the item is still created with whichever attachments did succeed, and the tool result names each one that failed.
- **Archiving moves attachments too**: when the user archives an item, its `inbox/user/attachments/{item id}/` directory moves to `archive/inbox/user/attachments/{item id}/` alongside the JSON file, so the item's attachments keep serving after archiving.
- **Serving**: the web UI fetches attachments from `GET /api/inbox/{id}/attachments/{index}`, which checks the active inbox first, then the archive, and confines every resolved path to the item's own attachment directory before serving — an out-of-tree path 404s rather than confirming it exists.

## Intended Usage

The agent inbox is for **low-urgency items** that don't need immediate attention — background task results that are informational but not actionable should route here rather than to a push notification channel. The agent should periodically triage it — reading items, acting on anything that needs follow-up, and archiving items that are resolved. This should be driven by a heartbeat pulse.

The user inbox is for findings the agent wants to hand to the user asynchronously, without interrupting a conversation — e.g. the built-in `reflection` pulse delivers its suggestions there. `memory_tending` files knowledge into the wiki and `USER.md` directly and does not deliver to the user inbox. Attach a file with `user_inbox_add`'s `attachments` parameter when the finding is easier to review as a file than as inline text (an export, a screenshot, a generated report). Before adding a new item, check prior items (including the archive) so the same suggestion isn't repeated.
