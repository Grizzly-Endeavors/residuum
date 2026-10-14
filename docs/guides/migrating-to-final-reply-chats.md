# Migrating to final-reply chats

Telegram, Discord, and Microsoft Teams now get only each turn's final reply. What the agent writes before a tool call, such as "checking the calendar first", is no longer posted to the chat as its own message. A turn that used to arrive as a run of short messages now arrives as one reply, with the typing indicator showing while the agent works.

## What behaves differently

- **Chats get the final reply only.** This covers the main agent's turns in your own direct messages and every conversation session's turns in group chats, channels, and other people's direct messages.
- **The final reply is the text after the turn's last tool call.** An answer the agent wrote before a trailing tool call, such as saving a note, reaches the chat only with `deliver_intermediate` on. The agent's system prompt tells it to put the full answer in its final reply.
- **The web UI is unchanged.** It still shows the agent's text, tool calls, and reply as the turn runs, including turns started from a chat.
- **Errors and `send_message` are unchanged.** A failed turn still sends its plain-language error to the chat that started it, and a message the agent sends with `send_message` still goes out when it is sent.
- **The typing indicator runs for the whole turn**, through every tool call, and stops when the turn ends.

## Getting the old behavior back

Turn on **Post updates while working** for the interface in Settings → (agent) → Connections, and choose **Save changes**. Or set it in the agent's `config.toml`:

```toml
[telegram]
deliver_intermediate = true
```

The same key works under `[discord]` and `[teams]`. Each interface is set on its own, so you can keep Telegram quiet and still see progress in a Teams channel. Saving restarts that interface's connection.

See [Telegram](../systems-usage/telegram.md#where-replies-go), [Discord](../systems-usage/discord.md#where-replies-go), and [Teams](../systems-usage/teams.md#where-replies-go) for where each kind of output goes.
