# Microsoft Teams

The Teams interface lets the agent chat in Microsoft Teams: in a direct message with its owner, and — when added to them — in group chats and team channels. For step-by-step setup see the [Teams setup guide](../guides/teams-setup.md).

## How messages reach Residuum

Teams has no outbound connection mode like Discord's gateway or Telegram's long polling: Microsoft's Bot Connector delivers every message as an HTTPS `POST` of a Bot Framework activity to the bot's public messaging endpoint. Residuum therefore runs a small dedicated listener for Teams on `[teams] port` (default `7701`, bound on the gateway's `bind` address) serving a single route, `POST /api/teams/messages`.

That listener is deliberately separate from the gateway port. The gateway serves the config and secrets API without authentication and is meant to stay on loopback; a public tunnel pointed at the Teams port exposes only the Teams route. Making the port reachable from the internet is up to the user (Tailscale Funnel, a Cloudflare named tunnel, or any reverse proxy with a trusted certificate — Microsoft requires HTTPS on 443 with a publicly trusted certificate).

Every request is authenticated before anything in it is trusted:

- The bearer JWT must verify (RS256) against Microsoft's published Bot Framework signing keys (`login.botframework.com` OpenID metadata, cached for a day and refetched when an unknown key ID appears), from a key endorsed for the `msteams` channel.
- Issuer must be `https://api.botframework.com`, audience must be `[teams] app_id`, the token must be within its validity window (5 minutes of clock skew tolerated), and its `serviceurl` claim must match the activity's `serviceUrl`, so a captured token cannot redirect replies elsewhere.
- The activity must come from the `msteams` channel and from `[teams] tenant_id`.

Failures return `401` (bad token), `403` (other tenant), or `503` when Residuum cannot fetch Microsoft's signing keys — the connector retries a `503` later. Each rejection is logged with the reason. Authenticated activities are acknowledged immediately and handled in order by a single worker, since the connector retries anything not acknowledged within about 15 seconds.

## Who the agent answers

The **owner** is whoever first sends the bot a direct message. With a custom-uploaded app that can only be the person who installed it. The owner's Entra object ID and DM conversation are saved in `teams_state.json` in the workspace, so ownership survives restarts. To hand the bot to someone else, stop Residuum, remove the `owner` entry from that file, and have the new owner DM the bot.

`[teams] respond_to_others` controls everyone else:

- `false` (default): only the owner's messages reach the agent. Anyone else who messages or @mentions the bot gets a short reply saying it only takes requests from the owner. Before an owner exists, the reply asks the owner to DM the bot first.
- `true`: anyone in the tenant who can reach the bot can talk to the agent.

Slash commands (`/stop`, `/reload`, `/inbox`, …) run only for the owner in either mode.

Every message the agent sees records who sent it and where, e.g. `[From: Jane Doe via teams (#builds (Eng Team))]` — see [Message Senders](memory.md#message-senders). The agent's standing orientation tells it to keep the owner's private information out of replies to other people.

## Direct messages, group chats, and channels

In a direct message every message goes to the agent.

In group chats and standard channels the agent acts only on messages that @mention it. The bot's own mention is stripped from the text; mentions of other people are kept as `@Name`.

When the app manifest grants the resource-specific consent permissions `ChatMessage.Read.Chat` and `ChannelMessage.Read.Group`, Teams also delivers the messages that don't mention the bot. Those are held in memory — never written to disk, and lost on restart — up to `[teams] context_messages` per conversation (default 20; `0` disables). When the bot is next @mentioned there, the held messages are handed to the agent as a single background message placed just before the mention:

```
Previous conversation in group chat "Launch prep" since you were last mentioned there. This is background only; the message that mentions you follows.
[14:05] Sam Lee: build is red again
[14:06] Priya: looks like the migration step
```

The buffer for that conversation is emptied when it is delivered, so each message reaches the agent at most once. Files posted without a mention appear in the buffer as `[shared file: name]` and are not downloaded. Private and shared channels never deliver unmentioned messages (a Teams restriction), so mentions there arrive without context. The buffer only covers messages since the bot joined and since Residuum last started; fetching history from Microsoft Graph instead is tracked in issue #150.

## Where replies go

- A reply goes to the conversation the message came from; in a channel it lands in the same thread.
- Proactive output with no originating Teams message — `send_message` to the `teams` endpoint, results routed through `idle_channel = "teams"`, background turns whose last user message came from Teams — goes to the owner's DM.
- System notices and errors go only to the owner's DM, never into a shared conversation.
- If a turn is already running when another Teams message arrives, the new message joins that turn and the answer goes to the conversation that started it.

Long replies are split into chunks of about 20 KB (Teams rejects larger activities). A typing indicator shows in the target conversation while a turn runs.

## Attachments

Files shared in a DM or in a message that @mentions the bot are downloaded into the agent inbox (`inbox/agent/`) and noted in the message, with an inbox item alongside; supported images under the inline limit are also shown to the model. Inline images pasted into a chat are fetched with the bot's token.

The agent cannot send files over Teams: bots can only deliver files through a consent-card upload flow, which this interface does not implement. When the agent sends a file to Teams, the text goes through with a note giving the file's path, and the file stays available in the web UI.

## Configuration

```toml
[teams]
app_id = "11111111-2222-3333-4444-555555555555"   # bot ID from the Teams Developer Portal
tenant_id = "your-directory-tenant-id"
app_password = "secret:teams"                     # client secret; or RESIDUUM_TEAMS_APP_PASSWORD
respond_to_others = false
context_messages = 20
port = 7701
```

`app_id`, `tenant_id`, and `app_password` are required whenever the section is present; a missing one fails config load with a message naming it rather than leaving a bot that silently never answers. Changing any `[teams]` value, or the gateway `bind` it shares, restarts the Teams listener on reload. Replies are sent with a client-credentials token from `login.microsoftonline.com/{tenant_id}`, cached until shortly before it expires.

## State

`teams_state.json` in the workspace root holds the owner and a reference (conversation ID, service URL, kind, label) for every conversation the bot has seen or been added to. References are what make proactive messages possible — Teams gives a bot no way to open a conversation it has never heard from. Removing the bot from a conversation, or uninstalling the app there, drops its reference.

## Code

`src/interfaces/teams/`: `mod.rs` (listener, worker, reply routing), `auth.rs` (token validation), `handler.rs` (activity handling), `connector.rs` (outbound calls), `context_buffer.rs`, `store.rs`, `subscriber.rs`, `activity.rs` (wire types).
