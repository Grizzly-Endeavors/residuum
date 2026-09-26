# Notifications

The notification system routes background task results via a pub/sub bus. A notification router subscribes to the `background:result` topic and delivers each result according to the disposition its producing agent declared.

## Routing

Routing is a match on the disposition the producing agent declared. There is no classifier and no policy file — the agent that ran the task states what should happen with its own result, because it is the only participant that has the full transcript and the task's intent.

| Agent's summary contains | Disposition | Delivered to |
|---|---|---|
| `HEARTBEAT_OK` (pulses only) | `Silent` | nowhere; logged at `trace` |
| `HEARTBEAT_URGENT` | `Urgent` | the inbox **and** every channel in `config/channels.toml` |
| neither | `Normal` | the inbox |

Results from agent-spawned (`spawned`) sessions never reach this router: every turn's outcome — completed, failed, cancelled, or panicked — is relayed directly to the session's **direct spawner** (main, or whichever session spawned it) through the agent-messaging system, tagged with the session's address and carrying the normal hop-count rules — see [background-tasks.md](background-tasks.md#messaging). The agent that asked for the work gets the answer, not necessarily main, and is never left simply not knowing what happened to a turn it's waiting on.

Results from `artifact` sessions (started by a workbench artifact) are discarded by this router whatever their disposition, `HEARTBEAT_URGENT` included, and are never relayed to main either. Their output belongs to the artifact that started them, which reads it from the session's own stream — see [background-tasks.md](background-tasks.md#artifact-sessions). A session like that files an inbox item itself (`user_inbox_add`) when its task calls for one.

Results from conversation-triggered sessions (A2A callers, and non-owner Discord/Telegram/Teams chats) are likewise discarded by this router whatever their disposition, `HEARTBEAT_URGENT` included. The session's output already went back to the conversation it came from, and its observations are merged into memory as an episode, so filing it to the inbox or a channel would duplicate content the user already saw.

An urgent result with no notification channels configured still reaches the inbox. Nothing is ever dropped for want of a push channel.

A failed or stopped run's summary is always empty, so its inbox item's body names what happened directly (the failure reason, or that the run was stopped) instead of being blank. A failed pulse or scheduled action also publishes its own owner-facing notice, separate from the inbox item.

### Steering it

Because urgency is the session's judgment, you steer it by wording the pulse's prompt, not by editing configuration. A pulse that says "report anything unusual" will escalate more than one that says "summarize today's activity". The pulse prompt tells the session that `HEARTBEAT_URGENT` means "this needs attention before the user would next check in".

The sentinel is deliberately distinctive so that a summary *about* something urgent does not escalate itself — the literal word "urgent" in a report has no effect.

### Getting results in front of the agent

No routing target injects into the agent's message feed. Two mechanisms do that job, and both are declared where the work is defined:

- **Agent-spawned sessions** (`subagent_spawn`, the `learner`) have every turn's outcome relayed automatically to their direct spawner — main, or the session that spawned them.

Everything else, except an `artifact` session's or a conversation-triggered session's results, reaches the agent through the inbox, which it reads with `inbox_list`.

## Error and Degradation Notices

Turn and session failures are classified into a plain-language message with a next step (bad API key, rate limiting, network problem, timeout, context limit exceeded, model unavailable, provider outage) rather than shown raw. The full technical chain travels alongside as a separate `details` field, shown in the web UI behind an expandable toggle and always in the logs — chat interfaces get the plain message only.

A fallback or recovery on the main model gets one notice per transition (failing over, and coming back), never per call or per retry. A response cut off by the output-token limit gets a notice naming the limit, plus a system note in the turn's own transcript — no automatic continuation. Startup degradations (an MCP server failing, a broken skills directory, channels or the action store failing to load, memory/embedding providers unavailable) are collected and published as one grouped notice once startup finishes, instead of sitting log-only.

## Endpoints

The endpoint registry tracks all available I/O endpoints. The `list_endpoints` tool shows what's available.

### Interactive endpoints

Bidirectional channels (WebSocket, Discord, Telegram, Microsoft Teams). The agent can:
- `switch_endpoint` to send background output (relayed session results, scheduled work, other turns the user didn't start) to a different interactive endpoint. The reply in progress is unaffected, and the user's next message switches output back to wherever they wrote from.
- `send_message` to send a one-off message to any interactive endpoint.
- `send_message` with `conversation` to post into a specific DM, group chat, or channel on a chat interface (Discord, Telegram, Teams). `list_conversations` shows the IDs. Without `conversation`, a proactive message on a chat interface goes to the owner's direct message.

### Notification endpoints

Output-only channels for push delivery. Configured in `config/channels.toml`.

| Type | Description |
|------|-------------|
| `ntfy` | Push notification via ntfy-compatible server. |
| `webhook` | HTTP POST to a configured URL. |
| `macos` | macOS native notification (when running on macOS). |
| `windows` | Windows Toast notification (when running on Windows). |

On macOS an urgent result posts at the `time_sensitive` interruption level so it breaks through Focus modes. Windows Toasts do not vary by urgency.

Editing `config/channels.toml` via `write_file`/`edit_file`, the workspace editor, or `POST /api/workspace/validate` reports a TOML syntax error, a channel missing a required field, an unrecognized channel type, or a retired option left in place as a diagnostic alongside the save — the write always goes through rather than being rejected.

### Inbox

Input-only. The agent cannot write to inbox. Items arrive from:
- User adds via HTTP API/UI.
- Webhook routing configured to `inbox`.
- Notification router deliveries.

## Tools

| Tool | Purpose |
|------|---------|
| `list_endpoints` | Show available interactive and notification endpoints. |
| `list_conversations` | Show the DMs, group chats, and channels each running chat interface can post into, with the IDs `send_message` takes as `conversation`. |
| `switch_endpoint` | Send background output to a different interactive endpoint. Auto-clears when the user sends a message. |
| `send_message` | One-off message to any interactive or notification endpoint, optionally to a specific `conversation` on a chat interface. Does not change where turn responses go. |
| `subagent_spawn` | Spawn a background subagent. The result is relayed back to you. |
| `schedule_action` | Schedule a future action. The result is filed to the inbox, or pushed too if marked urgent. |

## Gotchas

- Notification channel delivery failures are logged at warn level. They do not retry or block other channels.
- If `send_message` targets an offline endpoint, the agent receives an error via the bus error topic.
- The `inbox` topic is not available as a `send_message` target — inbox is input-only.
- A `conversation` is checked before sending: an ID `list_conversations` doesn't show is rejected. If the platform then refuses the post (the bot was removed, lacks permission), the owner gets an error message on that interface.
- Background results from agent-spawned tasks are injected mid-turn if the agent is active, or start a new turn if idle.
- Only the main agent talks to the owner. A session's `send_message` refuses the WebSocket endpoint and the owner's DM on every chat interface (named explicitly, or reached through the no-conversation default) — it must message `main` instead. `switch_endpoint` is main-only too.
- A session's `subagent_spawn` records the session as the spawner and its depth plus one on the new session; spawning past the configured `subagent_depth_cap` (default 2) is refused.
