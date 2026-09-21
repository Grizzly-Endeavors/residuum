# Telegram

The Telegram interface lets the agent chat in Telegram private chats and in groups the bot has been added to. Residuum long-polls the Bot API with the bot token, so no public endpoint is needed.

## Who the agent answers

The **owner** is whoever first sends the bot a private message. Their Telegram user ID and chat are saved in `telegram_state.json` in the workspace, so ownership survives restarts. To hand the bot to someone else, stop Residuum, remove the `owner` entry from that file, and have the new owner message the bot.

`[telegram] respond_to_others` controls everyone else:

- `false` (default): only the owner's messages reach the agent. Anyone else who messages or addresses the bot gets a short reply saying it only takes requests from the owner. Before an owner exists, the reply asks the owner to message the bot first.
- `true`: anyone who can reach the bot can talk to the agent.

Commands (`/stop`, `/reload`, `/inbox`, …) run only for the owner in either mode.

Every message the agent sees records who sent it and where, e.g. `[From: Bear Flinn via telegram (group "Launch prep")]` — see [Message Senders](memory.md#message-senders).

## Private chats and groups

In a private chat every message goes to the agent.

In a group or supergroup the agent acts only on messages addressed to the bot:

- a message that mentions `@botusername` (the mention is stripped from the text);
- a reply to one of the bot's messages;
- a command, unless it names a different bot (`/status@otherbot`).

With Telegram's default privacy mode these are also the only group messages the bot receives, so everything else in the group stays invisible to Residuum. Photo, document, and video captions count as the message text. Channels (broadcast-only) are not supported.

## Where replies go

- A reply goes to the chat the message came from.
- `send_message` with a `conversation` from `list_conversations` posts into that private chat or group. If Telegram refuses the post (the bot was removed, or can't write there), the owner gets an error message saying so.
- Other proactive output — `send_message` without a conversation, results routed through `idle_channel = "telegram"`, background turns — goes to the owner's chat. Until an owner exists it is dropped with a warning in the log.
- System notices and errors go only to the owner's chat, never into a group.

Long replies are split into 4096-character messages. A typing indicator shows while a turn runs. Files the agent sends go out as photos, audio, or documents by type; captions longer than Telegram's 1024-character limit are followed by the full text.

## Conversations the agent can list

Telegram gives bots no way to list their chats, so `list_conversations` shows what `telegram_state.json` has recorded: every private chat that has messaged the bot (`direct message with Bear Flinn`), and every group the bot has been added to or addressed in (`group "Launch prep"`). A group is dropped when the bot is removed from it, and when a group is upgraded to a supergroup its old ID is dropped; the new one appears once the bot is next addressed there.

## Configuration

```toml
[telegram]
token = "${RESIDUUM_TELEGRAM_TOKEN}"   # or secret:telegram; RESIDUUM_TELEGRAM_TOKEN also works on its own
respond_to_others = false
```

Changing any `[telegram]` value restarts the Telegram adapter on reload.

## Code

`src/interfaces/telegram/`: `mod.rs` (shared state, reply routing, conversation listing), `handler.rs` (long polling, inbound messages, commands, group membership), `groups.rs` (whether a group message is addressed to the bot), `subscriber.rs` (outbound delivery). Owner and conversation state is `src/interfaces/chat_state.rs`, shared with Discord and Teams.
