# Notifications

The notification system routes task results to one or more channels based on mappings in `NOTIFY.yml`. It handles output from heartbeat pulses, scheduled actions, and agent-spawned background tasks.

## NOTIFY.yml Format

```yaml
# Maps channel names to lists of task names that route to that channel.
agent_wake:
  - daily-review
  - urgent-alerts

agent_feed:
  - check-inbox

inbox:
  - deploy-watcher
  - monitor-health

ntfy:
  - critical-alerts

webhook:
  - ci-notifications
```

A single task name can appear in multiple channels — results will be delivered to all of them.

## Built-in Channels

| Channel | Behavior |
|---------|----------|
| `agent_wake` | Sets a flag to wake the main agent for a new turn. |
| `agent_feed` | Sets a flag to inject the result into the main agent's conversation feed. |
| `inbox` | Creates an inbox item with the task result as body and task name as source. |

## External Channels

| Channel | Behavior |
|---------|----------|
| `ntfy` | Dispatches via the ntfy notification service. |
| `webhook` | Sends an HTTP POST to a configured webhook URL. |

External channels are dispatched through the `NotificationChannel` trait, which provides `name()` and `deliver()` methods.

## Routing Flow

1. A background task completes and produces a `BackgroundResult`.
2. If routing is `Notify`: the router loads `NOTIFY.yml`, looks up the task name, and resolves matching channels.
3. If routing is `Direct(channels)`: the specified channel list is used directly (bypasses NOTIFY.yml).
4. Each matched channel processes the result according to its type.
5. The `RouteOutcome` reports which flags were set and which external channels were dispatched.

## Hot Reload

`NOTIFY.yml` is re-read from disk on every `route()` call. Edits take effect immediately without restart.

## Gotchas

- Task names in NOTIFY.yml must match exactly (case-sensitive) the `task_name` field on the background task.
- If a task name is not found in any channel mapping, the result is silently dropped (no error, but nothing happens).
- `agent_wake` and `agent_feed` are flags, not queues — if the agent is already awake, the flag is a no-op until the next check cycle.
- The `inbox` channel creates a new inbox item every time — it does not deduplicate.
- For scheduled actions, specify channels in the `schedule_action` tool call. For heartbeats, routing goes through NOTIFY.yml based on the pulse name.
