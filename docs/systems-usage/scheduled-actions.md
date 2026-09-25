# Scheduled Actions

Scheduled actions are one-off future tasks. They fire once at a specified time and are automatically removed afterward. For recurring tasks, use [heartbeats](heartbeats.md).

## How They Work

1. Agent (or user via agent) creates an action with `schedule_action`
2. Action persisted to `scheduled_actions.json` (atomic write: temp file + rename)
3. Gateway checks for due actions on a **30-second tick**
4. When `run_at` has passed, the gateway publishes a spawn request for a `scheduled` session; the action is removed from persistence only once that publish has actually succeeded, never before. If the publish fails, the action stays exactly as it was and is picked up again on the next tick — it is never silently lost to a failure at this step.
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

Scheduled action results flow through the pub/sub bus to the notification router and are filed to the inbox, or pushed to every configured notification channel as well when the summary contains `HEARTBEAT_URGENT`. A failed action run's inbox item names the failure reason (or that the run was stopped) directly, and also publishes its own owner-facing notice, separate from the inbox item.

See [notifications.md](notifications.md) for the full routing model.

## Scheduled View

The web UI's Scheduled view (opened from the hamburger menu, and at `/scheduled`; see [heartbeats.md](heartbeats.md#scheduled-view)) lists every pending action — what it does, when it's due, its agent/skill, and whether it's currently running — with a cancel button for each, backed by `GET /api/scheduled/actions` and `DELETE /api/scheduled/actions/{id}`.

## Persistence

- Actions stored in `scheduled_actions.json` at the workspace root
- `ActionStore` handles concurrent access
- IDs generated as `action-{8 hex chars}`
- Managed exclusively via tools — the agent should not edit `scheduled_actions.json` directly
- A stored action left over from before `agent: "main"` was removed is dropped at startup load — never silently reinterpreted as a plain session — logged as an error naming the action, and also raised as an owner-facing notice (a web UI toast, and the same message on any chat interface) naming every dropped action and linking to [`migrating-to-agent-sessions.md`](../guides/migrating-to-agent-sessions.md). This only happens once, right after the store loads at startup, since actions (unlike heartbeat pulses) aren't re-validated on a running tick.
- A `scheduled_actions.json` that exists but isn't valid JSON is never overwritten: it's moved aside to `scheduled_actions.json.corrupt-<unix-timestamp>` in the same directory, preserving its original bytes, and the store starts empty at the normal path. The owner is notified naming the moved-aside file. This also only happens once, at startup load.
- Fork/spawn failures for an action (e.g. naming a skill that doesn't exist) are deduped per action name: warn-logged and noticed once, not on every retry of an unchanged failure — a repeat with the same error is logged at `debug!` instead, and a fresh notice fires only if the error changes or after a later recovery and re-failure.
