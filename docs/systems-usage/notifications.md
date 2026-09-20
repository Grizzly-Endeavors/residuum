# Notifications

The notification system routes results from background tasks (heartbeat pulses, scheduled actions, agent-spawned sub-agents) to appropriate destinations. Routing is handled by a pub/sub bus with a dedicated LLM-based notification router.

## Routing Architecture

### Two-layer routing

When a background task completes:

1. **Layer 1 — Programmatic rules** (no LLM call):
   - `HEARTBEAT_OK` results from pulses are silently discarded (logged at `trace` only).
   - Results from agent-spawned tasks (`EventTrigger::Agent`) are relayed back to the main agent as a message on the `user:message` topic.

2. **Layer 2 — LLM router** (everything not handled by Layer 1):
   - A small-tier model receives the result content, metadata, the available target list, and the ALERTS.md policy.
   - It returns a list of delivery targets.

### Router targets

The router's target list is exactly `inbox` plus every endpoint registered with `NOTIFY_ONLY` capability — that is, the channels defined in `config/channels.toml`. Any target the model returns that is not on that list is discarded during validation, and if nothing valid remains the result falls back to `inbox`.

The router cannot deliver to interactive endpoints (WebSocket, Discord, Telegram). Those are reachable only through the agent's `send_message` tool, or by being the agent's current output endpoint.

### Failure behavior

If the small-tier provider cannot be built at startup, the router runs in a fallback mode that applies the Layer 1 rules and sends everything else to `inbox`. If an individual routing call fails or returns unparseable output, that result goes to `inbox`.

### ALERTS.md

`ALERTS.md` is the user-editable routing policy that the LLM router reads on every routing decision. Edits take effect immediately without restart. The agent can modify it at the user's request using standard file tools.

## Getting results in front of the agent

There is no notification target that injects into the agent's message feed. Two mechanisms do that job instead, and both are declared where the work is defined rather than chosen by the router:

- **`agent: main` on a pulse** (in `HEARTBEAT.yml`) runs the pulse as a wake turn: the prompt is injected into the agent's context as a system message. This bypasses the router entirely.
- **Agent-spawned sub-agents** have their results relayed back to the main agent automatically by Layer 1.

Everything else reaches the agent through the inbox, which the agent reads with `inbox_list`.

## Endpoints

The endpoint registry tracks all available I/O endpoints. The `list_endpoints` tool shows what's available.

### Interactive endpoints

Bidirectional channels (WebSocket, Discord, Telegram). The agent can:
- `switch_endpoint` to redirect responses to a different interactive endpoint.
- `send_message` to send a one-off message to any interactive endpoint.
- `send_message` with `file_path` to deliver a file attachment. Images render inline, audio gets a native player, other files appear as downloads. Telegram allows up to 50 MB; Discord, WebSocket, and notification-only endpoints cap at 25 MB. File attachments require an interactive endpoint — notification-only endpoints reject them.

### Notification endpoints

Output-only channels for push delivery. Configured in `config/channels.toml`.

| Type | Description |
|------|-------------|
| `ntfy` | Push notification via ntfy-compatible server. |
| `webhook` | HTTP POST to a configured URL. |
| `macos` | macOS native notification (when running on macOS). |
| `windows` | Windows Toast notification (when running on Windows). |

**Note**: The `webhook` external notification channel is separate from the `webhook` inbound channel (which receives messages *into* the agent via `POST /webhook`). They serve opposite directions.

### Inbox

Input-only. Items arrive from the LLM notification router, webhook routing, and the HTTP API/UI.

## Built-in Routing Targets

| Target | Behavior |
|--------|----------|
| `inbox` | Creates an inbox item with the task result as body and task name as source. Never enters the message feed. |

Every other valid target is a notification channel named in `config/channels.toml`.

## Tools

| Tool | Purpose |
|------|---------|
| `list_endpoints` | Show available interactive and notification endpoints. |
| `switch_endpoint` | Redirect subsequent responses to a different interactive endpoint. Auto-clears when the user sends a message. |
| `send_message` | One-off message and/or file attachment to any interactive or notification endpoint. Does not change where turn responses go. File attachments require an interactive endpoint. |

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
