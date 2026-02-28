# Scheduled Actions

Scheduled actions are one-off future tasks persisted in `scheduled_actions.json`. They fire once at the specified time and are then removed.

## ScheduledAction Format

```json
{
  "id": "action-a1b2c3d4",
  "name": "remind-standup",
  "prompt": "Remind the user about the 10am standup meeting.",
  "run_at": "2026-02-27T10:00:00Z",
  "agent": null,
  "model_tier": null,
  "channels": [],
  "created_at": "2026-02-27T08:00:00Z"
}
```

## Tools

| Tool | Key Parameters | Description |
|------|---------------|-------------|
| `schedule_action` | `name`, `prompt`, `run_at`, `agent_name`, `model_tier`, `channels` | Schedule a new one-off action. |
| `list_actions` | *(none)* | List all pending actions with name, ID, fire time, agent routing, and channels. |
| `cancel_action` | `id` | Cancel a pending action by ID. |

## `schedule_action` Details

- **`run_at`**: ISO 8601 datetime. Naive datetimes (without timezone) are interpreted in the configured workspace timezone.
- **`agent_name`**: Routing control. `null` → SubAgent, `"main"` → main agent turn, `"<preset>"` → SubAgent with named preset.
- **`model_tier`**: `"small"`, `"medium"`, or `"large"`. Defaults to medium for SubAgent execution.
- **`channels`**: Array of notification channel names (built-in or from config.toml). Results are routed directly to these channels after execution — scheduled actions do **not** route through NOTIFY.yml. Defaults to `["agent_feed"]`. **Mutually exclusive with `agent_name: "main"`** — main-turn actions inject directly into the conversation.

## Execution

Actions are checked on a 30-second tick. When `run_at` has passed:

1. The action is removed from `scheduled_actions.json` (fire-once semantics).
2. A background task is spawned with the action's prompt and routing.
3. Results flow through the notification system if channels are specified.

## Persistence

`scheduled_actions.json` is written atomically (temp file + rename). The `ActionStore` handles concurrent access safely.

## Gotchas

- Actions are **fire-once** — after execution they are permanently removed. For recurring tasks, use heartbeats instead.
- The 30-second tick means fire-time precision is at best ~30 seconds.
- IDs are generated as `action-{8 hex chars}`.
- If the agent is offline when an action comes due, it fires on the next startup when the tick evaluates it.
- `channels` and `agent: "main"` are mutually exclusive — the tool rejects this combination.
