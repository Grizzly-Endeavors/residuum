# Discord

The Discord interface lets the agent chat with its owner in Discord direct messages. Residuum connects out to Discord's gateway with the bot token, so no public endpoint is needed. Server (guild) messages are ignored.

## Who the agent answers

The **owner** is whoever first sends the bot a direct message. Their Discord user ID and DM channel are saved in `discord_state.json` in the workspace, so ownership survives restarts. To hand the bot to someone else, stop Residuum, remove the `owner` entry from that file, and have the new owner DM the bot.

`[discord] respond_to_others` controls everyone else:

- `false` (default): only the owner's messages reach the agent. Anyone else who DMs the bot gets a short reply saying it only takes requests from the owner. Before an owner exists, the reply asks the owner to DM the bot first.
- `true`: anyone who can DM the bot can talk to the agent.

Slash commands (`/stop`, `/reload`, `/inbox`, …) run only for the owner in either mode; anyone else gets a private "Only my owner can run commands." reply.

Every message the agent sees records who sent it, e.g. `[From: bear via discord (direct message)]` — see [Message Senders](memory.md#message-senders).

## Where replies go

- A reply goes to the DM the message came from.
- Proactive output with no originating Discord message — `send_message` to the `discord` endpoint, results routed through `idle_channel = "discord"`, background turns — goes to the owner's DM. Until an owner exists it is dropped with a warning in the log.
- System notices and errors go only to the owner's DM.

Long replies are split into 2000-character messages. A typing indicator shows in the target DM while a turn runs. Files the agent sends are uploaded as Discord attachments.

## Configuration

```toml
[discord]
token = "${RESIDUUM_DISCORD_TOKEN}"   # or secret:discord; RESIDUUM_DISCORD_TOKEN also works on its own
respond_to_others = false
```

Changing any `[discord]` value restarts the Discord connection on reload.

## Code

`src/interfaces/discord/`: `mod.rs` (connection, shared state, reply routing), `handler.rs` (inbound messages and slash commands), `subscriber.rs` (outbound delivery). Owner and conversation state is `src/interfaces/chat_state.rs`, shared with Telegram and Teams.
