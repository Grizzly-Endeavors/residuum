# Notifications

The notification system routes results from background tasks (heartbeat pulses, scheduled actions, agent-spawned sessions) to appropriate destinations over the pub/sub bus.

## Routing

Routing is a match on the disposition the producing agent declared. There is no classifier and no policy file — the agent that ran the task states what should happen with its own result, because it is the only participant that has the full transcript and the task's intent.

| Agent's summary contains | Disposition | Delivered to |
|---|---|---|
| `HEARTBEAT_OK` (pulses only) | `Silent` | nowhere; logged at `trace` |
| `HEARTBEAT_URGENT` | `Urgent` | the inbox **and** every channel in `config/channels.toml` |
| neither | `Normal` | the inbox |

Results from agent-spawned (`spawned`) sessions never reach this router: every turn's outcome — completed, failed, cancelled, or panicked — is relayed directly to the session's **direct spawner** (main, or whichever session spawned it) through the agent-messaging system, tagged with the session's address and carrying the normal hop-count rules — see [background-tasks.md](background-tasks.md#hop-counts). The agent that asked for the work gets the answer, not necessarily main, and is never left simply not knowing what happened to a turn it's waiting on.

Results from `artifact` sessions (started by a workbench artifact) are discarded by this router whatever their disposition, `HEARTBEAT_URGENT` included, and are never relayed to main either. Their output belongs to the artifact that started them, which reads it from the session's own stream — see [background-tasks.md](background-tasks.md#artifact-sessions). A session like that files an inbox item itself (`user_inbox_add`) when its task calls for one.

Results from conversation-triggered sessions (A2A callers, and non-owner Discord/Telegram/Teams chats) are likewise discarded by this router whatever their disposition, `HEARTBEAT_URGENT` included. The session's output already went back to the conversation it came from, and its observations are merged into memory as an episode, so filing it to the inbox or a channel would duplicate content the user already saw.

An urgent result with no notification channels configured still reaches the inbox. Nothing is ever dropped for want of a push channel.

A failed or stopped run's summary is always empty (a failed or cancelled turn produces no text output), so its inbox item's body names what happened directly — the failure reason for a failed run, or that the run was stopped for a cancelled one — rather than being blank. A failed pulse or scheduled action also publishes its own owner-facing notice (a toast in the web UI, and the same message on any chat interface), separate from the routine inbox item, naming the pulse or action and the failure reason.

### Steering it

Because urgency is the session's judgment, you steer it by wording the pulse's prompt, not by editing configuration. A pulse that says "report anything unusual" will escalate more than one that says "summarize today's activity". The pulse prompt tells the session that `HEARTBEAT_URGENT` means "this needs attention before the user would next check in".

The sentinel is deliberately distinctive so that a summary *about* something urgent does not escalate itself — the literal word "urgent" in a report has no effect.

### Getting results in front of the agent

No routing target injects into the agent's message feed. Two mechanisms do that job, and both are declared where the work is defined:

- **Agent-spawned sessions** (`subagent_spawn`, the `learner`) have every turn's outcome relayed automatically to their direct spawner — main, or the session that spawned them.

Everything else, except an `artifact` session's or a conversation-triggered session's results, reaches the agent through the inbox, which it reads with `inbox_list`.

## Error and Degradation Notices

Besides routing background-task results, the system notification channel carries operational notices and errors — published the same way (`NoticeEvent`/`ErrorEvent` on the bus), shown as a toast in the web UI and recorded in its recall history, and DMed to the owner on Discord, Telegram, and Teams.

