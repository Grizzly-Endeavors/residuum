# Scheduled Actions

Scheduled actions are one-off future tasks. They fire once at a specified time and are automatically removed afterward. For recurring tasks, use [heartbeats](heartbeats.md).

## How They Work

1. Agent (or user via agent) creates an action with `schedule_action`
2. Action persisted to `scheduled_actions.json` (atomic write: temp file + rename)
3. Gateway checks for due actions on a **30-second tick**
4. When `run_at` has passed: action removed from persistence, a `scheduled` session is forked
5. Results delivered by the notification router according to the disposition the agent declared

If the gateway was offline when an action was due, it fires on next startup.

## Tools

### `schedule_action`

| Parameter | Type | Required | Notes |
|-----------|------|----------|-------|
| `name` | string | yes | Human-readable label |
| `prompt` | string | yes | The prompt to execute when the action fires |
| `run_at` | string | yes | Local time without offset (e.g. `2026-03-01T09:00:00`). Interpreted in configured workspace timezone. Displayed times are also local. Must be in the future. |
| `agent_name` | string | no | `"<skill>"` = session forked with that skill as its role. Omitted = plain session with no skill. `"main"` is rejected. |
| `model_tier` | string enum | no | `"small"`, `"medium"`, `"large"`. |

### `list_actions`

No parameters. Returns all pending actions.

### `cancel_action`

| Parameter | Type | Required | Notes |
|-----------|------|----------|-------|
| `id` | string | yes | Action ID (e.g. `"action-a1b2c3d4"`) |

## Routing

Scheduled action results flow through the pub/sub bus to the notification router and are filed to the inbox, or pushed to every configured notification channel as well when the summary contains `HEARTBEAT_URGENT`.

See [notifications.md](notifications.md) for the full routing model.

## Persistence

- Actions stored in `scheduled_actions.json` at the workspace root
- `ActionStore` handles concurrent access
- IDs generated as `action-{8 hex chars}`
- Managed exclusively via tools — the agent should not edit `scheduled_actions.json` directly
- A stored action left over from before `agent: "main"` was removed is dropped at startup load — never silently reinterpreted as a plain session — logged as an error naming the action, and also raised as an owner-facing notice (a web UI toast, and the same message on any chat interface) naming every dropped action and linking to [`migrating-to-agent-sessions.md`](../guides/migrating-to-agent-sessions.md). This only happens once, right after the store loads at startup, since actions (unlike heartbeat pulses) aren't re-validated on a running tick.
