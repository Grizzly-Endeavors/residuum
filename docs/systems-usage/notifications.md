# Notifications

The notification system routes results from background tasks (heartbeat pulses, scheduled actions, agent-spawned sessions) to appropriate destinations over the pub/sub bus.

## Routing

Routing is a match on the disposition the producing agent declared. There is no classifier and no policy file — the agent that ran the task states what should happen with its own result, because it is the only participant that has the full transcript and the task's intent.

| Agent's summary contains | Disposition | Delivered to |
|---|---|---|
| `HEARTBEAT_OK` (pulses only) | `Silent` | nowhere; logged at `trace` |
| `HEARTBEAT_URGENT` | `Urgent` | the inbox **and** every channel in `config/channels.toml` |
| neither | `Normal` | the inbox |

Results from agent-spawned (`spawned`) sessions never reach this router: each turn's result is relayed directly to the session's **direct spawner** (main, or whichever session spawned it) through the agent-messaging system, tagged with the session's address and carrying the normal hop-count rules — see [background-tasks.md](background-tasks.md#hop-counts). The agent that asked for the work gets the answer, not necessarily main.

An urgent result with no notification channels configured still reaches the inbox. Nothing is ever dropped for want of a push channel.

### Steering it

Because urgency is the session's judgment, you steer it by wording the pulse's prompt, not by editing configuration. A pulse that says "report anything unusual" will escalate more than one that says "summarize today's activity". The pulse prompt tells the session that `HEARTBEAT_URGENT` means "this needs attention before the user would next check in".

The sentinel is deliberately distinctive so that a summary *about* something urgent does not escalate itself — the literal word "urgent" in a report has no effect.

### Getting results in front of the agent

No routing target injects into the agent's message feed. Two mechanisms do that job, and both are declared where the work is defined:

- **Agent-spawned sessions** (`subagent_spawn`, the `learner`) have each turn's result relayed automatically to their direct spawner — main, or the session that spawned them.

Everything else reaches the agent through the inbox, which it reads with `inbox_list`.

## Endpoints

The endpoint registry tracks all available I/O endpoints. The `list_endpoints` tool shows what's available. It is rebuilt whenever `config.toml` or `channels.toml` reloads, so an interface or channel added or removed takes effect for tools and urgent-notification routing without a restart.

### Interactive endpoints

Bidirectional channels (WebSocket, Discord, Telegram, Microsoft Teams). The agent can:
- `switch_endpoint` to send background output (relayed session results, scheduled work, other turns the user didn't start) to a different interactive endpoint. The reply in progress is unaffected, and the user's next message switches output back to wherever they wrote from.
- `send_message` to send a one-off message to any interactive endpoint.
- `send_message` with `conversation` to post into a specific DM, group chat, or channel on a chat interface (Discord, Telegram, Teams). `list_conversations` shows the IDs. Without `conversation`, a proactive message on a chat interface goes to the owner's direct message.
- `send_message` with `file_path` to deliver a file attachment. Images render inline, audio gets a native player, other files appear as downloads. Telegram allows up to 50 MB; Discord, WebSocket, and notification-only endpoints cap at 25 MB. File attachments require an interactive endpoint — notification-only endpoints reject them. Microsoft Teams cannot receive files from the agent: the text is delivered with a note giving the file's path.
- Only the main agent talks to the owner. A session's `send_message` refuses the WebSocket endpoint and the owner's DM on every chat interface — named explicitly as `conversation`, or reached through the no-conversation default — with an error telling it to message `main` instead. Posting to any other conversation or endpoint still works. `switch_endpoint` is main-only regardless.

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
| `list_conversations` | Show the DMs, group chats, and channels each running chat interface can post into, with the IDs `send_message` takes as `conversation`. |
| `switch_endpoint` | Send background output to a different interactive endpoint. Auto-clears when the user sends a message. |
| `send_message` | One-off message and/or file attachment to any interactive or notification endpoint, optionally to a specific `conversation` on a chat interface. Does not change where turn responses go. File attachments require an interactive endpoint. From a session, the owner's DM and the WebSocket endpoint are refused — message `main` instead. |

## Sentinels

Pulse prompts instruct the session on two sentinel strings:

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
