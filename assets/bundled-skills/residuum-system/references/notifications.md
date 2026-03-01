# Notifications

The notification system routes background task results to one or more channels. There are two routing modes depending on the task source.

## Routing Modes

### CHANNELS.yml and Pulse Routing (heartbeats)

`CHANNELS.yml` defines the channel registry. Pulse routing is declared on each pulse in `HEARTBEAT.yml` via the `channels:` field:

```yaml
# In HEARTBEAT.yml:
pulses:
  - name: daily-review
    schedule: "24h"
    channels: [agent_wake]
    tasks:
      - name: review
        prompt: "Review the day."

  - name: deploy-watcher
    schedule: "5m"
    channels: [inbox, ntfy]
    tasks:
      - name: check
        prompt: "Check deployments."
```

A single pulse can list multiple channels — results will be delivered to all of them.

### Direct routing (scheduled actions and agent-spawned tasks)

Scheduled actions and agent-spawned sub-agents specify their channels at creation time via the `channels` parameter. They use direct routing, same as pulses.

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

`CHANNELS.yml` is re-read from disk on every `route()` call. Edits take effect immediately without restart.

## Gotchas

- Channel names in a pulse's `channels:` field must match exactly (case-sensitive) a built-in channel or a channel defined in `config.toml` / `CHANNELS.yml`.
- If a pulse has no `channels:` field, the result is silently dropped.
- The `inbox` channel creates a new inbox item every time — it does not deduplicate.
- For scheduled actions, specify channels in the `schedule_action` tool call. For heartbeats, declare channels on the pulse in HEARTBEAT.yml.
