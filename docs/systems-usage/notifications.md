# Notifications

The notification system routes results from background tasks (heartbeat pulses, scheduled actions, agent-spawned sub-agents) to appropriate destinations. There are two routing modes and two categories of channels.

## Routing Modes

### NOTIFY.yml Routing (heartbeats only)

Heartbeat pulse results are routed by looking up the **pulse name** in NOTIFY.yml. The file maps channel names to lists of pulse names:

```yaml
agent_feed:
  - email_check
  - deploy_watch

inbox:
  - system_health
  - log_rotation

agent_wake:
  - urgent_alerts
```

A single pulse name can appear in multiple channels — results are delivered to all of them. If a pulse name doesn't appear in any channel, the result is dropped and a warn-level log is emitted (`"notification routed to zero channels"`) to surface the misconfiguration.

NOTIFY.yml is re-read from disk on every `route()` call — changes take effect immediately without restart.

### Direct Routing (scheduled actions and agent-spawned tasks)

Scheduled actions and agent-spawned sub-agents specify their channels at creation time via the `channels` parameter. They bypass NOTIFY.yml entirely.

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

Configured in `config.toml` under `[notifications.channels.<name>]`, then referenced by name in NOTIFY.yml or in `channels` parameters.

### ntfy

Push notifications via any ntfy-compatible server.

```toml
[notifications.channels.phone]
type = "ntfy"
url = "https://ntfy.sh"
topic = "my-agent-alerts"
```

### webhook

Generic HTTP POST to a configured endpoint. Intended for users to connect whatever external services they want (Slack, Discord webhooks, custom integrations).

```toml
[notifications.channels.ops_hook]
type = "webhook"
url = "https://hooks.example.com/agent"
method = "POST"                     # optional, default POST
# headers = { Authorization = "Bearer ..." }  # optional
```

External channel delivery failures are logged at warn level. They do not retry or block other channels.

**Note**: The `webhook` external notification channel is separate from the `webhook` inbound channel (which receives messages *into* the agent via `POST /webhook`). They serve opposite directions.

## HEARTBEAT_OK

Sub-agent pulses include an instruction: if nothing actionable was found, return the exact string `HEARTBEAT_OK`. Results containing this string are silently discarded **before routing** — they never reach any channel, regardless of NOTIFY.yml configuration.

## Agent Self-Evolution

The agent edits NOTIFY.yml as part of normal operation. If a pulse is producing too much noise on `agent_wake`, the agent should move it to `agent_feed` or `inbox`. If the user is ignoring inbox items from a particular pulse, the agent might disable that pulse in HEARTBEAT.yml entirely.

The default NOTIFY.yml on workspace creation is minimal:

```yaml
agent_feed: []
inbox: []
```

No tasks are routed until the agent (during onboarding or normal operation) configures routing.
