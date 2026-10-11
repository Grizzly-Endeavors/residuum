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

A failed or stopped run's summary is always empty (a failed or cancelled turn produces no text output), so its inbox item's body names what happened directly — the failure reason for a failed run, or that the run was stopped for a cancelled one — rather than being blank. A failed pulse or scheduled action also publishes its own owner-facing notice (a toast in the web UI), separate from the routine inbox item, naming the pulse or action and the failure reason.

### Steering it

Because urgency is the session's judgment, you steer it by wording the pulse's prompt, not by editing configuration. A pulse that says "report anything unusual" will escalate more than one that says "summarize today's activity". The pulse prompt tells the session that `HEARTBEAT_URGENT` means "this needs attention before the user would next check in".

The sentinel is deliberately distinctive so that a summary *about* something urgent does not escalate itself — the literal word "urgent" in a report has no effect.

### Getting results in front of the agent

No routing target injects into the agent's message feed. Two mechanisms do that job, and both are declared where the work is defined:

- **Agent-spawned sessions** (`subagent_spawn`, the `learner`) have every turn's outcome relayed automatically to their direct spawner — main, or the session that spawned them.

Everything else, except an `artifact` session's or a conversation-triggered session's results, reaches the agent through the inbox, which it reads with `inbox_list`.

## Error and Degradation Notices

Besides routing background-task results, the system notification channel carries operational notices and errors — published the same way (`NoticeEvent`/`ErrorEvent` on the bus), shown as a toast in the web UI and recorded in its recall history. They are not sent to Discord, Telegram, or Teams, which carry only conversation: agent replies, and a main-agent turn's failure sent back to the chat that started it.

**Turn and session failures** are classified into a plain-language message naming the cause and a next step (an invalid or expired API key, rate limiting, a network problem, a timeout, the conversation exceeding the model's context limit, the configured model being unavailable, or a provider outage) rather than shown as the raw error. The full technical cause chain travels alongside as a separate `details` field: the web UI shows it behind a Details disclosure in the Recent notifications dialog (the rail's Help menu) and in a session's inline status messages, and it's always in the logs. Chat interfaces (Discord, Telegram, Teams) show the plain message only, and only for a turn they started — they never receive `details`. A cause that doesn't match any of the categories above still gets a plain generic message, never the raw error.

**A fallback or recovery on any role with a provider chain** gets its own notice: one when that role's chain fails over to a fallback, naming why and which fallback it switched to, and one when it's back on the primary. This covers the main model, the memory observer, the memory reflector, the subconscious, and each background-session model tier (small/medium/large) — every role a `[main]`/`[observer]`/`[reflector]`/`[subconscious]`/`[background.models]` provider chain can be configured for. Neither fires again while nothing changes — a turn (or, for a background tier, any session spawn) that keeps landing on the same fallback while the primary stays down produces no repeat notice, and a retry within a single provider never surfaces to the user at all (it stays in the logs, at `warn` once when retries start and once when they resolve or exhaust).

**A response cut off by the model's output-token limit** gets a notice naming the limit, and a system note is added to the turn's own transcript so the agent knows its last response was incomplete. There is no automatic continuation — whether to pick up where it left off is left to the agent.

**Startup degradations** — an MCP server that failed to start, a broken skills directory, workspace channels or the scheduled action store that couldn't be loaded, the memory/embedding providers being unavailable, or the main agent's own observations/recent-context/recent-messages snapshot failing to load during construction — are collected while the gateway starts and published as one grouped notice once startup finishes, naming every degradation together, rather than sitting log-only. A config reload that degrades the same way (currently: the memory or embedding provider) gets its own grouped notice.

**A config reload the agent's own write caused** also reaches the agent directly, not just the user's interfaces: when the agent's `write_file`/`edit_file` touches `config.toml`, `providers.toml`, `mcp.json`, `channels.toml`, `a2a.json`, or `HEARTBEAT.yml`, the outcome of the reload that write triggers — success, failure, or degraded — arrives as a system note in the agent's own transcript on its next step, worded the same as whatever the user's interfaces already show. A reload triggered by something else (a manual edit, the web UI's Settings form) is never attributed to the agent's write; see `config.md` for the agent-facing guidance.

## Endpoints

The endpoint registry tracks all available I/O endpoints. The `list_endpoints` tool shows what's available. It is rebuilt whenever `config.toml` or `channels.toml` reloads, so an interface or channel added or removed takes effect for tools and urgent-notification routing without a restart.

### Interactive endpoints

