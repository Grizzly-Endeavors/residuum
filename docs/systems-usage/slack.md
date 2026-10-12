# Slack

The Slack interface lets the agent chat in Slack direct messages. It connects out to Slack over a Socket Mode websocket, so no public address, tunnel, or inbound port is needed — it works from a machine behind a firewall. Setting up the Slack app itself is a guide: [Set up Residuum in Slack](../guides/slack-setup.md).

DMs are the whole surface in this version: the app does not join channels or group chats, and threads, media, and reactions are v1-unsupported. One Slack app serves one agent.

## Startup and recovery

Opening the Socket Mode connection (with the app token) happens at startup; a transient failure there retries with exponential backoff instead of leaving the adapter dead until a config reload. A dropped connection — Slack-side disconnect or a hub restart on the other end of the websocket — reconnects the same way, and liveness is kept by an app-level ping every 30 s with a read-silence timeout of 90 s, so a silently dead socket is noticed and replaced rather than waited on forever. A corrupt `slack_state.json` is moved aside and the interface starts fresh with a notice, rather than staying down until someone fixes the file by hand — the owner will need to message the app again to be recognized.

## Who the agent answers

The **owner** is whoever first sends the app a direct message. Their Slack user ID and DM channel are saved in `slack_state.json` in the workspace, so ownership survives restarts. To hand the app to someone else, stop Residuum, remove the `owner` entry from that file, and have the new owner message the app.

`[slack] respond_to_others` controls everyone else:

- `false` (default): only the owner's messages reach the agent. Anyone else who DMs the app is **silently ignored** — unlike Telegram, no refusal reply is sent; each ignored message is recorded in the log (`slack message from someone other than the owner; respond_to_others is off`).
- `true`: anyone who can DM the app can talk to the agent.

## Where replies go

- A reply goes back to the DM that started the turn, from the agent's bot user.
- While a Slack-started turn runs, the DM shows a “Thinking…” message: Slack gives bots no typing signal, so one is posted when the turn starts and edited into the turn's first outgoing text — reply or failure — when it arrives. Turns started elsewhere never post one. A turn that ends without sending any text (an empty reply, or one the agent addressed elsewhere) leaves “_Turn ended without a reply._” in its place instead of a stale “Thinking…”.
- Other proactive output — `send_message` without a conversation, results routed through `idle_channel = "slack"` — goes to the owner's DM. Until an owner exists it is dropped with a warning in the log.
- When a main-agent turn started from Slack fails, the plain-language error goes back where its reply would have. System notices and other errors are never sent to Slack; they appear in the web UI.
- Long replies are split into 4,000-character messages, and the agent's markdown is converted to Slack's mrkdwn on the way out (code fences survive the split). Posts are rate-limited to Slack's roughly-one-per-second Web API budget, with backoff on 429s.

## Conversations the agent can list

`list_conversations` shows what `slack_state.json` has recorded: every DM channel that has messaged the app (`direct message with …`, the workspace member's user ID).

## Configuration

```toml
[slack]
bot_token = "secret:slack_bot_token"   # xoxb-… — posts messages as the bot user
app_token = "secret:slack_app_token"   # xapp-… — opens the Socket Mode connection
respond_to_others = false
```

Both tokens are required (each also accepts a `"${ENV_VAR}"` reference or a literal); a `[slack]` section with either missing resolves to no Slack adapter, with a warning in the log. Changing any `[slack]` value restarts the Slack adapter on reload — the old socket is drained and closed and a new one opens — and removing the section stops the adapter.

## Code

`src/interfaces/slack/`: `mod.rs` (shared state, reply routing, conversation listing), `handler.rs` (inbound DM events, owner binding, dedup, bus dispatch), `socket.rs` (Socket Mode client — connect, ack-first, reconnect, liveness), `subscriber.rs` (outbound delivery), `api.rs` (Web API client, rate-limited sender), `format.rs` (markdown → mrkdwn), `events.rs` (envelope and DM event types). Owner and conversation state is `src/interfaces/chat_state.rs`, shared with Discord, Teams, and Telegram.
