# Migrating to web-only updates while you're away

Turns your agent starts on its own, such as a session's finished result or a teammate's message to it, go to the web app once you have been away for the idle period (`[idle] timeout_minutes`, 30 minutes by default). With no `idle_channel` set, they no longer go to the Telegram, Discord or Teams chat you last wrote from. Nothing changes while you are active: until the idle period passes, these turns still reply wherever you last wrote. A message you send is always answered where you sent it, even when it arrives while one of these turns is running: the turn takes your message in, and the rest of it, typing indicator and reply included, goes to the chat you wrote from. From then on the agent's own updates follow you there until the next idle period.

## If you want updates on a chat while you're away

Name the chat in the agent's `config.toml`:

```toml
[idle]
idle_channel = "telegram"   # or "discord" or "teams"
```

Or open Settings, the agent's Runtime section, Advanced, When you're away, and pick the chat under Send its updates to. The chat's own section (`[telegram]`, `[discord]` or `[teams]`) must be configured; a chat named without it falls back to the web app, with a notice.

## What behaves differently

- **No `idle_channel` set** now means the web app. There is no setting that keeps updates on whichever chat you last used once you are away; pick the chat you want instead.
- **The Settings choice "Keep where it is" is gone.** A file with no `idle_channel` shows This app.
- **An `idle_channel` the config can't use** (a chat without its section, or an unknown name) sends updates to the web app, with a notice, rather than leaving them where you last wrote.
- **The agent is told where its updates go.** The `[Idle]` note it gets when you go away says they reach you only in the web app, and that `send_message` reaches a chat.
- **The agent can still reach a chat on purpose** with `send_message` at any time, and a `switch_endpoint` it makes still takes precedence.

See [Idle](../systems-usage/idle.md) for how the transition works.
