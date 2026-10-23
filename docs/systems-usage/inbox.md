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

The agent inbox is a queue for the agent itself to triage — it's where the `inbox` notification-routing target delivers results. The user inbox is a one-way delivery channel *to* the user: the agent (often a background sub-agent, e.g. the built-in `introspection` skill) writes to it with `user_inbox_add`, and the user reads and archives them in the web UI's Inbox (see [Web Inbox](#web-inbox)), where a reply the user writes lands in the agent inbox. The agent has no tool to list, read, or archive the user inbox — only to add to it.

Archiving is a soft delete in both inboxes, not a permanent one: an archived item's JSON file (and its attachments directory, if it has one) simply moves under `archive/inbox/`, so restoring it is just moving it back. The agent restores its own inbox items with `inbox_restore`; the user restores a conversation from the web UI Inbox's Archived folder (see [Web Inbox](#web-inbox)); `POST /api/agents/{name}/inbox/{id}/restore` makes the same move for one agent's item.

The user inbox's HTTP routes (`GET /api/agents/{name}/inbox` and `.../inbox/archive`, `PUT .../inbox/{id}/read`, `POST .../inbox/{id}/archive` and `.../restore`, and `GET .../inbox/{id}/attachments/{index}`) only read and write the inbox files, so they answer for a stopped or failed agent as well as a running one (see [Hub HTTP Surface](hub-http.md#agent-routes)). `POST /api/agents/{name}/agent-inbox`, which adds to the agent inbox, needs the agent running.

The hub also serves the mail the web UI's Inbox shows, across every agent (see [Web Inbox](#web-inbox)).

## How Items Arrive

- **Agent inbox**: the notification router files every substantive `scheduled` result and every substantive webhook-triggered `external` result here. A conversation-triggered `external` result (A2A, or a non-owner Discord/Telegram/Teams chat) never reaches the inbox — its output already went back to the conversation it came from, and its observations are merged into memory as an episode — nor does an `artifact` or `spawned` session's result, both of which are relayed elsewhere (see [background-tasks.md](background-tasks.md#result-routing)). A workbench artifact can also add an item directly with `POST /api/agents/{name}/agent-inbox` (body `{ title?, body }`) — the same place the WS `/inbox` command writes to. `title` defaults to the body's first line, in full; a blank `body` is refused with `400`. The item's `source` is `artifact:<name>` when the request carries the `X-Residuum-Artifact` header the workbench SDK sends, `web` otherwise. There is no equivalent HTTP endpoint for the user inbox.
- **User inbox**: the agent calls `user_inbox_add`; nothing else writes here, the hub included. An agent's failure and an agent being created, deleted or restored reach the user through the hub's events, its team event log and push instead (see [Agent states](hub.md#agent-states) and [Agent creation, deletion and restore](agent-lifecycle.md)). After saving an item the tool publishes its ID on the agent's bus, which the hub's watcher passes on (see [Watching running agents](hub.md#watching-running-agents)).

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
- **IDs are unique within an agent.** When the generated stem is already taken, by an item in the inbox or in its archive, `_2`, `_3`, ... is appended (`20260227_weekly_export_ready_2`), so two items with the same title on the same day each keep their own file and attachment directory, and an archived item's ID is never reused. Every writer goes through this: `user_inbox_add`, the notification router, the WS `/inbox` command, `POST /api/agents/{name}/agent-inbox`, and a chat attachment's companion item. Any existing file keeps working under its own stem, suffixed or not.
- Archiving and restoring never replace a different item that has the same ID. If a file in the destination already has it (an item placed by hand, say), the move fails with an error naming the conflict, and both items stay where they are. The hub API answers that case with `409`.
- `timestamp` is a naive local time to the minute (`YYYY-MM-DDTHH:MM`) in the hub's timezone. The hub API reports it as RFC 3339 with an offset (see [Hub HTTP Surface](hub-http.md)).
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
- **Serving**: the web UI downloads an attachment from `GET /api/agents/{name}/inbox/{id}/attachments/{index}`, which checks the active inbox first, then the archive, and confines every resolved path to the item's own attachment directory before serving — an out-of-tree path 404s rather than confirming it exists. A missing attachment answers `404` with `{ "error" }`; a browser opening the link in a tab is sent to the item's conversation in the Inbox (`/inbox?thread=<ref>`) with a notice that the attachment is gone (see [Browser landings](hub-http.md#browser-landings)). A `.md` or `.txt` attachment opens in the Files panel instead of downloading (see [Web Inbox](#web-inbox)).
- **Restoring**: `GET /api/agents/{name}/inbox/archive` lists archived user inbox items the same shape as `GET /api/agents/{name}/inbox`; `POST /api/agents/{name}/inbox/{id}/restore` moves one back to the active inbox. The web UI's Inbox lists and restores archived conversations through the hub's `/api/hub/mail/*` routes instead, which make the same moves.

## Web Inbox

The Inbox place (`/inbox`, **Inbox** at the top of the left-hand rail) is a mail client over every agent's mail. It reads and writes through the hub's `/api/hub/mail/*` routes, which work from the agents' files, so a stopped or failed agent's mail is listed, read, archived and answered like a running agent's.

- **Folders.** **Inbox** holds the conversations with at least one message from an agent that isn't archived, **Sent** the conversations with a message you wrote, and **Archived** the conversations whose agent messages are all archived. A conversation can be in Inbox and Sent at once.
- **Agent filter.** The filter beside the folders is **All agents** or one agent. In the Inbox folder it shows each agent's unread count.
- **List.** Conversations are newest first, in pages (**Show older** pages older ones in). A row names who the conversation is with (in Sent, who it went to: **To atlas, wren**), how many messages it holds, when it last moved, its subject (**(no subject)** when it has none) and the first line of the latest message. An unread conversation has a dot and a bold subject, and a conversation with files has a paperclip.
- **Unread counts.** The counts on the rail, in the bottom bar, in the tab title and on the app badge are each agent's `inbox_unread` from the [team overview](hub.md#team-overview): the unread messages from agents, not conversations. They recount when mail is read, archived, restored or arrives, and the open list and conversation refresh with them.
- **Address.** The folder, the filter, the open conversation and a New message are in the URL: `/inbox?folder=sent&agent=atlas&thread=<ref>` and `/inbox?compose=1&to=atlas,scout`, with `thread` percent-encoded. A link naming any message of a conversation opens the whole conversation. `?item=<agent>:<id>` opens that agent's message and `?tab=archived` the Archived folder. An agent in the URL that doesn't exist is dropped, with a notice.

### Reading and filing

Opening a conversation shows its messages oldest first, each marked as yours or the agent's, with the time, the Markdown body, its attachments and, on a message you wrote, whether each agent it went to has seen it.

- **Read.** Opening a conversation marks it read at once. If the hub refuses, it goes back to unread and the reason is shown.
- **Archive and restore.** Archiving moves the whole conversation to Archived, and restoring moves it back to Inbox. Each shows a toast with **Undo**.
- **Reply.** The box under the messages is reply-all: it goes to every agent in the conversation, listed on its **To:** line, and says that each agent will see it the next time it checks its inbox. Ctrl+Enter or Cmd+Enter sends it. A reply draft stays while you switch folders, and is kept in memory only.
- **Attachments.** A `.md` or `.txt` attachment opens in the Files panel beside the conversation, rendered, with **Edit** to change it and save it in the agent's workspace. Any other attachment downloads. Closing the conversation, opening another or changing folder closes a panel the conversation opened, and leaves a Files panel opened some other way.

### Writing

**New message** (in the place header, and in an agent's chat as **New message to** that agent) opens a message in the pane beside the list. **To** takes one or more agents (the filtered agent is filled in), and the message has a subject, a Markdown body and attachments. The subject may be empty, and the first line of the body is then the subject. The body may not be empty. Sending opens the conversation in Sent; the hub files one copy in each recipient's inbox. Cancelling drops the draft. Leaving a New message that has text or files asks first, unless it is already being sent (the send carries on, and a notice says whether it went), and closing or reloading the page with any unsent draft asks too.

A file attached to a message or reply uploads when it is chosen, shows its progress on its chip, and can be removed first. A failed upload shows **Try again** on its chip, and **Send** waits for uploads still running and won't go while one has failed. Both boxes say under the text what a file may weigh (**Any file, up to 50 MB each.**) and when the recipients will see the message, and a file over that is refused before it uploads, with a message naming the file and the limit. A message waiting on uploads says so under the text. In a New message, Ctrl+Enter or Cmd+Enter sends from the message box or the subject, as in a reply; a plain Enter in the subject moves to the message box and never sends.

### Layout

The conversation list, the conversation and a Files panel share the main region, and the conversation narrows to a minimum width before anything else gives way. When the list and the conversation no longer fit side by side, as when a Files panel opens beside them, the place shows one at a time: the list, or the open conversation or New message with **Back to list**. On a phone it is always one at a time. Focus follows what opens: opening a conversation or New message while the list is hidden, and sending a New message, put focus on the conversation's subject (or **New message**), and **Back to list** or **Cancel** put it back on the conversation's row or the list. **Show older** keeps focus when its last page takes the button away, moving it to the first new row.

## Intended Usage

The agent inbox is for **low-urgency items** that don't need immediate attention — background task results that are informational but not actionable should route here rather than to a push notification channel. The agent should periodically triage it — reading items, acting on anything that needs follow-up, and archiving items that are resolved. This should be driven by a heartbeat pulse.

The user inbox is for findings the agent wants to hand to the user asynchronously, without interrupting a conversation — e.g. the built-in `reflection` pulse delivers its suggestions there. `memory_tending` files knowledge into the team wiki and `team/USER.md` directly and does not deliver to the user inbox. Attach a file with `user_inbox_add`'s `attachments` parameter when the finding is easier to review as a file than as inline text (an export, a screenshot, a generated report). Before adding a new item, check prior items (including the archive) so the same suggestion isn't repeated.
