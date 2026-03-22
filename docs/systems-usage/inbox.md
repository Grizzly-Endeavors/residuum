# Inbox

The inbox is a capture system for items the agent or background tasks want to save for later triage. Items are stored as individual JSON files in the workspace `inbox/` directory.

## How Items Arrive

- The LLM notification router delivers results to inbox based on ALERTS.md policy
- Webhook-triggered results can be routed to inbox by the notification router
- Users can add items via the HTTP API/UI

## Unread Count in Context

The agent sees an unread inbox count in its context assembly (e.g., "You have 3 unread inbox items"). This happens automatically — the agent does not need to call `inbox_list` to know items are waiting.

## Item Format

Each item is a JSON file in `inbox/`:

```json
{
  "id": "inbox-a1b2c3d4",
  "title": "Deploy completed",
  "body": "Production deployment finished successfully. 3 services updated.",
  "source": "deploy-watcher",
  "timestamp": "2026-02-27T14:30:00Z",
  "read": false,
  "attachments": []
}
```

- IDs are generated as `inbox-{8 hex chars}`
- Filenames are auto-generated from date and sanitized title
- `attachments` is supported in the schema but currently unused

## Tools

| Tool | Parameters | Notes |
|------|-----------|-------|
| `inbox_list` | `unread_only` (bool, optional, default false) | Lists all items |
| `inbox_read` | `id` (string) | Reads item content, marks as read as a side effect. Cannot be unmarked. |
| `inbox_archive` | `ids` (string[]) | Moves items from `inbox/` to `archive/inbox/`. This is a move, not a copy. |

## Intended Usage

The inbox is for **low-urgency items** that don't need immediate attention. Background task results that are informational but not actionable should route here rather than to `agent_wake` or `agent_feed`.

The agent should periodically triage the inbox — reading items, acting on anything that needs follow-up, and archiving items that are resolved. This should be driven by a heartbeat pulse.
