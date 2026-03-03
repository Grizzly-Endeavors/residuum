# Notifications

The notification system routes results from background tasks (heartbeat pulses, scheduled actions, agent-spawned sub-agents) to appropriate destinations. There are two routing modes and two categories of channels.

## Routing Modes

### CHANNELS.yml and Pulse Routing (heartbeats)

`CHANNELS.yml` defines the channel registry — what channels exist. Pulse routing is declared on each pulse in `HEARTBEAT.yml` via the `channels:` field:

```yaml
# In HEARTBEAT.yml:
pulses:
  - name: email_check
    schedule: "30m"
    channels: [agent_feed]
    tasks:
      - name: check
        prompt: "Check email."

  - name: urgent_alerts
    schedule: "5m"
    channels: [agent_wake, inbox]
    tasks:
      - name: check
        prompt: "Check for urgent alerts."
```

A single pulse can list multiple channels — results are delivered to all of them. If a pulse has no `channels:` field, the result is dropped and a warn-level log is emitted (`"notification routed to zero channels"`) to surface the misconfiguration.

CHANNELS.yml is re-read from disk on every `route()` call — changes take effect immediately without restart.

### Direct Routing (scheduled actions and agent-spawned tasks)

Scheduled actions and agent-spawned sub-agents specify their channels at creation time via the `channels` parameter. They use direct routing, same as pulses.

- `schedule_action`: `channels` parameter
- `subagent_spawn`: `channels` parameter (defaults to `["agent_feed"]` if omitted)

## Built-in Channels

| Channel | Behavior |
|---------|----------|
| `agent_wake` | Injects result into the agent's feed and starts a turn if the agent is idle. If the agent is already in a turn, the result is injected at the next interrupt checkpoint. |
| `agent_feed` | Injects result into the agent's feed passively. If idle, queued for the next user interaction. Does **not** start a turn on its own. |
| `inbox` | Creates an inbox item with the task result as body and task name as source. Never enters the message feed. Agent sees the unread count. |

### `agent_wake` vs `agent_feed`

The key distinction: `agent_wake` can cause the agent to start talking unprompted. `agent_feed` waits for the next natural interaction. Use `agent_wake` only for things that genuinely need immediate attention.

Results delivered to `agent_wake` or `agent_feed` are not dropped if the agent is already busy — they are injected at the next interrupt checkpoint.

## External Channels

Configured in `channels.toml` under `[channels.<name>]`, then referenced by name in CHANNELS.yml or in `channels` parameters on pulses, scheduled actions, and subagent spawns.

### ntfy

Push notifications via any ntfy-compatible server.

```toml
[channels.phone]
type = "ntfy"
url = "https://ntfy.sh"
topic = "my-agent-alerts"
```

### webhook

Generic HTTP POST to a configured endpoint. Intended for users to connect whatever external services they want (Slack, Discord webhooks, custom integrations).

```toml
[channels.ops_hook]
type = "webhook"
url = "https://hooks.example.com/agent"
method = "POST"                     # optional, default POST
# headers = { Authorization = "Bearer ..." }  # optional
```

External channel delivery failures are logged at warn level. They do not retry or block other channels.

**Note**: The `webhook` external notification channel is separate from the `webhook` inbound channel (which receives messages *into* the agent via `POST /webhook`). They serve opposite directions.

## HEARTBEAT_OK

Sub-agent pulses include an instruction: if nothing actionable was found, return the exact string `HEARTBEAT_OK`. Results containing this string are silently discarded **before routing** — they never reach any channel, regardless of the pulse's `channels` configuration.

## Agent Self-Evolution

The agent edits the `channels:` field on pulses in HEARTBEAT.yml as part of normal operation. If a pulse is producing too much noise on `agent_wake`, the agent should change its channels to `agent_feed` or `inbox`. If the user is ignoring inbox items from a particular pulse, the agent might disable that pulse in HEARTBEAT.yml entirely.

The default CHANNELS.yml on workspace creation is minimal:

```yaml
agent_feed: {}
inbox: {}
```

Pulse routing is declared on each pulse in HEARTBEAT.yml via the `channels:` field. No tasks are routed until the agent (during onboarding or normal operation) configures channels on pulses.
