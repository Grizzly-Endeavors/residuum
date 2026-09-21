# Telegram

The Telegram interface lets the agent chat with its owner in Telegram private chats. Residuum long-polls the Bot API with the bot token, so no public endpoint is needed. Group chats are ignored.

## Who the agent answers

The **owner** is whoever first sends the bot a private message. Their Telegram user ID and chat are saved in `telegram_state.json` in the workspace, so ownership survives restarts. To hand the bot to someone else, stop Residuum, remove the `owner` entry from that file, and have the new owner message the bot.

`[telegram] respond_to_others` controls everyone else:

- `false` (default): only the owner's messages reach the agent. Anyone else who messages the bot gets a short reply saying it only takes requests from the owner. Before an owner exists, the reply asks the owner to message the bot first.
- `true`: anyone who can find the bot can talk to the agent.

Commands (`/stop`, `/reload`, `/inbox`, …) run only for the owner in either mode.

Every message the agent sees records who sent it, e.g. `[From: Bear Flinn via telegram (direct message)]` — see [Message Senders](memory.md#message-senders).

## Where replies go

- A reply goes to the chat the message came from.
- Proactive output with no originating Telegram message — `send_message` to the `telegram` endpoint, results routed through `idle_channel = "telegram"`, background turns — goes to the owner's chat. Until an owner exists it is dropped with a warning in the log.
- System notices and errors go only to the owner's chat.

Long replies are split into 4096-character messages. A typing indicator shows while a turn runs. Files the agent sends go out as photos, audio, or documents by type; captions longer than Telegram's 1024-character limit are followed by the full text.

## Configuration

```toml
[telegram]
token = "${RESIDUUM_TELEGRAM_TOKEN}"   # or secret:telegram; RESIDUUM_TELEGRAM_TOKEN also works on its own
respond_to_others = false
```

Changing any `[telegram]` value restarts the Telegram adapter on reload.

## Code

`src/interfaces/telegram/`: `mod.rs` (shared state, reply routing), `handler.rs` (long polling, inbound messages, commands), `subscriber.rs` (outbound delivery). Owner and conversation state is `src/interfaces/chat_state.rs`, shared with Discord and Teams.
