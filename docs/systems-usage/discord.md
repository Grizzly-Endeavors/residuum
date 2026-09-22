# Discord

The Discord interface lets the agent chat in Discord direct messages and in the channels of servers the bot has been added to. Residuum connects out to Discord's gateway with the bot token, so no public endpoint is needed.

## Setup

Create an application and bot at [discord.com/developers](https://discord.com/developers/applications). On the bot page, turn on the **Message Content** privileged intent: the bot requests it, and Discord refuses the connection without it. Put the bot token in `[discord] token`.

To use the bot in a server, invite it with the `bot` and `applications.commands` scopes and at least the View Channels, Send Messages, Read Message History, and Attach Files permissions. Direct messages need no invite: DM the bot from any server you share with it.

## Who the agent answers

The **owner** is whoever first sends the bot a direct message. Their Discord user ID and DM channel are saved in `discord_state.json` in the workspace, so ownership survives restarts. To hand the bot to someone else, stop Residuum, remove the `owner` entry from that file, and have the new owner DM the bot.

`[discord] respond_to_others` controls everyone else:

- `false` (default): only the owner's messages reach the agent. Anyone else who DMs or @mentions the bot gets a short reply saying it only takes requests from the owner. Before an owner exists, the reply asks the owner to DM the bot first.
- `true`: anyone who can DM or @mention the bot can talk to the agent.

Slash commands (`/stop`, `/reload`, `/inbox`, …) run only for the owner in either mode; anyone else gets a private "Only my owner can run commands." reply. In a server, command output is visible only to the owner.

Every message the agent sees records who sent it and where, e.g. `[From: bear via discord (#builds (Eng Team))]` — see [Message Senders](memory.md#message-senders).

## Direct messages and server channels

In a direct message every message goes to the agent.

In a server channel or thread the agent acts only on messages that @mention the bot (a reply that pings the bot counts). The bot's own mention is stripped from the text; other mentions are kept.

Messages that don't mention the bot are held in memory — never written to disk, and lost on restart — up to `[discord] context_messages` per channel (default 20; `0` disables). When the bot is next @mentioned there, the held messages are handed to the agent as a single background message placed just before the mention:

```
Previous conversation in #builds (Eng Team) since you were last mentioned there. This is background only; the message that mentions you follows.
[14:05] Sam: build is red again
[14:06] Priya: looks like the migration step
```

The buffer for that channel is emptied when it is delivered, so each message reaches the agent at most once.

## Where replies go

- A reply goes to the DM or channel the message came from.
- `send_message` with a `conversation` from `list_conversations` posts into that DM or channel. If Discord refuses the post (the bot lacks permission there, or was removed), the owner gets an error DM saying so.
- Other proactive output — `send_message` without a conversation, results routed through `idle_channel = "discord"`, background turns — goes to the owner's DM. Until an owner exists it is dropped with a warning in the log.
- System notices and errors go only to the owner's DM, never into a server channel.

Long replies are split into 2000-character messages. A typing indicator shows in the target channel while a turn runs. Files the agent sends are uploaded as Discord attachments.

## Conversations the agent can list

`list_conversations` shows, for Discord:

- every DM the bot has had, labelled with the person (`direct message with bear`), from `discord_state.json`;
- every text and announcement channel in every server the bot is in (`#builds (Eng Team)`), fetched from the Discord API on each call. Active threads are not listed, but a thread the bot was mentioned in can be replied to.

## Configuration

```toml
[discord]
token = "${RESIDUUM_DISCORD_TOKEN}"   # or secret:discord; RESIDUUM_DISCORD_TOKEN also works on its own
respond_to_others = false
context_messages = 20
```

Changing any `[discord]` value restarts the Discord connection on reload.

## Code

`src/interfaces/discord/`: `mod.rs` (connection, shared state, reply routing), `handler.rs` (inbound messages and slash commands), `channels.rs` (server channel labels, mention handling, conversation listing), `subscriber.rs` (outbound delivery). Owner and conversation state is `src/interfaces/chat_state.rs`, shared with Telegram and Teams; the directory `list_conversations` reads is `src/interfaces/conversations.rs`. The unmentioned-message buffer is `src/interfaces/context_buffer.rs`, shared with Telegram and Teams.
