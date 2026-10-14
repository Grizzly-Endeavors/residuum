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
- **`agent_name`**: Routing control. `null` → session with no skill, `"<skill>"` → session with that skill as its role. `"main"` is rejected.
- **`model_tier`**: a tier (`"small"`, `"medium"`, `"large"`) or an explicit model id in `provider/model` form, the format `providers.toml` uses (`anthropic/claude-haiku-4-5`, or `<providers entry>/<model>`). Defaults to medium. An explicit model runs that one model with no failover, and the tier's temperature/thinking overrides don't apply. A value that is neither, or names a provider that isn't configured, is rejected with an error naming it.

Results are filed to the inbox by the notification router, and pushed to every configured notification channel as well when the summary contains `HEARTBEAT_URGENT`.

## Execution

Actions are checked on a 30-second tick. Each tick first checks every pending action's model: one whose `model_tier` can't run (not a tier or `provider/model` id, or a provider `providers.toml` doesn't have) is held — kept, not run even when due — and the owner gets one notice naming it and why (again only if the reason changes). It runs on the first tick after its model works, or can be cancelled. When `run_at` has passed:

1. A spawn request for a `scheduled` session is published with the action's prompt and routing.
2. The action is removed from `scheduled_actions.json` only once that publish has actually succeeded — never before. A failed publish leaves the action exactly as it was, to be picked up again on the next tick, rather than losing it.
3. Results flow through the notification router to the inbox, and to notification channels when marked urgent. A failed run's inbox item names the failure reason directly, and also publishes its own owner-facing notice separate from the inbox item.

## Persistence

`scheduled_actions.json` is written atomically (temp file + rename). The `ActionStore` handles concurrent access safely. A file that exists but isn't valid JSON is never overwritten: it's moved aside to `scheduled_actions.json.corrupt-<unix-timestamp>`, preserving its bytes, and the store starts empty at the normal path — the owner is notified naming the moved-aside file.

## Schedule Place

The web UI's Schedule place (under each agent in the rail, `/agent/<agent>/schedule`) lists every pending action — its name in words, when it's due, its skill, its model when it names one, why it's held when its model can't run, and whether it's running now — with Cancel for each, backed by `GET /api/agents/<agent>/scheduled/actions` and `DELETE /api/agents/<agent>/scheduled/actions/{id}`.

## Gotchas

- Actions are **fire-once** — after execution they are permanently removed. For recurring tasks, use heartbeats instead.
- The 30-second tick means fire-time precision is at best ~30 seconds.
- IDs are generated as `action-{8 hex chars}`.
- If the agent is offline when an action comes due, it fires on the next startup when the tick evaluates it.
- A stored action left over from before `agent: "main"` was removed is dropped at startup load, logged as an error naming it, and raised once as an owner-facing notice (a web UI toast) naming every dropped action and linking to `migrating-to-agent-sessions.md`. This only ever happens once at startup; the model check above is the only one a running tick repeats.
- Fork/spawn failures (e.g. naming a skill that doesn't exist) are deduped per action name: warned and noticed once, not on every retry of an unchanged failure.
