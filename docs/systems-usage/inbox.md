# Inbox

The inbox is a capture system for items the agent or background tasks want to save for later triage. There are **two separate inboxes**, each stored as individual JSON files under the workspace root — they have different paths, different tools, and different consumers.

| | Agent inbox | User inbox |
|---|---|---|
| Path | `inbox/agent/` | `inbox/user/` |
| Archive path | `archive/inbox/agent/` | `archive/inbox/user/` |
| Populated by | Notification router (`inbox` channel target) for `scheduled` and webhook-triggered `external` results, the WS `/inbox` command, `POST /api/agents/{name}/agent-inbox` | `user_inbox_add` tool |
| Read/manage tools | `inbox_list`, `inbox_read`, `inbox_archive`, `inbox_restore` | *(none for the agent)* |
| Consumed by | The agent, via the tools above | The user, via the web UI/HTTP API |
| Attachments | Populated only when a chat attachment is saved to the agent inbox as a companion item; not attachable via `inbox_list`/`inbox_read`/`inbox_archive`/`inbox_restore` | Populated by `user_inbox_add`'s optional `attachments` parameter, served at `GET /api/agents/{name}/inbox/{id}/attachments/{index}` |

The agent inbox is a queue for the agent itself to triage — it's where the `inbox` notification-routing target delivers results. The user inbox is a one-way delivery channel *to* the user: the agent (often a background sub-agent, e.g. the built-in `introspection` skill) writes to it with `user_inbox_add`, and the user reads and archives items through the web UI. The agent has no tool to list, read, or archive the user inbox — only to add to it.

Archiving is a soft delete in both inboxes, not a permanent one: an archived item's JSON file (and its attachments directory, if it has one) simply moves under `archive/inbox/`, so restoring it is just moving it back. The agent restores its own inbox items with `inbox_restore`; the user restores inbox items through the web UI's archived view, which calls `POST /api/agents/{name}/inbox/{id}/restore`.

The user inbox's HTTP routes (`GET /api/agents/{name}/inbox` and `.../inbox/archive`, `PUT .../inbox/{id}/read`, `POST .../inbox/{id}/archive` and `.../restore`, and `GET .../inbox/{id}/attachments/{index}`) only read and write the inbox files, so they answer for a stopped or failed agent as well as a running one (see [Hub HTTP Surface](hub-http.md#agent-routes)). `POST /api/agents/{name}/agent-inbox`, which adds to the agent inbox, needs the agent running.

The hub also serves every agent's user inbox as one list (see [Cross-Agent View](#cross-agent-view)).

## How Items Arrive

