# Inbox

There are **two** inboxes, stored as individual JSON files under the workspace root. They are separate queues with different tools, consumers, and archive locations — don't conflate them.

| | Agent inbox | User inbox |
|---|---|---|
| Path | `inbox/agent/` | `inbox/user/` |
| Archive path | `archive/inbox/agent/` | `archive/inbox/user/` |
| Write tool | *(external — notification router for `scheduled` and webhook-triggered `external` results, WS `/inbox`, `POST /api/agent-inbox`)* | `user_inbox_add` |
| Read/manage tools | `inbox_list`, `inbox_read`, `inbox_archive` | *(none — consumed via the web UI)* |
| Consumer | The agent itself | The user, via the web UI |

The agent inbox is where the notification router files results — every substantive `scheduled` result and every substantive webhook-triggered `external` result — for the agent to triage. A conversation-triggered `external` result (A2A, or a non-owner Discord/Telegram/Teams chat) never lands here: its output already went back to the conversation it came from, and its observations are merged into memory as an episode. `artifact` and `spawned` session results are relayed elsewhere too (see [background-tasks](background-tasks.md#result-routing)). The user inbox is a delivery channel: the agent (most often a sub-agent like `introspection`) writes findings there with `user_inbox_add`, and the user reads/archives them through the web UI, not through agent tools.

## InboxItem Format

Each item is a JSON file. There is **no `id` field in the JSON body** — the item's ID is its filename stem (e.g. a file named `20260227_deploy-completed.json` has ID `20260227_deploy-completed`):

```json
{
  "title": "Deploy completed",
  "body": "Production deploy v2.3.1 finished successfully.",
  "source": "deploy-watcher",
  "timestamp": "2026-02-27T14:30",
  "read": false,
  "attachments": []
}
```

## Agent Inbox Tools

| Tool | Parameters | Description |
|------|-----------|-------------|
| `inbox_list` | `unread_only` (bool, optional) | List agent inbox items. Defaults to showing all; set `unread_only: true` to filter. |
| `inbox_read` | `id` (string — the filename stem) | Read a single agent inbox item by ID. **Marks it as read** as a side effect. |
| `inbox_archive` | `ids` (array of strings — filename stems) | Move one or more items from `inbox/agent/` to `archive/inbox/agent/`. |

There is no tool to read or manage the user inbox from the agent side — it's write-only for the agent (`user_inbox_add`), the user handles read/archive themselves.

## Typical Agent-Inbox Workflow

1. Check for unread items: `inbox_list` with `unread_only: true`.
2. Read items of interest: `inbox_read` with the item ID (filename stem).
3. Act on the content (reply, create a task, etc.).
4. Archive processed items: `inbox_archive` with the IDs.

## Delivering to the User Inbox

Use `user_inbox_add` (title + body) when a background task — most often a sub-agent that only talks to the user asynchronously — has findings the user should see but that don't need to interrupt a conversation. Before adding a new item, it's worth checking prior items (including the archive) so you don't repeat a suggestion the user already saw.

Pass `attachments` — an array of paths to files you've already written to disk — when the finding is easier to review as a file than as inline text (an export, a screenshot, a generated report). Each file is copied into the item's own storage, so the original can safely be moved or deleted afterward. If any attachment can't be copied, the whole call fails and no item is created — retry with valid paths rather than expecting a partial item.

## Integration with Notifications

The notification router creates an item in the **agent inbox** (`inbox/agent/`) for every `Normal` or `Urgent` result from a `scheduled` session, and from a webhook-triggered `external` session, with the result's summary as the body and its source label as the source. See [notifications](notifications.md). This is a separate path from `user_inbox_add`.

## Workbench Artifacts

A workbench artifact can file an agent-inbox item directly with `POST /api/agent-inbox` (`{ title?, body }`), the same queue `inbox_list`/`inbox_read`/`inbox_archive` work from. Its title defaults to the body's first line, in full, and a blank body is refused. The item's source records which artifact sent it (`artifact:<name>`) or `web` for a direct web UI call. There is no equivalent for the user inbox — an artifact still has no way to write there.

## Gotchas

- `inbox_read` marks the item as read immediately — there is no way to mark it unread again.
- Archived items are moved (not copied) to the matching `archive/inbox/{agent,user}/` directory. The original file is removed from the source directory.
- There is no unread-count surfaced anywhere in the agent's context or status line — check with `inbox_list unread_only: true` if you need to know.
- `user_inbox_add`'s `attachments` parameter is all-or-nothing: if one file in the batch fails to copy, none of them are attached and no item is created.
- There is no tool to list, read, or archive the user inbox's attachments from the agent side — same as the rest of the user inbox, they're write-only for you.
