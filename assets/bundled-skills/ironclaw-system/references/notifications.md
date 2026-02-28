# Notifications

The notification system routes background task results to one or more channels. There are two routing modes depending on the task source.

## Routing Modes

### NOTIFY.yml (heartbeat pulses only)

Heartbeat pulse results are routed by looking up the **pulse name** in NOTIFY.yml. The file maps channel names to lists of pulse names:

```yaml
# Maps channel names to pulse names whose results they receive.
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
```

A single pulse name can appear in multiple channels — results will be delivered to all of them.

### Direct routing (scheduled actions and agent-spawned tasks)

Scheduled actions and agent-spawned sub-agents specify their channels at creation time via the `channels` parameter. They bypass NOTIFY.yml entirely.

- `schedule_action`: `channels` parameter (defaults to `["agent_feed"]`)
- `subagent_spawn`: `channels` parameter (defaults to `["agent_feed"]`)

## Built-in Channels

| Channel | Behavior |
|---------|----------|
| `agent_wake` | Injects result into the agent's feed and starts a turn if idle. If the agent is busy, the result is injected at the next interrupt checkpoint. |
| `agent_feed` | Injects result into the agent's feed passively. If idle, queued for the next user interaction. Does not start a turn. |
| `inbox` | Creates an inbox item with the task result as body and task name as source. Never enters the message feed. |

Results delivered to `agent_wake` or `agent_feed` are not dropped if the agent is busy — they are injected at the next interrupt checkpoint.

## External Channels

External channels are configured in `config.toml` under `[notifications.channels.<name>]`:

| Type | Description |
|------|-------------|
| `ntfy` | Push notification via ntfy-compatible server. |
| `webhook` | HTTP POST to a configured URL. |

External channel delivery failures are logged at warn level. They do not retry or block other channels.

## Hot Reload

`NOTIFY.yml` is re-read from disk on every `route()` call. Edits take effect immediately without restart.

## Gotchas

- Pulse names in NOTIFY.yml must match exactly (case-sensitive) the pulse `name` field in HEARTBEAT.yml.
- If a pulse name is not found in any channel mapping, the result is silently dropped.
- The `inbox` channel creates a new inbox item every time — it does not deduplicate.
- For scheduled actions, specify channels in the `schedule_action` tool call. For heartbeats, routing goes through NOTIFY.yml by pulse name.