- **Agent inbox**: the notification router files every substantive `scheduled` result and every substantive webhook-triggered `external` result here. A conversation-triggered `external` result (A2A, or a non-owner Discord/Telegram/Teams chat) never reaches the inbox — its output already went back to the conversation it came from, and its observations are merged into memory as an episode — nor does an `artifact` or `spawned` session's result, both of which are relayed elsewhere (see [background-tasks.md](background-tasks.md#result-routing)). A workbench artifact can also add an item directly with `POST /api/agents/{name}/agent-inbox` (body `{ title?, body }`) — the same place the WS `/inbox` command writes to. `title` defaults to the body's first line, in full; a blank `body` is refused with `400`. The item's `source` is `artifact:<name>` when the request carries the `X-Residuum-Artifact` header the workbench SDK sends, `web` otherwise. There is no equivalent HTTP endpoint for the user inbox.
- **User inbox**: the agent calls `user_inbox_add`; nothing else writes here. After saving an item the tool publishes its ID on the agent's bus, which the hub's watcher passes on (see [Watching running agents](hub.md#watching-running-agents)).

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

- Filenames are generated from the date and the sanitized title (`{YYYYMMDD}_{title}`, at most 60 title characters). The filename stem *is* the ID used by `inbox_read`/`inbox_archive`, and the directory attachments are copied into.
- **IDs are unique within an agent.** When the generated stem is already taken, by an item in the inbox or in its archive, `_2`, `_3`, ... is appended (`20260227_weekly_export_ready_2`), so two items with the same title on the same day each keep their own file and attachment directory, and an archived item's ID is never reused. Every writer goes through this: `user_inbox_add`, the notification router, the WS `/inbox` command, `POST /api/agents/{name}/agent-inbox`, a chat attachment's companion item, and the hub's own notes. Any existing file keeps working under its own stem, suffixed or not.
- Archiving and restoring never replace a different item that has the same ID. If a file in the destination already has it (an item placed by hand, say), the move fails with an error naming the conflict, and both items stay where they are. The hub API answers that case with `409`.
- `timestamp` is a naive local time to the minute (`YYYY-MM-DDTHH:MM`) in the hub's timezone. The hub API reports it as RFC 3339 with an offset (see [Cross-Agent View](#cross-agent-view)).
- Only the final path component (the filename) of each `attachments` entry is meaningful — the directory portion can go stale once an item is archived, since archiving physically moves the item's attachment directory alongside its JSON file. Consumers (the web UI, the HTTP serving endpoint) resolve attachments by filename against the item's *current* location, not by trusting the stored path literally.
- There is no unread-count surfaced anywhere in the agent's context or status line — the agent has to call `inbox_list` (with `unread_only: true`) to find out.

## Tools (Agent Inbox Only)

| Tool | Parameters | Notes |
|------|-----------|-------|
| `inbox_list` | `unread_only` (bool, optional, default false), `archived` (bool, optional, default false) | Lists agent inbox items. `archived: true` lists `archive/inbox/agent/` instead and ignores `unread_only` (everything there is already read). |
| `inbox_read` | `id` (string — filename stem) | Reads item content, marks as read as a side effect. Cannot be unmarked. |
| `inbox_archive` | `ids` (string[] — filename stems) | Moves items from `inbox/agent/` to `archive/inbox/agent/`. This is a move, not a copy. |
| `inbox_restore` | `ids` (string[] — filename stems) | Moves items from `archive/inbox/agent/` back to `inbox/agent/`. The only way to undo `inbox_archive`. |

## User Inbox Attachments

`user_inbox_add` accepts an optional `attachments` parameter: an array of paths to files the agent has already written to disk (an export, a report, a screenshot). Each file is copied — not moved or linked — into the item's own directory, so the item keeps working even if the original file is later moved or deleted.

- **Copy, not reference**: files land at `inbox/user/attachments/{item id}/{filename}`. Source filenames are reduced to their final path component before use, so a traversal-style source path can't place a copy outside the item's directory, and same-name collisions within one call get a `_2`, `_3`, ... suffix rather than clobbering.
- **No size cap**: these are already-local files, not something arriving over a platform with its own upload limit.
- **Partial failure is not fatal**: a file that fails to copy (missing, unreadable) is skipped and logged; the item is still created with whichever attachments did succeed, and the tool result names each one that failed.
- **Archiving moves attachments too**: when the user archives an item, its `inbox/user/attachments/{item id}/` directory moves to `archive/inbox/user/attachments/{item id}/` alongside the JSON file, so the item's attachments keep serving after archiving; restoring the item reverses that move.
- **Serving**: the web UI fetches attachments from `GET /api/agents/{name}/inbox/{id}/attachments/{index}`, which checks the active inbox first, then the archive, and confines every resolved path to the item's own attachment directory before serving — an out-of-tree path 404s rather than confirming it exists.
- **Restoring**: `GET /api/agents/{name}/inbox/archive` lists archived user inbox items the same shape as `GET /api/agents/{name}/inbox`; `POST /api/agents/{name}/inbox/{id}/restore` moves one back to the active inbox. The web UI's inbox has an archived view with a Restore action wired to this endpoint.

## Cross-Agent View

`GET /api/hub/inbox` and its per-item routes serve the user inboxes of every agent as one list, read straight from each agent's `inbox/user/` and `archive/inbox/user/`, so an agent's items are included whether it is running, stopped, or failed. The routes and shapes are in [Hub HTTP Surface](hub-http.md#cross-agent-inbox).

- Each item is identified by its agent and its ID, since two agents can have items with the same ID.
- Each agent's unread count is part of its [team overview](hub.md#team-overview). The overview counts again when the hub reads, archives or restores an item through these routes, and when the `user_inbox_add` tool or a change to the inbox's files touches a running agent's inbox.
- The list is newest first, by the item's time and then its ID, in pages.
- An item's time is the stored naive local time read in the hub's configured timezone at the moment of the request, so changing the timezone changes the instants reported. A local time that happened twice (a DST fall-back) takes its first occurrence, and one that never happened (a spring-forward gap) moves forward by the length of the gap.
- Marking an item read, archiving it, and restoring it make the same file changes as the per-agent routes.
- Attachment links point at the per-agent attachment route, which serves a stopped agent as well.

## Intended Usage

The agent inbox is for **low-urgency items** that don't need immediate attention — background task results that are informational but not actionable should route here rather than to a push notification channel. The agent should periodically triage it — reading items, acting on anything that needs follow-up, and archiving items that are resolved. This should be driven by a heartbeat pulse.

The user inbox is for findings the agent wants to hand to the user asynchronously, without interrupting a conversation — e.g. the built-in `reflection` pulse delivers its suggestions there. `memory_tending` files knowledge into the team wiki and `team/USER.md` directly and does not deliver to the user inbox. Attach a file with `user_inbox_add`'s `attachments` parameter when the finding is easier to review as a file than as inline text (an export, a screenshot, a generated report). Before adding a new item, check prior items (including the archive) so the same suggestion isn't repeated.
