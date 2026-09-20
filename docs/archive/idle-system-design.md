# Residuum — Idle System

## Overview

After a configurable period of user inactivity (default: 30 minutes), the gateway transitions to idle state. This deactivates active projects and skills, generates an LLM-summarized log of what was active, switches notifications to a configurable default interface, and fires the observer to persist the session.

The idle system is driven by a simple mechanism: a deadline that resets on every user message. When the deadline expires, idle triggers. No periodic polling, no background checks.

---

## Design Philosophy

Same principles as the rest of Residuum:

1. **Put the right work in the right place.** The gateway tracks the timer and orchestrates the transition. The LLM generates the context-aware deactivation log. No new subsystem — just a deadline in the event loop.
2. **No silent failures.** The idle transition is logged, the user is notified on their configured idle channel, and projects get proper deactivation logs. Nothing disappears quietly.
3. **Simplicity that stays practical.** One timer, one threshold, one trigger. No idle stages, no gradual degradation, no reactivation heuristics.

---

## Mechanism

### What counts as user activity

A **user message** — any `InboundMessage` received through `inbound_rx` in the event loop. This is the same event that updates `Agent::last_user_message_at` for the status line.

What does NOT reset the timer:
- Heartbeat/pulse evaluations
- Background task results (including `agent_wake`)
- Wake turns
- LLM activity (tool calls, completions)
- The user reading without typing

### The idle deadline

The event loop maintains an `idle_deadline: Option<tokio::time::Instant>`, following the same pattern as `observe_deadline`. It resets to `now + idle_timeout` on every inbound user message. When the deadline fires, the idle transition runs.

```rust
// In run_event_loop, alongside observe_deadline:
let mut idle_deadline: Option<tokio::time::Instant> = None;

// On every inbound user message:
idle_deadline = Some(tokio::time::Instant::now() + idle_timeout);

// New select! arm:
Some(d) = async { idle_deadline }, if idle_deadline.is_some() => {
    tokio::time::sleep_until(d).await;
    execute_idle_transition(&mut rt).await;
    idle_deadline = None;
}
```

The deadline is `None` at startup (no user has interacted yet — nothing to idle from). It's set on the first user message and reset on every subsequent one, including at the end of a completed turn (so a long multi-tool turn doesn't cause idle to fire immediately after). After the idle transition fires, it's cleared back to `None` and only resets when the next user message arrives.

### No periodic check

There is no interval timer or polling. The idle deadline is a single `sleep_until` in the `select!` loop — the same pattern used for `observe_deadline`. When the user sends a message, the deadline moves forward. When it expires, idle fires exactly once. Zero cost when the user is active.

---

## Idle Transition

When the idle deadline fires, the gateway executes these steps in order. Steps 1 and 3 are no-ops if no project is active. The remaining steps (skill deactivation, observer, interface switch, system message) always run — the user is idle regardless of whether a project was loaded.

### 1. Deactivate active project and generate log

If a project is active: generate an LLM session log (see below), then deactivate the project using that log. This follows the same `project_deactivate` contract — the log is written to `notes/log/YYYY-MM/log-DD.md`. Skills loaded by project activation are removed as part of deactivation.

If no project is active, this step and the log generation are no-ops.

### 2. Deactivate remaining skills

Remove any explicitly activated skills that weren't already removed by project deactivation in step 1.

### 3. Generate deactivation log (single LLM call)

If a project is active, a single call to the user's configured `small` background model generates a context-aware summary. If no project is active, this step is a no-op — there is no log to generate.

This is NOT a full SubAgent turn loop — no tool access, no multi-turn reasoning. Just:

**Input:**
- All messages from `recent_messages.json` tagged with the active project context, capped at observer trigger tokens
- The name and description of the project being deactivated

**Prompt:**
```
The user has been inactive for {timeout} minutes. Summarize what was being
worked on and the current state, for use as a session log entry. Be concise
(2-4 sentences). Focus on what was done and what's pending.
```

**Output:** A string used as the `log` field for `project_deactivate`, and also persisted as a system message in the agent's message history for continuity.

If the LLM call fails (timeout, provider error), the gateway:

1. Saves the raw messages that would have been summarized as a JSON file in the project's log directory: `notes/log/YYYY-MM/idle-raw-DD-HHMMSS.json`.
2. Uses a structured fallback as the deactivation log: `"[idle] Auto-deactivated after {timeout}m of inactivity. LLM summary failed — raw messages saved to {path}."`.

This preserves the context so a log entry can be generated manually or on a future LLM call. The raw file contains the same message slice that was passed to the LLM prompt.