**Turn and session failures** are classified into a plain-language message naming the cause and a next step (an invalid or expired API key, rate limiting, a network problem, a timeout, the conversation exceeding the model's context limit, the configured model being unavailable, or a provider outage) rather than shown as the raw error. The full technical cause chain travels alongside as a separate `details` field: the web UI shows it behind an expandable "details" toggle in the notification corner's recall list and in a session's inline status messages, and it's always in the logs. Chat interfaces (Discord, Telegram, Teams) show the plain message only — they never receive `details`. A cause that doesn't match any of the categories above still gets a plain generic message, never the raw error.

**A fallback or recovery on the main model** gets its own notice: one when the main model's provider chain fails over to a fallback, naming why and which fallback it switched to, and one when it's back on the primary. Neither fires again while nothing changes — a turn that keeps landing on the same fallback while the primary stays down produces no repeat notice, and a retry within a single provider never surfaces to the user at all (it stays in the logs, at `warn` once when retries start and once when they resolve or exhaust).

**A response cut off by the model's output-token limit** gets a notice naming the limit, and a system note is added to the turn's own transcript so the agent knows its last response was incomplete. There is no automatic continuation — whether to pick up where it left off is left to the agent.

**Startup degradations** — an MCP server that failed to start, a broken skills directory, workspace channels or the scheduled action store that couldn't be loaded, the memory/embedding providers being unavailable — are collected while the gateway starts and published as one grouped notice once startup finishes, naming every degradation together, rather than sitting log-only. A config reload that degrades the same way (currently: the memory or embedding provider) gets its own grouped notice.

## Endpoints

The endpoint registry tracks all available I/O endpoints. The `list_endpoints` tool shows what's available. It is rebuilt whenever `config.toml` or `channels.toml` reloads, so an interface or channel added or removed takes effect for tools and urgent-notification routing without a restart.

### Interactive endpoints

Bidirectional channels (WebSocket, Discord, Telegram, Microsoft Teams). The agent can:
- `switch_endpoint` to send background output (relayed session results, scheduled work, other turns the user didn't start) to a different interactive endpoint. The reply in progress is unaffected, and the user's next message switches output back to wherever they wrote from.
- `send_message` to send a one-off message to any interactive endpoint.
- `send_message` with `conversation` to post into a specific DM, group chat, or channel on a chat interface (Discord, Telegram, Teams). `list_conversations` shows the IDs. Without `conversation`, a proactive message on a chat interface goes to the owner's direct message.
- `send_message` with `file_path` to deliver a file attachment. Images render inline, audio gets a native player, other files appear as downloads. Telegram allows up to 50 MB; Discord, WebSocket, and notification-only endpoints cap at 25 MB. File attachments require an interactive endpoint — notification-only endpoints reject them. Microsoft Teams cannot receive files from the agent: the text is delivered with a note giving the file's path. On the web UI, a file inside the workspace gets a durable link keyed by its workspace-relative path (`/api/files/workspace?path=...`), which keeps working for as long as the file itself exists rather than expiring after an hour; a file outside the workspace still gets the older kind of link, a random token that expires after an hour. Either way, a link to a file that's since been deleted or moved answers with a plain explanation rather than a bare 404.
- Only the main agent talks to the owner. A session's `send_message` refuses the WebSocket endpoint and the owner's DM on every chat interface — named explicitly as `conversation`, or reached through the no-conversation default — with an error telling it to message `main` instead. Posting to any other conversation or endpoint still works. `switch_endpoint` is main-only regardless.

### Notification endpoints

Output-only channels for push delivery. Configured in `config/channels.toml`.

| Type | Description |
|------|-------------|
| `ntfy` | Push notification via ntfy-compatible server. |
| `webhook` | HTTP POST to a configured URL. |
| `macos` | macOS native notification (when running on macOS). |
| `windows` | Windows Toast notification (when running on Windows). |

On macOS, an urgent result is posted at the `time_sensitive` interruption level so it breaks through Focus modes; everything else uses the channel's configured `default_priority`. On Windows, an urgent result is posted as a Reminder Toast with a Dismiss button, which opens expanded and stays on screen until dismissed; everything else is a normal Toast. Both platforms batch deliveries within a throttle window (default 30s), collapsing to a summary notification past three in a window — the summary's own body always ends with "See all in your inbox.", since every notification that reaches a native channel was filed to the inbox too, so the unsummarized rest are never a dead end. On macOS, the summary notification's "Open" action also deep-links into the web UI, opening the inbox to the full list; Windows Toasts have no equivalent click action, hence the body text.

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

Editing `config/channels.toml` through the agent's `write_file`/`edit_file` tools, the workspace editor, or `POST /api/workspace/validate` reports the same problems the loader would skip or ignore: a TOML syntax error (with the parser's line/column), a channel missing a field its type needs (`ntfy` without `url`/`topic`, `webhook` without `url` or with an unsupported `method`), an unrecognized channel type, or a retired option (`default_category`, `default_scenario`) left in place — the retired-option case is a warning since the channel still loads; the others are errors since that channel won't. The save always goes through; a diagnostic names the problem instead of the write being rejected.

A config reload that touches `channels.toml` parses the new file completely before touching anything running — a `channels.toml` that fails to parse leaves every currently-running channel subscriber in place, with a notice naming the parse error, instead of tearing them all down and starting none.
