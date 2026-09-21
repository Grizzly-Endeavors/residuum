# Notifications

The notification system routes results from background tasks (heartbeat pulses, scheduled actions, agent-spawned sub-agents) to appropriate destinations over the pub/sub bus.

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

Bidirectional channels (WebSocket, Discord, Telegram, Microsoft Teams). The agent can:
- `switch_endpoint` to redirect responses to a different interactive endpoint.
- `send_message` to send a one-off message to any interactive endpoint.
- `send_message` with `file_path` to deliver a file attachment. Images render inline, audio gets a native player, other files appear as downloads. Telegram allows up to 50 MB; Discord, WebSocket, and notification-only endpoints cap at 25 MB. File attachments require an interactive endpoint — notification-only endpoints reject them. Microsoft Teams cannot receive files from the agent: the text is delivered with a note giving the file's path.

### Notification endpoints

Output-only channels for push delivery. Configured in `config/channels.toml`.

| Type | Description |
|------|-------------|
| `ntfy` | Push notification via ntfy-compatible server. |
| `webhook` | HTTP POST to a configured URL. |
| `macos` | macOS native notification (when running on macOS). |
| `windows` | Windows Toast notification (when running on Windows). |

On macOS, an urgent result is posted at the `time_sensitive` interruption level so it breaks through Focus modes; everything else uses the channel's configured `default_priority`. Windows Toasts do not vary by urgency — an urgent result reaches the channel, it just arrives at the same Toast priority as any other. Both platforms batch deliveries within a throttle window (default 30s), collapsing to a summary notification past three in a window.

**Note**: The `webhook` external notification channel is separate from the `webhook` inbound channel (which receives messages *into* the agent via `POST /webhook`). They serve opposite directions.

### Inbox

Input-only. Items arrive from the notification router, webhook routing, and the HTTP API/UI.

## Tools

| Tool | Purpose |
|------|---------|
| `list_endpoints` | Show available interactive and notification endpoints. |
| `switch_endpoint` | Redirect subsequent responses to a different interactive endpoint. Auto-clears when the user sends a message. |
| `send_message` | One-off message and/or file attachment to any interactive or notification endpoint. Does not change where turn responses go. File attachments require an interactive endpoint. |

## Sentinels

Pulse prompts instruct the sub-agent on two sentinel strings:

- `HEARTBEAT_OK` — nothing actionable was found. The result is discarded before routing; it never reaches any endpoint.
- `HEARTBEAT_URGENT` — the result needs attention now. It is pushed to every configured notification channel in addition to being filed.

`HEARTBEAT_OK` is honored for pulses only. `HEARTBEAT_URGENT` is honored for any background result that reaches the router. If a pulse summary somehow contains both, it is treated as silent — an agent saying it has nothing to report is taken at its word.

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