Bidirectional channels (WebSocket, Discord, Telegram, Microsoft Teams). The agent can:
- `switch_endpoint` to send background output (relayed session results, scheduled work, other turns the user didn't start) to a different interactive endpoint. The reply in progress is unaffected, and the user's next message switches output back to wherever they wrote from.
- `send_message` to send a one-off message to any interactive endpoint. On the web it arrives as a `response` frame outside any turn (an empty `reply_to`); the web UI also follows the main agent's turns from every other interface (see [Turn Control](turn-control.md#the-main-conversation-stream)).
- `send_message` with `conversation` to post into a specific DM, group chat, or channel on a chat interface (Discord, Telegram, Teams). `list_conversations` shows the IDs. Without `conversation`, a proactive message on a chat interface goes to the owner's direct message.
- `send_message` with `file_path` to deliver a file attachment. Images render inline, audio gets a native player, other files appear as downloads. The size cap is per platform, matching that platform's own upload limit: 50 MB on Telegram, 20 MB on Discord. The WebSocket endpoint and notification-only endpoints have no size cap. File attachments require an interactive endpoint — notification-only endpoints reject them. Microsoft Teams cannot receive files from the agent: the text is delivered with a note giving the file's path. On the web UI, a file inside the workspace gets a durable link keyed by its workspace-relative path (`/api/agents/{name}/files/workspace?path=...`), which keeps working for as long as the file itself exists rather than expiring after an hour; a file outside the workspace still gets the older kind of link, a random token that expires after an hour. Either way, a link to a file that's since been deleted or moved answers with a plain explanation rather than a bare 404.
- Only the main agent talks to the owner. A session's `send_message` refuses the WebSocket endpoint and the owner's DM on every chat interface — named explicitly as `conversation`, or reached through the no-conversation default — with an error telling it to message `main` instead. Posting to any other conversation or endpoint still works. `switch_endpoint` is main-only regardless.

### Notification endpoints

Output-only channels for push delivery. Configured in `config/channels.toml`.

| Type | Description |
|------|-------------|
| `ntfy` | Push notification via ntfy-compatible server. |
| `webhook` | HTTP POST to a configured URL. |
| `macos` | macOS native notification (when running on macOS). |
| `windows` | Windows Toast notification (when running on Windows). |

On macOS, an urgent result is posted at the `time_sensitive` interruption level so it breaks through Focus modes; everything else uses the channel's configured `default_priority`. On Windows, an urgent result is posted as a Reminder Toast with a Dismiss button, which opens expanded and stays on screen until dismissed; everything else is a normal Toast. Both platforms batch deliveries within a throttle window (default 30s), collapsing to a summary notification past three in a window — the summary's own body always ends with "All of them are in your agent's inbox: inbox/agent in the workspace.", since every result that reaches a native channel was filed to the agent inbox too, so the unsummarized rest are never a dead end. On macOS, a notification's "Open" action opens the web UI at the Files place of the agent you used last, where `inbox/agent/` holds those items; Windows Toasts have no equivalent click action, hence the body text. The web UI's Inbox shows the user inboxes, which these results never go to.

**Note**: The `webhook` external notification channel is separate from the `webhook` inbound channel (which receives messages *into* the agent via `POST /webhook/<agent>/<name>`). They serve opposite directions.

### Inbox

Input-only. Items arrive from the notification router, webhook routing, and the HTTP API/UI.

## Web Push

Web Push delivers notifications to the user's browsers and installed apps through their push services (Google, Apple, Mozilla), whether or not a Residuum window is open. It belongs to the hub, not to an agent: one signing key and one list of devices serve every agent, and the routes that manage them are under `/api/hub/push/` (see [hub-http.md](hub-http.md#web-push-devices)).

In the web UI, Settings → All agents → Notifications turns push on for the browser it runs in (the browser asks for permission, and the subscription is registered under a name), sets that device's name and preferences, sends the test notification, shows how delivery to each device is going, removes other devices, and edits `[push] contact` under More options. Push needs a secure connection (HTTPS, Residuum Cloud, or localhost), and on iPhone and iPad the app added to the Home Screen. The app's service worker shows every push as a notification and sets the app icon's badge to `badge`; a click brings a Residuum window to the notification's `target`, or opens one there.

### Devices

A **device** is one browser or installed app registered for notifications. The web UI registers it with its browser push subscription, a label, and its preferences; registering the same subscription again updates that device instead of adding another. A device has an `id`, its `label`, when it was created, when a notification last reached its push service (`last_success_at`), its most recent failure (`last_failure`: when, the push service's HTTP status or `null`, and a plain-language message), and four preferences, one per event it can be told about:

| Preference | Event | New device |
|------------|-------|------------|
| `inbox_item` | an agent filed an item in the user inbox | on |
| `agent_failed` | an agent failed to start or crashed | on |
| `outbound_unreachable` | a task sent to a remote agent can't reach it | off |
| `reply_while_away` | an agent replied while no Residuum window was open | off |

A message is sent only to devices whose preference for its event is on. The test notification is the exception: it goes to the device that asked, whatever its preferences say.

A browser can rotate a device's subscription on its own (a key expiring, its storage being cleared), which fires `pushsubscriptionchange` on the service worker. The worker subscribes again under the hub's key and sends the new subscription back with the endpoint it replaces, so the hub updates that device in place — same `id`, label and preferences — instead of registering a second one. Any open window of the app is told the endpoint changed, so its own record of this device stays in step. This only runs for a browser that supports the event; one that doesn't still falls back to the ordinary path below: the next delivery attempt against the stale endpoint gets 404 or 410, and the hub prunes the device and raises the notice described under Delivery.

The device list is `hub/push-devices.json`, readable only by its owner because a subscription's address and secret let anyone who holds them send to that device. The API never returns the address or the keys. A file that can't be read is left alone and reported (an error from the routes and a log line) instead of being treated as empty, so a fault never turns into the loss of every registration.

### The signing key

Push services identify the hub by a VAPID key pair (RFC 8292), kept in `hub/push-vapid.key` (readable only by its owner). It is created the first time it is needed and is never regenerated automatically: each subscription is bound to the public key it was created with, so replacing the key would end notifications on every device. A key file that can't be read is reported and left alone; to start over, move it away, and every device then has to turn notifications on again.

Each push carries a short-lived token signed with this key, naming a contact the push service can reach about misbehaving traffic: the hub config's `[push] contact` (a `mailto:` address or an `https:` URL), or `https://github.com/Grizzly-Endeavors/residuum` when it is unset. A contact that is neither is dropped with a notice and the default is used until it is fixed. A changed contact applies to the next push without a restart.

Neither file is in the hub checkpoint allowlist, so a restore never rolls them back, and the file tools refuse every agent write to both.

### What sends a push

Four things send a push, each once by its own rule. A message goes to the devices whose preference for its event is on, except the ones in front of the user (see Presence below).

| Event | Sent when | Title | Body | A click opens | Tag |
|-------|-----------|-------|------|---------------|-----|
| `inbox_item` | an agent files an item with `user_inbox_add` | the item's title | "From <agent>: " and the start of the item's body | the item in the inbox, `/inbox?item=<agent>:<id>` | `inbox:<agent>:<id>` |
| `agent_failed` | an agent enters the `failed` state, whether starting or running | "<agent> couldn't start", or "<agent> stopped unexpectedly" when it was running | a line chosen by the kind of error: its settings need fixing, another agent is using its Teams port, an internal error, or a pointer to Residuum for anything else | the agent's chat, `/agent/<agent>` | `failed:<agent>` |
| `outbound_unreachable` | a task the agent sent to a remote agent has been unreachable past the tracker's 10-minute threshold, once per streak, when the tracker's notice for it goes out | "<agent> can't reach <remote>" | "A task has been waiting since <time>.", the time the streak began in the hub's timezone, with the date when it isn't today | the agent's Activity, `/agent/<agent>/activity` | `outbound:<agent>:<task id>` |
| `reply_while_away` | a main turn the user was part of ends with a reply no client was there to see, once per turn | "<agent> replied" | the reply as a one-line plain preview | the agent's chat, `/agent/<agent>` | `reply:<agent>` (a later reply replaces an earlier one) |

- A body is the item's or reply's Markdown as plain text on one line (links keep their text), cut to 120 characters. An item with no title is pushed as "New inbox item", and one with no body as "From <agent>.".
- `badge` is the total of unread items across every agent's user inbox, counted from disk when the push is made, whether the agent is running or not.
- A failed agent sends the `agent_failed` push alone: the hub files no item in the user inbox for it, so no `inbox_item` push comes beside it. An item that left the active inbox before its push was worded (the user archived it) sends none, and one that can't be read is pushed with the generic title and the sender only, with a warning in the log.
- A reply from a background turn (a teammate's message, a pulse) never sends `reply_while_away`, and neither does a reply a client could see. A client could see it if one had the agent's WebSocket open as the reply went out (a reader who closes the page before the rest of the turn finishes still saw it), or has it open when the turn ends (a page opened mid-turn is shown the turn so far).
- What the triggers make of each event they read, a push started or nothing, is logged: the pushes at `debug`, the events that start none at `trace`.
- A failed agent started again that fails again sends another `agent_failed`, and a task whose streak ended and began again sends another `outbound_unreachable`.
- When no device wants an event, nothing is worded or counted; the push is skipped with a debug log line.

### Presence

A Residuum window tells the hub when it is in front of the user, so the hub doesn't push what the user is already looking at. Over the hub WebSocket (see [hub-http.md](hub-http.md#hub-websocket)), a client sends `{ "type": "presence", "device_id": "<the device's id>", "active": true }` while a window is visible and focused on a device with push turned on, and again every 30 seconds while it stays so. It sends `active: false` when the window hides or loses focus.

- A device is skipped, whatever the event, while it has a report of `active: true` from the last 60 seconds that came from a connection that is still open.
- A report ends early when its connection closes, so a closed tab or a lost network never silences a phone for the rest of the minute, and one that isn't repeated goes stale after 60 seconds.
- Several windows of one browser are one device: it is present while any of them is active.
- Presence is kept in memory only, and never changes what a device is registered for. The test notification ignores it.
- A report with no `device_id` or `active`, or one that isn't JSON, is refused with a warning `notice` on that connection and changes nothing.

### Delivery

A payload is encrypted for the device (RFC 8291, `aes128gcm`) before it leaves the hub, so the push service carries it without being able to read it. The decrypted JSON is `{ v: 1, event, agent, title, body, target, tag, badge }`:

- `event` is one of `inbox_item`, `agent_failed`, `outbound_unreachable`, `reply_while_away`, or `test`.
- `agent` is the agent the notification is about, empty for the test.
- `body` is plain text of at most 120 characters, cut with an ellipsis when longer.
- `target` is the app path a click opens, `tag` makes notifications with the same tag replace each other, and `badge` is the user's total unread inbox count.

Each message also carries a `TTL` and an `Urgency` for the push service: `agent_failed` is urgent and kept for 24 hours; `inbox_item` is normal and kept for 24 hours; `outbound_unreachable` is normal and kept for 6 hours; `reply_while_away` and `test` are normal and kept for 1 hour.

What the push service answers decides what the hub does:

| Answer | The hub |
|--------|---------|
| any 2xx | sets the device's `last_success_at` |
| 404 or 410 | removes the device: the browser revoked the subscription or it expired |
| 429, any 5xx, or no answer | waits 30 seconds and tries once more, then records the outcome |
| anything else | records a failure without retrying |

Recording a failure sets the device's `last_failure` and logs at warn level with the device's label. `last_failure` stays until the next failure replaces it, so compare its `at` with `last_success_at` to tell whether delivery has recovered. A redirect is never followed.

The user is told when a push sent in the background shows that a device has stopped working, as a warning hub notice (a toast in the web UI, and an entry in the team event log): once when the push service says the device is gone and the hub removes it, naming the device and saying to turn notifications on again on it; and once when a device that was working starts failing, with the plain-language reason. Further failures of a device that is already failing add nothing until a success ends the streak. A test notification tells only the person who asked.

Delivery never blocks or fails what triggered it: a background send returns at once and each device's delivery runs on its own. A device removed or re-registered while its retry waits is not retried, and its delivery ends at once instead of holding the wait. In-process, `PushService::subscribe_deliveries` reports each background delivery's steps (a retry scheduled, then delivered, failed, the device gone, or dropped because the device changed), and `deliveries_pending` counts the deliveries still under way. Only an `https:` subscription address is accepted when a device registers.

`POST /api/hub/push/devices/{id}/test` sends the test notification (`event: "test"`, title "Residuum test notification", target `/home`) to one device and waits for the answer. It tries once, without the 30-second retry, since a person is waiting. The answer is `{ delivered, error }`, with `error` in plain words when the push service refused it, and a device the push service no longer knows is removed.

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

Editing `config/channels.toml` through the agent's `write_file`/`edit_file` tools, the Files editor, or `POST /api/agents/{name}/workspace/validate` reports the same problems the loader would skip or ignore: a TOML syntax error (with the parser's line/column), a channel missing a field its type needs (`ntfy` without `url`/`topic`, `webhook` without `url` or with an unsupported `method`), an unrecognized channel type, or a retired option (`default_category`, `default_scenario`) left in place — the retired-option case is a warning since the channel still loads; the others are errors since that channel won't. The save always goes through; a diagnostic names the problem instead of the write being rejected.

A config reload that touches `channels.toml` parses the new file completely before touching anything running — a `channels.toml` that fails to parse leaves every currently-running channel subscriber in place, with a notice naming the parse error, instead of tearing them all down and starting none.