### 4. Fire the observer and clear the in-memory buffer

Fire the observer normally using the existing `execute_observation()` path — the observer sees the full recent context and produces a proper episode summary.

After the observer completes, clear the agent's in-memory message buffer. The buffer holds the last ~3 user/agent turns to preserve continuity mid-conversation. Clearing it after idle prevents the agent from seeing stale conversational context on the next interaction and thinking it's still mid-conversation. The messages are already persisted to disk and captured by the observer — the buffer clear only affects what the agent sees in its next turn's context assembly.

### 5. Switch notification interface

Update the gateway's `last_reply` to point at the configured idle interface (e.g., Telegram) so that any subsequent `agent_wake` results or notifications reach the user where they'll see them.

This requires adapters to expose an **unsolicited send handle** — a `ReplyHandle` that can be constructed without an inbound message to trigger it. Currently, reply handles are only created in response to inbound messages. Each adapter (Telegram, Discord, WebSocket) needs a factory method that produces a handle targeting a default destination.

The default destination is captured automatically from the first inbound message on each adapter (e.g., the Telegram chat ID, Discord channel ID). This can be overridden via the adapter's config section (e.g., `[telegram] idle_chat_id`). The gateway holds these handles after the first message arrives on each interface.

This same abstraction enables a future `send_message` tool where the agent directs a message to a specific interface. The idle system is the first consumer, but the capability should be designed with both use cases in mind.

### 6. Log the transition

Emit an `info!` log and inject a system message into the agent's message history:

```
[Idle] Transitioned to idle after 30m of inactivity. Deactivated project "aerohive-setup" and 2 skills. Session log written.
```

This ensures the agent has continuity context when the next user message arrives.

---

## Reactivation

There is no automatic reactivation. When the user sends their next message:

1. The message is processed normally through the inbound path.
2. The idle deadline resets to `now + idle_timeout`.
3. The agent sees the idle transition system message in its history and understands context was deactivated.
4. The agent decides whether to reactivate the previous project/skills based on the conversation — the same autonomous activation behavior that already exists.

---

## Configuration

### config.toml

```toml
[idle]
timeout_minutes = 30          # 0 = disabled
idle_channel = "telegram"     # interface to route notifications to when idle
                              # must be a configured interface name
                              # omit to keep current interface
```

### Config structs

```rust
// In deserialize.rs
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct IdleConfigFile {
    pub(super) timeout_minutes: Option<u64>,
    pub(super) idle_channel: Option<String>,
}

// In types.rs
#[derive(Clone, Debug, PartialEq)]
pub struct IdleConfig {
    /// Inactivity timeout. Duration::ZERO means disabled.
    pub timeout: Duration,
    /// Interface to switch to when idle (e.g., "telegram", "discord").
    /// None means keep the current interface.
    pub idle_channel: Option<String>,
}

impl Default for IdleConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30 * 60),
            idle_channel: None,
        }
    }
}
```

### ConfigFile addition

```rust
pub(crate) struct ConfigFile {
    // ... existing fields ...
    pub(super) idle: Option<IdleConfigFile>,
}
```

### Config addition

```rust
pub struct Config {
    // ... existing fields ...
    pub idle: IdleConfig,
}
```

### Validation

- `timeout_minutes = 0` disables the idle system entirely (no deadline set).
- `idle_channel` must match a configured interface name (`"telegram"`, `"discord"`, `"websocket"`). Invalid names are rejected at config load with a clear error.

---

## Interaction with Existing Systems

### Observational Memory

The forced observation in step 4 ensures the session is captured before context goes stale. The idle transition system message (step 6) becomes part of the observation record, providing a clear boundary marker in the episode timeline.

### Projects

Uses the standard `project_deactivate` contract. The only difference is the log comes from a single LLM call rather than the agent's own judgment. The gateway enforces the same non-empty log requirement.

### Background Tasks

Background tasks continue running during and after idle. Their results are routed normally — `agent_wake` results still trigger wake turns, `agent_feed` results queue for the next interaction, `inbox` items accumulate. The idle system does not cancel or pause background work.

### Pulses

Pulses continue firing during idle. They are independent of user activity by design — monitoring and scheduled checks should keep running regardless.

### Config Reload

If the idle timeout changes via hot-reload:
- If the deadline is currently set, recalculate it based on the new timeout from the last user message time.
- If idle is disabled (`timeout_minutes = 0`), clear the deadline.
- If idle was disabled and is now enabled, set the deadline from `last_user_message_at` if available.

---

## Event Loop Changes

