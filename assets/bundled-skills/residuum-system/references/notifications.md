# Notifications

The notification system routes background task results via a pub/sub bus. A dedicated LLM-based notification router subscribes to the `background:result` topic and decides where each result goes based on content analysis and the ALERTS.md policy file.

## Routing Architecture

### Two-layer routing

When a background task (pulse, scheduled action, or agent-spawned subagent) completes:

1. **Layer 1 — Programmatic rules** (no LLM call):
   - `HEARTBEAT_OK` results from pulses are silently discarded (logged only).
   - Results from agent-spawned tasks are relayed back to the main agent as an interrupt.

2. **Layer 2 — LLM router** (everything not handled by Layer 1):
   - A small model receives the result content, metadata, available endpoints, and the ALERTS.md policy.
   - It decides which endpoints to deliver to: notification channels, inbox, interactive endpoints, or nothing.

### ALERTS.md

`ALERTS.md` lives at `~/.residuum/workspace/ALERTS.md`. It is the user-editable routing policy that the LLM router reads on every routing decision. Edits take effect immediately without restart.

A default is created at bootstrap:

```markdown
# Routing Policy

Route background task results based on content and urgency.

## Rules
- Security alerts, errors, and failures -> notify channels (ntfy, etc.) + inbox
- Routine findings and informational results -> inbox only
- Webhook-triggered results -> inbox (unless content indicates urgency)
```

The agent can modify ALERTS.md at the user's request using standard file tools.

## Endpoints

The endpoint registry tracks all available I/O endpoints. The `list_endpoints` tool shows what's available.

### Interactive endpoints

Bidirectional channels (WebSocket, Discord, Telegram). The agent can:
- `switch_endpoint` to redirect responses to a different interactive endpoint.
- `send_message` to send a one-off message to any interactive endpoint.

### Notification endpoints

Output-only channels for push delivery. Configured in `config.toml` under external channel settings.

| Type | Description |
|------|-------------|
| `ntfy` | Push notification via ntfy-compatible server. |
| `webhook` | HTTP POST to a configured URL. |
| `macos` | macOS native notification (when running on macOS). |

### Inbox

Input-only. The agent cannot write to inbox. Items arrive from:
- User adds via HTTP API/UI.
- Webhook routing configured to `inbox`.
- LLM notification router decisions.

## Tools

| Tool | Purpose |
|------|---------|
| `list_endpoints` | Show available interactive and notification endpoints. |
| `switch_endpoint` | Redirect subsequent responses to a different interactive endpoint. Auto-clears when the user sends a message. |
| `send_message` | One-off message to any interactive or notification endpoint. Does not change where turn responses go. |
| `subagent_spawn` | Spawn a background subagent. Results route through the notification router. |
| `schedule_action` | Schedule a future action. Results route through the notification router. |

## Gotchas

- Notification channel delivery failures are logged at warn level. They do not retry or block other channels.
- If `send_message` targets an offline endpoint, the agent receives an error via the bus error topic.
- The `inbox` topic is not available as a `send_message` target — inbox is input-only.
- Background results from agent-spawned tasks are injected mid-turn if the agent is active, or start a new turn if idle.
