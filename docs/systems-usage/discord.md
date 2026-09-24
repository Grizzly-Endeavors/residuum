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

## Conversation routing

Only the owner's own direct messages reach the main agent. Every other admitted conversation — a server channel or thread, and a non-owner's DM when `respond_to_others` is on — is handled by an [agent session](background-tasks.md) of its own instead: a temporary fork of the main agent, addressed deterministically by that channel, that keeps its own memory and idle timeout rather than sharing the owner's private conversation. This holds even when the owner is the one talking in a server channel — a channel is still a shared space, so it gets a session, not main. The session sees the same sender attribution and buffered chatter described below, and replies into that channel — see [Where replies go](#where-replies-go).

`/stop` follows the same routing: typed in the owner's own DM it stops main's current turn; typed in a server channel or thread it stops that channel's session instead, never main — see [Turn Control](turn-control.md).

## Direct messages and server channels

In a direct message every message goes to the agent.

In a server channel or thread the agent acts only on messages that @mention the bot (a reply that pings the bot counts). The bot's own mention is stripped from the text; other mentions are kept.

Messages that don't mention the bot are held in memory — never written to disk, and lost on restart — up to `[discord] context_messages` per channel (default 20; `0` disables), across at most 256 channels at once; past that, the channel that went longest without new chatter is dropped. When the bot is next @mentioned there, the held messages are handed to the agent as a single background message placed just before the mention:

```
Previous conversation in #builds (Eng Team) since you were last mentioned there. This is background only; the message that mentions you follows.
[14:05] Sam: build is red again
[14:06] Priya: looks like the migration step
```

The buffer for that channel is emptied when it is delivered, so each message reaches the agent at most once.

## Where replies go

- A DM reply goes to the owner from the main agent's own conversation.
- A server channel or thread's reply comes from that channel's own session and always goes back to that same channel. It never falls back to the owner's DM: if Discord refuses the post (the bot lacks permission there, or was removed), the output is dropped, the error is logged naming the session and channel, and main gets a notice so the owner can be told if it matters.
- `send_message` with a `conversation` from `list_conversations` posts into that DM or channel, from whichever agent (main or a session) calls it. If Discord refuses the post, main's own send gets an error DM to the owner the same way it always has; a session's send is subject to the same never-falls-back rule as its own replies above.
- Other proactive output from main — `send_message` without a conversation, results routed through `idle_channel = "discord"`, background turns — goes to the owner's DM. Until an owner exists it is dropped with a warning in the log.
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