The idle system adds one new arm to the `select!` loop and a deadline reset in the inbound message handler. No new channels, no new tasks, no new subsystems.

### New state in `GatewayRuntime`

```rust
struct GatewayRuntime {
    // ... existing fields ...
    /// Idle timeout duration from config. Duration::ZERO = disabled.
    idle_timeout: Duration,
}
```

### Modified inbound handler

```rust
// In the inbound message arm, after existing processing:
if !rt.idle_timeout.is_zero() {
    idle_deadline = Some(tokio::time::Instant::now() + rt.idle_timeout);
}

// Also at the end of a completed turn (after response delivery):
if !rt.idle_timeout.is_zero() {
    idle_deadline = Some(tokio::time::Instant::now() + rt.idle_timeout);
}
```

### New select! arm

```rust
// New arm alongside observe_deadline:
_ = async {
    match idle_deadline {
        Some(d) => tokio::time::sleep_until(d).await,
        None => std::future::pending().await,
    }
}, if idle_deadline.is_some() => {
    execute_idle_transition(&mut rt, &mut observe_deadline).await;
    idle_deadline = None;
}
```

### `execute_idle_transition`

```rust
async fn execute_idle_transition(
    rt: &mut GatewayRuntime,
    observe_deadline: &mut Option<tokio::time::Instant>,
) {
    let timeout_mins = rt.idle_timeout.as_secs() / 60;
    tracing::info!(timeout_mins, "idle timeout reached, transitioning");

    // 1. Generate deactivation log via single LLM call
    let log = generate_idle_log(rt).await;

    // 2. Deactivate active project (if any)
    deactivate_project_if_active(&rt.project_state, &rt.layout, &log).await;

    // 3. Deactivate active skills
    deactivate_all_skills(&rt.skill_state).await;

    // 4. Fire observer, then clear in-memory message buffer
    execute_observation(
        &rt.observer, &rt.reflector, &rt.search_index,
        &rt.layout, &mut rt.agent,
        rt.vector_store.as_ref(), rt.embedding_provider.as_ref(),
    ).await;
    *observe_deadline = None;
    rt.agent.clear_messages();

    // 5. Switch notification interface
    if let Some(ref channel_name) = rt.cfg.idle.idle_channel {
        switch_idle_interface(rt, channel_name).await;
    }

    // 6. Inject system message for continuity
    let summary = format_idle_summary(timeout_mins, &log);
    rt.agent.inject_system_message(summary);
}
```

---

## Implementation Phases

### Phase 1: Core idle system

Config, timer, deactivation, LLM log, observer, and buffer clear. All internal gateway work — no adapter changes, no UI.

- Add `IdleConfig` to config structs, deserialization, and resolve
- Add `idle_deadline` to event loop state
- Reset deadline on inbound messages and at end of completed turns
- New `select!` arm for idle deadline expiry
- `execute_idle_transition`: deactivate project (with LLM-generated log), deactivate skills
- Single LLM call using `small` background model tier with project-tagged messages from `recent_messages.json`
- On LLM failure: save raw messages as JSON, use structured fallback log
- Fire observer after deactivation, then clear in-memory message buffer
- Inject system message for agent continuity
- Tests: deadline resets on message, resets after turn, fires after timeout, deactivates project/skills, LLM log generation, LLM failure fallback, observer fires, buffer cleared, no-op when nothing active

**Milestone: Projects and skills auto-deactivate after inactivity with context-aware session logs.**

### Phase 2: Interface switching, UI, and config reload

Adapter plumbing, config UI, and hot-reload support.

- Unsolicited send handles: adapter factory methods for Telegram, Discord, WebSocket
- Default destination captured from first inbound message per adapter, with config override
- `idle_channel` config validation against configured interfaces
- `switch_idle_interface` implementation using unsolicited send handles
- Config UI updates for idle settings (timeout, idle channel)
- Config hot-reload: recalculate deadline on timeout change, handle enable/disable transitions
- Tests: interface switches, config validation, reload transitions, UI fields

**Milestone: Full idle system with interface switching and config UI.**

---

## What's Not Included

- **Multiple idle stages.** One timeout, one transition. No "soft idle" vs "deep idle."
- **Activity tracking beyond messages.** Tool approvals, file reads, or other implicit signals are not tracked. If you're not sending messages, you're inactive.
- **Automatic reactivation.** The agent decides what to reactivate based on the next conversation, same as always.
- **Idle-specific scheduled actions.** Pulses and actions continue normally — idle is about user presence, not system activity.
- **Pausing background work.** Background tasks, pulses, and scheduled actions are independent of user activity.
