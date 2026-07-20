# Inbox

There are **two** inboxes, stored as individual JSON files under the workspace root. They are separate queues with different tools, consumers, and archive locations — don't conflate them.

| | Agent inbox | User inbox |
|---|---|---|
| Path | `inbox/agent/` | `inbox/user/` |
| Archive path | `archive/inbox/agent/` | `archive/inbox/user/` |
| Write tool | *(external — background tasks, notification router)* | `user_inbox_add` |
| Read/manage tools | `inbox_list`, `inbox_read`, `inbox_archive` | *(none — consumed via the web UI)* |
| Consumer | The agent itself | The user, via the web UI |

The agent inbox is where background results land when the notification router's `inbox` channel target is used — it's a queue for the agent to triage. The user inbox is a delivery channel: the agent (most often a sub-agent like `introspection`) writes findings there with `user_inbox_add`, and the user reads/archives them through the web UI, not through agent tools.

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

## Integration with Notifications

When a task's `channels` configuration includes `inbox`, the notification router creates an item in the **agent inbox** (`inbox/agent/`) with the task result as the body and the task name as the source. See [notifications](notifications.md). This is a separate path from `user_inbox_add`.

## Gotchas

- `inbox_read` marks the item as read immediately — there is no way to mark it unread again.
- Archived items are moved (not copied) to the matching `archive/inbox/{agent,user}/` directory. The original file is removed from the source directory.
- There is no unread-count surfaced anywhere in the agent's context or status line — check with `inbox_list unread_only: true` if you need to know.
- Attachments are supported in the schema but currently unused by built-in systems.
