# Scheduled Actions

Scheduled actions are one-off future tasks persisted in `scheduled_actions.json`. They fire once at the specified time and are then removed.

## ScheduledAction Format

```json
{
  "id": "action-a1b2c3d4",
  "name": "remind-standup",
  "prompt": "Remind the user about the 10am standup meeting.",
  "run_at": "2026-02-27T10:00:00",
  "agent": null,
  "model_tier": null,
  "created_at": "2026-02-27T08:00:00Z"
}
```

## Tools

| Tool | Key Parameters | Description |
|------|---------------|-------------|
| `schedule_action` | `name`, `prompt`, `run_at`, `agent_name`, `model_tier` | Schedule a new one-off action. |
| `list_actions` | *(none)* | List all pending actions with name, ID, fire time, and agent routing. |
| `cancel_action` | `id` | Cancel a pending action by ID. |

## `schedule_action` Details

- **`run_at`**: Local time without offset (e.g. `2026-03-01T09:00:00`). Interpreted in the configured workspace timezone. All displayed times are also in local time — no UTC conversion needed.
- **`agent_name`**: Routing control. `null` → SubAgent, `"main"` → main agent turn, `"<preset>"` → SubAgent with named preset.
- **`model_tier`**: `"small"`, `"medium"`, or `"large"`. Defaults to medium for SubAgent execution.
Results are routed through the LLM notification router based on content and `ALERTS.md` policy. Main-turn actions (`agent_name: "main"`) inject directly into the main agent conversation.

## Execution

Actions are checked on a 30-second tick. When `run_at` has passed:

1. The action is removed from `scheduled_actions.json` (fire-once semantics).
2. A background task is spawned with the action's prompt and routing.
3. Results flow through the notification router based on ALERTS.md policy.

## Persistence

`scheduled_actions.json` is written atomically (temp file + rename). The `ActionStore` handles concurrent access safely.

## Gotchas

- Actions are **fire-once** — after execution they are permanently removed. For recurring tasks, use heartbeats instead.
- The 30-second tick means fire-time precision is at best ~30 seconds.
- IDs are generated as `action-{8 hex chars}`.
- If the agent is offline when an action comes due, it fires on the next startup when the tick evaluates it.
- Main-turn actions (`agent: "main"`) inject directly into the conversation — they bypass the notification router.
