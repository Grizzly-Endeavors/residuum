# Idle

After a configurable period of user inactivity, the gateway runs an idle transition: it deactivates any explicitly-activated skills, switches where agent-started turns are delivered (the web UI unless `idle_channel` names a chat), fires the observer and clears the in-memory message buffer, and injects a continuity message into the agent's history. There is no polling — a single deadline in the event loop's `select!` loop (`idle_deadline`, alongside `observe_deadline`) fires once and is cleared.

## Trigger

Only a person's messages reset the idle deadline: at the end of the turn the message started (`handle_inbound_message` in `src/gateway/event_loop/turns.rs`), or, for a message that joins a running agent-started turn, the moment it arrives (`OwnerFollow`, see below). Background-origin turns (`origin.endpoint == "background"`) do not. The deadline is set to `now + rt.cfg.idle.timeout` (with `last_user_message_instant` set to `now`), provided the timeout isn't zero. When the deadline expires, `idle::execute_idle_transition` (`src/gateway/idle.rs`) runs and `idle_deadline` is cleared back to `None` (`src/gateway/event_loop/run_loop.rs`). Nothing resets the deadline besides a person's message — tool calls, background task results, and pulses are not activity.

`timeout_minutes = 0` disables the system entirely: the deadline is never set.

## Transition sequence

`execute_idle_transition` runs these steps in order:

1. **Deactivate explicitly-activated skills** (`deactivate_remaining_skills`).
2. **Switch where agent-started turns go** to `idle.idle_channel` (see below). This happens when the transition starts, so an owner who writes while the observer runs moves routing back to themselves and stays there.
3. **Fire the observer, then clear the in-memory message buffer.** Runs the normal `execute_observation` path in the background memory worker so the episode gets a proper summary; once it returns, `apply_idle_continuation` calls `rt.agent.clear_messages()` so the agent doesn't see stale mid-conversation context on the next turn. Also clears `observe_deadline`.
4. **Inject a continuity system message** built by `format_idle_summary`, also from `apply_idle_continuation`. It tells the agent where turns it starts now reach the user, e.g. `[Idle] Transitioned to idle after 30m of inactivity. Deactivated 2 skills. Until the user writes again, turns you start reach them only in the web app; use send_message to reach them on a chat.` With a chat idle channel the last sentence is `Until the user writes again, turns you start reach them on telegram.`

## Switching where agent-started turns go

An agent-started main turn is one no person's message started: a session's relayed result, a teammate's `message_agent` to main, a delivery-failure notice. Its replies go to `rt.last_output_endpoint` (or to the endpoint `switch_endpoint` chose), and every person's message sets that field to the endpoint it came from, so while the owner is active these turns reply wherever the owner last wrote.

The idle transition sets `rt.last_output_endpoint` to the resolved `idle.idle_channel` through `idle_output_endpoint`. It looks the channel up in `rt.endpoint_registry`; when the endpoint is missing or not `EndpointCapabilities::INTERACTIVE`, it logs a `warn` and uses the web UI's `ws` endpoint instead. The web UI is always registered, so after the idle period agent-started turns always go somewhere defined, never to the chat the owner happened to use last.

With the default (`ws`), a chat interface is handed none of an agent-started turn's events after the idle period: no typing indicator, intermediate text, reply or error. The web UI shows the turn in the main conversation as it shows every main turn. The agent still reaches a chat deliberately with `send_message`, and a `switch_endpoint` the agent sets still takes precedence over the idle channel, as it does over the owner's last endpoint. The owner's next message, from any interface, sets `last_output_endpoint` back to where they wrote from.

A person's message can also arrive while an agent-started turn is running. It joins that turn, as any mid-turn message does, and the owner counts as back on that interface from the moment it arrives (`OwnerFollow::owner_wrote_from` in `src/gateway/event_loop/turns.rs`): `last_output_endpoint` moves there, the `switch_endpoint` override is cleared, and the idle timer restarts. The running turn's output endpoint is a `LiveOutputEndpoint` the turn reads at each publish, so the rest of that turn goes there too: its typing indicator moves to the owner's chat (the chat it was on, if any, gets the turn's end), and its later intermediate text, its reply, and a failure are delivered there. So a message the owner sends is always answered where they sent it.

Owner-started turns keep their output where they started: a person's message is always answered where it came from.

## Configuration

`config.toml`:

```toml
[idle]
timeout_minutes = 30       # 0 disables the idle system entirely
idle_channel = "telegram"  # optional; where agent-started turns go once you're away (default: the web UI)
```

Resolved into `IdleConfig { timeout: Duration, idle_channel: String }` (`crates/config/src/types.rs`) by `resolve_idle_config` (`crates/config/src/resolve/channels.rs`). `idle_channel` always resolves to a concrete endpoint: unset, `"websocket"` and `"ws"` resolve to the web UI's `ws` endpoint; `"telegram"`, `"discord"` and `"teams"` are kept when the corresponding `[telegram]`/`[discord]`/`[teams]` section is present. A chat named without its section, or any other name, resolves to `ws` with a notice naming the problem, rather than failing config load over one bad setting. This config-time validation is independent of the endpoint-registry check above, which handles a channel that resolved fine here but is not registered by the time the transition runs.

The web UI edits both keys in the agent's Runtime settings (Advanced), under When you're away: Quiet for (`timeout_minutes`, in minutes, 0 turns idle off) and Send its updates to (`idle_channel`: This app, Telegram, Discord or Microsoft Teams). A file with no `idle_channel` shows This app.

### Hot reload

A config reload that changes `[idle]` is detected via `ConfigDiff::idle_changed` (`src/gateway/reload.rs`) and translated into an `IdleAction`:

- `Disable` — timeout became zero; clears `idle_deadline` immediately.
- `Recalculate { new_timeout }` — the timeout or `idle_channel` changed; the deadline is recomputed from `last_user_message_instant` plus the timeout. If that recalculated deadline has already passed, the idle transition runs again immediately instead of waiting for the next tick. That is also how a new `idle_channel` takes effect while the owner is away: the transition re-runs (another observe, message clear and `[Idle]` message) and switches to the new channel.
- `None` — no idle-relevant change.

## Reactivation

There is none, automatic or otherwise. The next user message is processed normally, the idle deadline resets, and the agent sees the injected `[Idle]` system message in its history — whether it reactivates the previous skills is left entirely to its own judgment based on the conversation that follows.

## Interaction with Other Systems

- **Skills**: explicitly-activated skills are swept in step 1. See [skills.md](skills.md).
- **Memory**: the observer fires before the message buffer is cleared, so the idle boundary is captured in the episode record rather than lost. See [memory.md](memory.md).
- **Notifications**: switching `last_output_endpoint` decides where relayed session results, teammates' messages and other agent-started turns surface after the user goes idle, until the user writes again. See [notifications.md](notifications.md).
- **Background Tasks**: unaffected by idle — background tasks, pulses, and scheduled actions keep running regardless of user activity; only user messages drive the idle timer. See [background-tasks.md](background-tasks.md).
