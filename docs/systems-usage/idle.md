# Idle

After a configurable period of user inactivity, the gateway runs an idle transition: it deactivates any explicitly-activated skills, fires the observer and clears the in-memory message buffer, optionally switches where subsequent output is routed, and injects a continuity message into the agent's history. There is no polling — a single deadline in the event loop's `select!` loop (`idle_deadline`, alongside `observe_deadline`) fires once and is cleared.

## Trigger

Only inbound user messages reset the idle deadline (`handle_inbound_message` in `src/gateway/event_loop/turns.rs`) — background-origin turns (`origin.endpoint == "background"`) do not. On a qualifying message, `idle_deadline` is set to `now + rt.cfg.idle.timeout`, provided the timeout isn't zero. When the deadline expires, `idle::execute_idle_transition` (`src/gateway/idle.rs`) runs and `idle_deadline` is cleared back to `None` (`src/gateway/event_loop/run_loop.rs`). Nothing resets the deadline besides a new user message — tool calls, background task results, and pulses are not activity.

`timeout_minutes = 0` disables the system entirely: the deadline is never set.

## Transition sequence

`execute_idle_transition` runs these steps in order:

1. **Deactivate explicitly-activated skills** (`deactivate_remaining_skills`).
2. **Fire the observer, then clear the in-memory message buffer.** Runs the normal `execute_observation` path so the episode gets a proper summary, then calls `rt.agent.clear_messages()` so the agent doesn't see stale mid-conversation context on the next turn. Also clears `observe_deadline`.
3. **Switch the notification interface**, if `idle.idle_channel` is configured (see below).
4. **Inject a continuity system message** built by `format_idle_summary`, e.g. `[Idle] Transitioned to idle after 30m of inactivity. Deactivated 2 skills.`

## Switching the notification interface

`switch_idle_interface` looks up `idle.idle_channel` in `rt.endpoint_registry`. If the endpoint exists and has `EndpointCapabilities::INTERACTIVE`, `rt.last_output_endpoint` is set to it — the same field that already governs where background-turn responses get routed, so this reuses existing routing plumbing rather than a separate mechanism. If the endpoint is missing or not interactive, the switch is skipped with a `warn` log and the current output topic is left unchanged; this is a soft fallback, not a hard failure.

## Configuration

`config.toml`:

```toml
[idle]
timeout_minutes = 30       # 0 disables the idle system entirely
idle_channel = "telegram"  # optional; interface to route output to when idle
```

Resolved into `IdleConfig { timeout: Duration, idle_channel: Option<String> }` (`src/config/types.rs`) by `resolve_idle_config` (`src/config/resolve/mod.rs`). `idle_channel`, if set, is validated at config load time against the interfaces actually configured: `"telegram"` and `"discord"` require the corresponding `[telegram]`/`[discord]` section to be present, `"websocket"` is always valid, and any other name is rejected outright. An invalid value fails config load with a `FatalError::Config` — it is not a silent runtime fallback like the endpoint-registry check in `switch_idle_interface` above.

### Hot reload

A config reload that changes `[idle]` is detected via `ConfigDiff::idle_changed` (`src/gateway/reload.rs`) and translated into an `IdleAction`:

- `Disable` — timeout became zero; clears `idle_deadline` immediately.
- `Recalculate { new_timeout }` — timeout changed; the deadline is recomputed from `last_user_message_instant` plus the new timeout. If that recalculated deadline has already passed, the idle transition runs immediately instead of waiting for the next tick.
- `None` — no idle-relevant change.

## Reactivation

There is none, automatic or otherwise. The next user message is processed normally, the idle deadline resets, and the agent sees the injected `[Idle]` system message in its history — whether it reactivates the previous skills is left entirely to its own judgment based on the conversation that follows.

## Interaction with Other Systems

- **Skills**: explicitly-activated skills are swept in step 1. See [skills.md](skills.md).
- **Memory**: the observer fires before the message buffer is cleared, so the idle boundary is captured in the episode record rather than lost. See [memory.md](memory.md).
- **Notifications**: switching `last_output_endpoint` affects where wake-turn and relayed background-task output surfaces after the user goes idle. See [notifications.md](notifications.md).
- **Background Tasks**: unaffected by idle — background tasks, pulses, and scheduled actions keep running regardless of user activity; only user messages drive the idle timer. See [background-tasks.md](background-tasks.md).
