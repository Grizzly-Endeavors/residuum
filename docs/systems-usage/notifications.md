# Notifications

The notification system routes results from background tasks (heartbeat pulses, scheduled actions, agent-spawned sub-agents) to appropriate destinations. Routing is handled by a pub/sub bus with a dedicated LLM-based notification router.

## Routing Architecture

### Two-layer routing

When a background task completes:

1. **Layer 1 — Programmatic rules** (no LLM call):
   - `HEARTBEAT_OK` results from pulses are silently discarded (logged only).
   - Results from agent-spawned tasks are relayed back to the main agent as an interrupt.

2. **Layer 2 — LLM router** (everything not handled by Layer 1):
   - A small model receives the result content, metadata, available endpoints, and the ALERTS.md policy.
   - It decides which endpoints to deliver to: notification channels, inbox, interactive endpoints, or nothing.

### ALERTS.md

`ALERTS.md` is the user-editable routing policy that the LLM router reads on every routing decision. Edits take effect immediately without restart. The agent can modify it at the user's request using standard file tools.

## Endpoints

The endpoint registry tracks all available I/O endpoints. The `list_endpoints` tool shows what's available.

### Interactive endpoints

Bidirectional channels (WebSocket, Discord, Telegram). The agent can:
- `switch_endpoint` to redirect responses to a different interactive endpoint.
- `send_message` to send a one-off message to any interactive endpoint.

### Notification endpoints

Output-only channels for push delivery. Configured in `config/channels.toml`.

| Type | Description |
|------|-------------|
| `ntfy` | Push notification via ntfy-compatible server. |
| `webhook` | HTTP POST to a configured URL. |
| `macos` | macOS native notification (when running on macOS). |

**Note**: The `webhook` external notification channel is separate from the `webhook` inbound channel (which receives messages *into* the agent via `POST /webhook`). They serve opposite directions.

### Inbox

Input-only. Items arrive from the LLM notification router, webhook routing, and the HTTP API/UI.

## Built-in Routing Targets

| Target | Behavior |
|--------|----------|
| `agent_wake` | Injects result into the agent's feed and starts a turn if the agent is idle. If the agent is already in a turn, the result is injected at the next interrupt checkpoint. |
| `agent_feed` | Injects result into the agent's feed passively. If idle, queued for the next user interaction. Does **not** start a turn on its own. |
| `inbox` | Creates an inbox item with the task result as body and task name as source. Never enters the message feed. Agent sees the unread count. |

### `agent_wake` vs `agent_feed`

The key distinction: `agent_wake` can cause the agent to start talking unprompted. `agent_feed` waits for the next natural interaction. Use `agent_wake` only for things that genuinely need immediate attention.

Results delivered to `agent_wake` or `agent_feed` are not dropped if the agent is already busy — they are injected at the next interrupt checkpoint.

## Tools

| Tool | Purpose |
|------|---------|
| `list_endpoints` | Show available interactive and notification endpoints. |
| `switch_endpoint` | Redirect subsequent responses to a different interactive endpoint. Auto-clears when the user sends a message. |
| `send_message` | One-off message to any interactive or notification endpoint. Does not change where turn responses go. |

## HEARTBEAT_OK

Sub-agent pulses include an instruction: if nothing actionable was found, return the exact string `HEARTBEAT_OK`. Results containing this string are silently discarded **before routing** — they never reach any endpoint.

## channels.toml

External notification channels are configured in `config/channels.toml`:

### ntfy

```toml
[channels.phone]
type = "ntfy"
url = "https://ntfy.sh"
topic = "my-agent-alerts"
```

### webhook

```toml
[channels.ops_hook]
type = "webhook"
url = "https://hooks.example.com/agent"
method = "POST"                     # optional, default POST
# headers = { Authorization = "Bearer ..." }  # optional
```

External channel delivery failures are logged at warn level. They do not retry or block other channels.

## Agent Self-Evolution

The agent can edit `ALERTS.md` to adjust routing policy based on what's useful. If a certain class of results keeps generating noise, the agent should update the routing rules to redirect them to inbox or suppress them.
