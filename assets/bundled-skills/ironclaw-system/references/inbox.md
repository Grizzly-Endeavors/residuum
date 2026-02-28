# Inbox

The inbox is a simple message queue stored as individual JSON files in `inbox/`. Items arrive from notifications, background tasks, or manual additions.

## InboxItem Format

Each item is a JSON file in `inbox/`:

```json
{
  "id": "inbox-a1b2c3d4",
  "title": "Deploy completed",
  "body": "Production deploy v2.3.1 finished successfully.",
  "source": "deploy-watcher",
  "timestamp": "2026-02-27T14:30:00Z",
  "read": false,
  "attachments": []
}
```

## Tools

| Tool | Parameters | Description |
|------|-----------|-------------|
| `inbox_list` | `unread_only` (bool, optional) | List inbox items. Defaults to showing all; set `unread_only: true` to filter. |
| `inbox_read` | `id` (string) | Read a single inbox item by ID. **Marks it as read** as a side effect. |
| `inbox_add` | `title` (string), `body` (string), `source` (string, optional) | Create a new inbox item. |
| `inbox_archive` | `ids` (array of strings) | Move one or more items from `inbox/` to `archive/inbox/`. |

## Typical Workflow

1. Check for unread items: `inbox_list` with `unread_only: true`.
2. Read items of interest: `inbox_read` with the item ID.
3. Act on the content (reply, create a task, etc.).
4. Archive processed items: `inbox_archive` with the IDs.

## Integration with Notifications

When `NOTIFY.yml` routes a task to the `inbox` channel, the notification system automatically creates an inbox item with the task result as the body and the task name as the source. See [notifications](notifications.md).

## Gotchas

- `inbox_read` marks the item as read immediately — there is no way to mark it unread again.
- Archived items are moved (not copied) to `archive/inbox/`. The original file is removed from `inbox/`.
- Item IDs are generated as `inbox-{8 hex chars}`.
- Attachments are supported in the schema but currently unused by built-in systems.
