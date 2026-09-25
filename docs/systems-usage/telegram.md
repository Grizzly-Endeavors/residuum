# Telegram

The Telegram interface lets the agent chat in Telegram private chats and in groups the bot has been added to. Residuum long-polls the Bot API with the bot token, so no public endpoint is needed.

## Startup and recovery

Verifying the bot token (`getMe`) at startup talks to the Bot API. A transient failure there (network not up yet, a momentary API error) retries with exponential backoff instead of leaving the adapter dead until a config reload — a notice is published once when retries start, once on recovery, and once if it gives up after 10 attempts. A corrupt `telegram_state.json` is moved aside (to `telegram_state.json.corrupt`) and the interface starts fresh with a notice, rather than staying down until someone fixes the file by hand — the owner will need to message the bot again to be recognized.

## Who the agent answers

The **owner** is whoever first sends the bot a private message. Their Telegram user ID and chat are saved in `telegram_state.json` in the workspace, so ownership survives restarts. To hand the bot to someone else, stop Residuum, remove the `owner` entry from that file, and have the new owner message the bot.

`[telegram] respond_to_others` controls everyone else:

- `false` (default): only the owner's messages reach the agent. Anyone else who messages or addresses the bot gets a short reply saying it only takes requests from the owner. Before an owner exists, the reply asks the owner to message the bot first.
- `true`: anyone who can reach the bot can talk to the agent.

Commands (`/stop`, `/reload`, `/inbox`, …) run only for the owner in either mode.

Every message the agent sees records who sent it and where, e.g. `[From: Bear Flinn via telegram (group "Launch prep")]` — see [Message Senders](memory.md#message-senders).

## Conversation routing

Only the owner's own private chat reaches the main agent. Every other admitted conversation — a group or supergroup, and a non-owner's private chat when `respond_to_others` is on — is handled by an [agent session](background-tasks.md) of its own instead: a temporary fork of the main agent, addressed deterministically by that chat, that keeps its own memory and idle timeout rather than sharing the owner's private conversation. This holds even when the owner is the one talking in a group — a group is still a shared space, so it gets a session, not main. The session sees the same sender attribution and buffered chatter described below, and replies into that chat — see [Where replies go](#where-replies-go).

`/stop` follows the same routing: typed in the owner's own private chat it stops main's current turn; typed in a group or supergroup it stops that chat's session instead, never main — see [Turn Control](turn-control.md).

## Private chats and groups

In a private chat every message goes to the agent.

In a group or supergroup the agent acts only on messages addressed to the bot:

- a message that mentions `@botusername` (the mention is stripped from the text);
- a reply to one of the bot's messages;
- a command, unless it names a different bot (`/status@otherbot`).

With Telegram's default privacy mode these are also the only group messages the bot receives, so everything else in the group stays invisible to Residuum. **To have the bot see and buffer unaddressed group messages, turn off privacy mode for the bot in [@BotFather](https://t.me/BotFather)** (`/mybots` → your bot → *Bot Settings* → *Group Privacy* → *Turn off*), or make the bot a group admin, which grants it the same visibility. This is a platform restriction, not something Residuum can work around from its side.

Once the bot receives them, unaddressed group messages are held in memory — never written to disk, and lost on restart — up to `[telegram] context_messages` per group (default 20; `0` disables), across at most 256 groups at once; past that, the group that went longest without new chatter is dropped. When the bot is next addressed there, the held messages are handed to the agent as a single background message placed just before the addressed one:

```
Previous conversation in group "Launch prep" since you were last mentioned there. This is background only; the message that mentions you follows.
[14:05] Sam Lee: build is red again
[14:06] Priya: looks like the migration step
```

The buffer for that group is emptied when it is delivered, so each message reaches the agent at most once. Photo, document, and video captions count as the message text. Channels (broadcast-only) are not supported.

## Where replies go

- A private-chat reply goes to the owner from the main agent's own conversation.
- A group's reply comes from that group's own session and always goes back to that same group. It never falls back to the owner's chat: if Telegram refuses the post (the bot was removed, or can't write there), the output is dropped, the error is logged naming the session and chat, and main gets a notice so the owner can be told if it matters.
- `send_message` with a `conversation` from `list_conversations` posts into that private chat or group, from whichever agent (main or a session) calls it. If Telegram refuses the post, main's own send gets an error message to the owner the same way it always has; a session's send is subject to the same never-falls-back rule as its own replies above.
- Other proactive output from main — `send_message` without a conversation, results routed through `idle_channel = "telegram"`, background turns — goes to the owner's chat. Until an owner exists it is dropped with a warning in the log.
- System notices and errors go only to the owner's chat, never into a group.

Long replies are split into 4096-character messages. A typing indicator shows while a turn runs. Files the agent sends go out as photos, audio, or documents by type; captions longer than Telegram's 1024-character limit are followed by the full text.

## Conversations the agent can list

Telegram gives bots no way to list their chats, so `list_conversations` shows what `telegram_state.json` has recorded: every private chat that has messaged the bot (`direct message with Bear Flinn`), and every group the bot has been added to or addressed in (`group "Launch prep"`). A group is dropped when the bot is removed from it, and when a group is upgraded to a supergroup its old ID is dropped; the new one appears once the bot is next addressed there.

## Configuration

```toml
[telegram]
token = "${RESIDUUM_TELEGRAM_TOKEN}"   # or secret:telegram; RESIDUUM_TELEGRAM_TOKEN also works on its own
respond_to_others = false
context_messages = 20
```

Changing any `[telegram]` value restarts the Telegram adapter on reload.

## Code

`src/interfaces/telegram/`: `mod.rs` (shared state, reply routing, conversation listing), `handler.rs` (long polling, inbound messages, commands, group membership), `groups.rs` (whether a group message is addressed to the bot), `subscriber.rs` (outbound delivery). Owner and conversation state is `src/interfaces/chat_state.rs`, shared with Discord and Teams. The unaddressed-message buffer is `src/interfaces/context_buffer.rs`, shared with Discord and Teams.
