# Scheduled Actions

Scheduled actions are one-off future tasks. They fire once at a specified time and are automatically removed afterward. For recurring tasks, use [heartbeats](heartbeats.md).

## How They Work

1. Agent (or user via agent) creates an action with `schedule_action`
2. Action persisted to `scheduled_actions.json` (atomic write: temp file + rename)
3. Gateway checks for due actions on a **30-second tick**
4. When `run_at` has passed: action removed from persistence, background task spawned
5. Results routed through the notification router based on content and ALERTS.md policy

If the gateway was offline when an action was due, it fires on next startup.

## Tools

### `schedule_action`

| Parameter | Type | Required | Notes |
|-----------|------|----------|-------|
| `name` | string | yes | Human-readable label |
| `prompt` | string | yes | The prompt to execute when the action fires |
| `run_at` | string | yes | Local time without offset (e.g. `2026-03-01T09:00:00`). Interpreted in configured workspace timezone. Displayed times are also local. Must be in the future. |
| `agent_name` | string | no | `"main"` = full wake turn with conversation context. `"<preset>"` = sub-agent with named preset. Omitted = default sub-agent. |
| `model_tier` | string enum | no | `"small"`, `"medium"`, `"large"`. Only applies to sub-agent actions. |

### `list_actions`

No parameters. Returns all pending actions.

### `cancel_action`

| Parameter | Type | Required | Notes |
|-----------|------|----------|-------|
| `id` | string | yes | Action ID (e.g. `"action-a1b2c3d4"`) |

## Routing

Scheduled action results flow through the pub/sub bus to the LLM notification router, which decides where each result goes based on content analysis and the `ALERTS.md` policy file. Main-turn actions (`agent_name: "main"`) inject directly into the main agent conversation.

See [notifications.md](notifications.md) for the full routing model.

## Persistence

- Actions stored in `scheduled_actions.json` at the workspace root
- `ActionStore` handles concurrent access
- IDs generated as `action-{8 hex chars}`
- Managed exclusively via tools — the agent should not edit `scheduled_actions.json` directly
