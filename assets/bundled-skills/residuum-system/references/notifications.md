# Notifications

The notification system routes background task results via a pub/sub bus. A notification router subscribes to the `background:result` topic and delivers each result according to the disposition its producing agent declared.

## Routing

Routing is a match on the disposition the producing agent declared. There is no classifier and no policy file — the agent that ran the task states what should happen with its own result, because it is the only participant that has the full transcript and the task's intent.

| Agent's summary contains | Disposition | Delivered to |
|---|---|---|
| `HEARTBEAT_OK` (pulses only) | `Silent` | nowhere; logged at `trace` |
| `HEARTBEAT_URGENT` | `Urgent` | the inbox **and** every channel in `config/channels.toml` |
| neither | `Normal` | the inbox |

Results from agent-spawned sub-agents are relayed back to the main agent instead, whatever their disposition — the agent that asked for the work gets the answer.

An urgent result with no notification channels configured still reaches the inbox. Nothing is ever dropped for want of a push channel.

### Steering it

Because urgency is the sub-agent's judgment, you steer it by wording the pulse's prompt, not by editing configuration. A pulse that says "report anything unusual" will escalate more than one that says "summarize today's activity". The pulse prompt tells the sub-agent that `HEARTBEAT_URGENT` means "this needs attention before the user would next check in".

The sentinel is deliberately distinctive so that a summary *about* something urgent does not escalate itself — the literal word "urgent" in a report has no effect.

### Getting results in front of the agent

No routing target injects into the agent's message feed. Two mechanisms do that job, and both are declared where the work is defined:

- **`agent: main` on a pulse** (in `HEARTBEAT.yml`) runs the pulse as a wake turn: the prompt is injected into the agent's context as a system message. This bypasses the router entirely.
- **Agent-spawned sub-agents** have their results relayed back to the main agent automatically.

Everything else reaches the agent through the inbox, which it reads with `inbox_list`.

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
| `windows` | Windows Toast notification (when running on Windows). |

On macOS an urgent result posts at the `time_sensitive` interruption level so it breaks through Focus modes. Windows Toasts do not vary by urgency.

### Inbox

Input-only. The agent cannot write to inbox. Items arrive from:
- User adds via HTTP API/UI.
- Webhook routing configured to `inbox`.
- Notification router deliveries.

## Tools

| Tool | Purpose |
|------|---------|
| `list_endpoints` | Show available interactive and notification endpoints. |
| `switch_endpoint` | Redirect subsequent responses to a different interactive endpoint. Auto-clears when the user sends a message. |
| `send_message` | One-off message to any interactive or notification endpoint. Does not change where turn responses go. |
| `subagent_spawn` | Spawn a background subagent. The result is relayed back to you. |
| `schedule_action` | Schedule a future action. The result is filed to the inbox, or pushed too if marked urgent. |

## Gotchas

- Notification channel delivery failures are logged at warn level. They do not retry or block other channels.
- If `send_message` targets an offline endpoint, the agent receives an error via the bus error topic.
- The `inbox` topic is not available as a `send_message` target — inbox is input-only.
- Background results from agent-spawned tasks are injected mid-turn if the agent is active, or start a new turn if idle.
