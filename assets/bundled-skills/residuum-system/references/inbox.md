# Inbox

There are **two** inboxes, stored as individual JSON files under the workspace root. They are separate queues with different tools, consumers, and archive locations — don't conflate them.

| | Agent inbox | User inbox |
|---|---|---|
| Path | `inbox/agent/` | `inbox/user/` |
| Archive path | `archive/inbox/agent/` | `archive/inbox/user/` |
| Attachments path | `inbox/agent/attachments/<item id>/` | `inbox/user/attachments/<item id>/` |
| Write tool | *(external — notification router for `scheduled` and webhook-triggered `external` results, WS `/inbox`, `POST /api/agents/<agent>/agent-inbox`, and the companion item of each file a Discord, Telegram, Teams or A2A message carries or an A2A task's remote agent returns as an artifact)* | `user_inbox_add` |
| Read/manage tools | `inbox_list`, `inbox_read`, `inbox_archive`, `inbox_restore` | `user_inbox_list` and `inbox_read` (read-only, every agent's items); the user reads, archives and answers in the web UI |
| Consumer | The agent itself | The user, via the web UI |

Archiving is a soft delete, not a permanent one, in both inboxes: the item's JSON file (and its attachments directory, if it has one) just moves under `archive/inbox/`, so restoring it is the same move in reverse. You restore your own agent inbox items with `inbox_restore`; the user restores theirs through the web UI's archived view. The user inbox is served straight from its files, so the user can list, read, archive, and restore its items while an agent is stopped.

The agent inbox is where the notification router files results — every substantive `scheduled` result and every substantive webhook-triggered `external` result — for the agent to triage. A conversation-triggered `external` result (A2A, or a non-owner Discord/Telegram/Teams chat) never lands here: its output already went back to the conversation it came from, and its observations are merged into memory as an episode. `artifact` and `spawned` session results are relayed elsewhere too (see [background-tasks](background-tasks.md#result-routing)). The user inbox is a delivery channel: the agent (most often a sub-agent like `introspection`) writes findings there with `user_inbox_add`, and the user reads, archives and answers them through the web UI. Every agent can see every agent's user-inbox items (`user_inbox_list`, `inbox_read` with a ref) but never changes them.

## InboxItem Format

Each item is a JSON file. There is **no `id` field in the JSON body** — the item's ID is its filename stem (e.g. a file named `20260227_deploy-completed.json` has ID `20260227_deploy-completed`):

```json
{
  "title": "Deploy completed",
  "body": "Production deploy v2.3.1 finished successfully.",
  "source": "deploy-watcher",
  "timestamp": "2026-02-27T14:30",
  "read": false,
  "attachments": [],
  "in_reply_to": "atlas/agent/20260227_deploy-status"
}
```

`in_reply_to` is present only on an item that answers a mail message. It holds that message's **ref**, `<agent>/<user|agent>/<id>`: the agent's folder name, which inbox, and the item ID. A ref names one item anywhere on the hub, active or archived.

## Mail threads

The user's mail with the agents is made of inbox items: a message the user sends an agent from the web UI (or the CLI) is an item in that agent's agent inbox with source `web` or `cli`, one copy per recipient; a message an agent sends the user is an item in that agent's user inbox. Items linked by `in_reply_to` form a thread, across agents and across the archive, and the thread's subject is its first message's title. The user's replies arrive in your agent inbox with `in_reply_to` naming the message they answer. A message the user sends several agents is one copy per recipient, all with the same ID, and the user sees it as one message. Only items with source `web` or `cli` are the user's mail; other agent-inbox items (scheduled results, webhooks, pulses, artifacts, file companions) are never part of a thread.

Your reply to the user goes to the user only. The user's reply-all can name every agent that wrote in the thread, and agents that need to coordinate use `message_agent`.

IDs are unique within an agent. A filename is the date plus the sanitized title; when that stem is already taken, by an item in the inbox or in its archive, `_2`, `_3`, ... is appended, so two items with the same title on one day don't overwrite each other and an archived item's ID is never reused. `user_inbox_add` returns the ID it used, which may carry such a suffix.

## Agent Inbox Tools

| Tool | Parameters | Description |
|------|-----------|-------------|
| `inbox_list` | `unread_only` (bool, optional), `archived` (bool, optional) | List agent inbox items. Defaults to showing all active items; set `unread_only: true` to filter. Set `archived: true` to list `archive/inbox/agent/` instead — this ignores `unread_only`, since everything there is already read. |
| `inbox_read` | `id` (string — a filename stem of your agent inbox, or any item's ref) | Read one item: yours by ID, or any agent's agent-inbox or user-inbox item by ref. Shows its ref and, for mail, the thread's earlier messages oldest first and the refs of later ones. **Marks it as read** only when it is an active item of your own agent inbox. |
| `inbox_archive` | `ids` (array of strings — filename stems) | Move one or more items from `inbox/agent/` to `archive/inbox/agent/`. |
| `inbox_restore` | `ids` (array of strings — filename stems) | Move one or more items from `archive/inbox/agent/` back to `inbox/agent/`. Find the stem to restore with `inbox_list archived: true`. |

## User Inbox Tools

| Tool | Parameters | Description |
|------|-----------|-------------|
| `user_inbox_add` | `title` (string; optional when `in_reply_to` is set), `body` (string), `in_reply_to` (ref, optional), `attachments` (paths, optional) | Write an item to your user inbox. With `in_reply_to` it answers that message in its thread, titled `Re: <subject>` unless you give a title; an unknown ref is refused and nothing is written. |
| `user_inbox_list` | `agent` (folder name, optional), `include_archived` (bool, optional) | List every agent's user-inbox items newest first, with ref, title, agent, time, read and archived state, and the message each answers. Read-only; at most 100 items, with a note when more match. |

Neither tool reads or changes the user's read and archived state: archiving and restoring user-inbox items is the user's, in the web UI.

## Typical Agent-Inbox Workflow

1. Check for unread items: `inbox_list` with `unread_only: true`.
2. Read items of interest: `inbox_read` with the item ID (filename stem).
3. Act on the content (reply, create a task, etc.).
4. Archive processed items: `inbox_archive` with the IDs.

## Delivering to the User Inbox

Use `user_inbox_add` (title + body) when a background task — most often a sub-agent that only talks to the user asynchronously — has findings the user should see but that don't need to interrupt a conversation. Before adding a new item, check what every agent has already sent (`user_inbox_list` with `include_archived: true`, then `inbox_read` on the relevant refs) so you don't repeat a suggestion the user already saw. To answer the user's mail, reply in its thread with `in_reply_to` set to the ref of the message you answer.

Pass `attachments` — an array of paths to files you've already written to disk — when the finding is easier to review as a file than as inline text (an export, a screenshot, a generated report). Each file is copied into the item's own storage, so the original can safely be moved or deleted afterward. If an attachment can't be copied, the item is still created with the ones that were, and the result lists each file that failed, so send the missing file in a follow-up item if the user needs it.

## Integration with Notifications

The notification router creates an item in the **agent inbox** (`inbox/agent/`) for every `Normal` or `Urgent` result from a `scheduled` session, and from a webhook-triggered `external` session, with the result's summary as the body and its source label as the source. See [notifications](notifications.md). This is a separate path from `user_inbox_add`.

## Workbench Artifacts

A workbench artifact can file an agent-inbox item directly with `POST /api/agents/<agent>/agent-inbox` (`{ title?, body }`), the same queue `inbox_list`/`inbox_read`/`inbox_archive` work from. Its title defaults to the body's first line, in full, and a blank body is refused. The item's source records which artifact sent it (`artifact:<name>`), or `api` for a request without the artifact header. Neither is the user's mail. There is no equivalent for the user inbox — an artifact still has no way to write there.

## Gotchas

- `inbox_read` marks an item of your own agent inbox as read immediately — there is no way to mark it unread again.
- Archived items are moved (not copied) to the matching `archive/inbox/{agent,user}/` directory. The original file is removed from the source directory. `inbox_restore` (agent inbox) or the web UI's archived view (user inbox) moves it back.
- There is no unread-count surfaced anywhere in the agent's context or status line — check with `inbox_list unread_only: true` if you need to know.
- `user_inbox_add`'s `attachments` parameter is best-effort: a file that fails to copy is skipped and named in the result, and the item is created with the rest.
- `inbox_read` lists an item's attachment paths (absolute for another agent's item), which you can open with `read_file`. Archiving and restoring user-inbox items is web-UI-only.
