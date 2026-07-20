# Inbox

The inbox is a capture system for items the agent or background tasks want to save for later triage. There are **two separate inboxes**, each stored as individual JSON files under the workspace root — they have different paths, different tools, and different consumers.

| | Agent inbox | User inbox |
|---|---|---|
| Path | `inbox/agent/` | `inbox/user/` |
| Archive path | `archive/inbox/agent/` | `archive/inbox/user/` |
| Populated by | Notification router (`inbox` channel target), background tasks | `user_inbox_add` tool |
| Read/manage tools | `inbox_list`, `inbox_read`, `inbox_archive` | *(none for the agent)* |
| Consumed by | The agent, via the tools above | The user, via the web UI/HTTP API |

The agent inbox is a queue for the agent itself to triage — it's where the `inbox` notification-routing target delivers results. The user inbox is a one-way delivery channel *to* the user: the agent (often a background sub-agent, e.g. the built-in `introspection` preset) writes to it with `user_inbox_add`, and the user reads and archives items through the web UI. The agent has no tool to list, read, or archive the user inbox — only to add to it.

## How Items Arrive

- **Agent inbox**: the LLM notification router delivers results here when a pulse/task's `channels` list includes `inbox`, per `ALERTS.md` policy.
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

- Filenames are auto-generated from date and sanitized title; the filename stem *is* the ID used by `inbox_read`/`inbox_archive`.
- `attachments` is supported in the schema but currently unused.
- There is no unread-count surfaced anywhere in the agent's context or status line — the agent has to call `inbox_list` (with `unread_only: true`) to find out.

## Tools (Agent Inbox Only)

| Tool | Parameters | Notes |
|------|-----------|-------|
| `inbox_list` | `unread_only` (bool, optional, default false) | Lists agent inbox items |
| `inbox_read` | `id` (string — filename stem) | Reads item content, marks as read as a side effect. Cannot be unmarked. |
| `inbox_archive` | `ids` (string[] — filename stems) | Moves items from `inbox/agent/` to `archive/inbox/agent/`. This is a move, not a copy. |

## Intended Usage

The agent inbox is for **low-urgency items** that don't need immediate attention — background task results that are informational but not actionable should route here rather than to `agent_wake` or `agent_feed`. The agent should periodically triage it — reading items, acting on anything that needs follow-up, and archiving items that are resolved. This should be driven by a heartbeat pulse.

The user inbox is for findings the agent wants to hand to the user asynchronously, without interrupting a conversation — e.g. the built-in `reflection` and `memory_tending` pulses deliver their output there. Before adding a new item, check prior items (including the archive) so the same suggestion isn't repeated.
