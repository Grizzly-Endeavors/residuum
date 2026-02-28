# Notification Routing — NOTIFY.yml

## Overview

This document describes how background task results get delivered to the user. It replaces the previous `Alerts.md` + `AlertLevel` system with a simpler, extensible model: a flat YAML file that maps notification channels to the task names they receive.

The previous design used a three-tier alert level (high/medium/low) baked into each task, with an LLM-facing prose playbook (`Alerts.md`) defining behavior at each level. This conflated routing (where results go) with urgency assessment (how important they are), made external notification channels impossible without bolting on side-channels, and put routing logic in prose that the LLM had to interpret.

The new design separates concerns cleanly:

1. **Actionability** — The SubAgent makes one binary judgment: is this result worth reporting, or is it HEARTBEAT_OK? This is the only gate.
2. **Routing** — `NOTIFY.yml` maps channel names to pulse name lists (for heartbeat pulses). Scheduled actions and agent-spawned subagents specify channels directly at creation time. The gateway dispatches results to the resolved channels. No urgency assessment, no alert levels.
3. **Channel infrastructure** — `config.toml` defines what channels exist and how to reach them. This is user-managed infrastructure config, not agent-editable policy.

---

## NOTIFY.yml

A workspace file at `~/.ironclaw/workspace/NOTIFY.yml`. The agent reads and edits this file autonomously, following the same self-evolution pattern as `HEARTBEAT.yml`.

### Structure

Channels are top-level keys. Each channel lists the heartbeat pulse names whose results it should receive.

```yaml
# NOTIFY.yml — Notification routing
# Maps channels to the heartbeat pulses they receive.
# This file is yours to evolve based on user preferences.

agent_wake:
  - work_check
  - deploy_check

agent_feed:
  - github_prs

ntfy:
  - work_check
  - deploy_check
  - github_prs

inbox:
  - system_health
```

### Reading the file

The file answers one question per channel: "what will this channel send me?"

- `agent_wake` will start a turn for `work_check` and `deploy_check` pulse results.
- `ntfy` will push-notify for `work_check`, `deploy_check`, and `github_prs` pulse results.
- `system_health` pulse results go to the inbox and nowhere else.

A pulse can appear in multiple channels. A pulse not listed in any channel is not routed — its result is silently discarded after transcript storage.

### Task name resolution

Task names in `NOTIFY.yml` correspond to pulse names from `HEARTBEAT.yml` (e.g., `work_check`). NOTIFY.yml is exclusively for heartbeat pulse routing.

Scheduled actions and agent-spawned subagents do **not** use NOTIFY.yml. Both specify their output channels directly at creation time:

- **Scheduled actions**: `channels` parameter on `schedule_action` (defaults to `["agent_feed"]`)
- **Agent-spawned subagents**: `channels` parameter on `subagent_spawn` (defaults to `["agent_feed"]`)

The gateway validates channel names at spawn time against built-in channels and `config.toml` definitions.

---

## Built-in Channels

Four channel types are built into the gateway. They require no external configuration.

### `agent_wake`

Injects the result into the main agent's message feed. If the agent is idle, starts a new turn immediately. If the agent is busy, the result is injected at the next interrupt checkpoint.

Use for results that need the agent's attention as soon as possible.

### `agent_feed`

Injects the result into the main agent's message feed passively. If the agent is busy, injected at the next interrupt checkpoint. If the agent is idle, queued for the next user-initiated turn. Does not start a turn on its own.

Use for results the agent should see at the next natural interaction point.

### `inbox`

Creates an `InboxItem` — a lightweight record stored as an individual JSON file in the `inbox/` directory. Never enters the message feed. The agent sees an unread count in its context ("You have 3 unread inbox items") and can review items via the `inbox_list` tool.

Use for results that should be recorded but don't need immediate attention.

### (no listing)

A task not listed in any channel produces its result, writes the transcript to disk, and stops. The result is available in the transcript archive but is not delivered anywhere.

---

## External Channels

External channels deliver results outside the gateway — push notifications, webhooks, or any service that accepts a message. They are defined in `config.toml` and referenced by name in `NOTIFY.yml` or in the `channels` parameter of `schedule_action` / `subagent_spawn`.

### Configuration

```toml
[notifications.channels.ntfy]
type = "ntfy"
url = "https://ntfy.sh"
topic = "ironclaw"

[notifications.channels.ops_webhook]
type = "webhook"
url = "https://hooks.example.com/ironclaw"
method = "POST"
```

Channel names in `config.toml` must match the keys used in `NOTIFY.yml`. If `NOTIFY.yml` references a channel name not defined in `config.toml` and not a built-in, the gateway logs a warning and skips that channel during dispatch.

### Channel types

| Type | Description |
|------|-------------|
| `ntfy` | Push notification via [ntfy](https://ntfy.sh). Fields: `url`, `topic`, `priority` (optional, default `"default"`). |
| `webhook` | HTTP POST/PUT to an arbitrary URL. Fields: `url`, `method` (optional, default `"POST"`), `headers` (optional). Payload is JSON: `{ task_name, summary, timestamp, source_type }`. |

Additional channel types can be added by implementing the `NotificationChannel` trait. The gateway discovers channel types from the `type` field in config.

### Trait

```rust
#[async_trait]
trait NotificationChannel: Send + Sync {
    /// Channel name as configured.
    fn name(&self) -> &str;

    /// Deliver a notification. Errors are logged, not propagated —
    /// a failed external channel should not block other deliveries.
    async fn deliver(&self, notification: &Notification) -> Result<()>;
}

struct Notification {
    pub task_name: String,
    pub summary: String,
    pub source: TaskSource,
    pub timestamp: DateTime<Utc>,
}
```

Built-in channels (`agent_wake`, `agent_feed`, `inbox`) also implement this trait, but their delivery mechanics are handled directly by the gateway's interrupt system rather than through external I/O.

---

## Routing Flow

```
Background task completes
      │
      ├── SubAgent returned HEARTBEAT_OK
      │   └── Log silently, no routing. Done.
      │
      └── SubAgent returned a result
          │
          ▼
    Write transcript to disk
          │
          ▼
    Determine channels:
      ├── Pulse → Look up pulse name in NOTIFY.yml
      ├── Action → Use channels from schedule_action call (direct routing)
      └── Agent-spawned → Use channels from subagent_spawn call (direct routing)
          │
          ├── No channels → Done (transcript preserved, not delivered)
          │
          └── One or more channels
              │
              ▼
        Dispatch to all channels in parallel:
              │
              ├── agent_wake → Interrupt channel (Interrupt::BackgroundResult), wake if idle
              ├── agent_feed → Interrupt channel (Interrupt::BackgroundResult), passive
              ├── inbox → Write InboxItem to inbox/ directory
              └── ntfy/webhook/... → HTTP request to external service
```

External channel delivery is fire-and-forget from the routing perspective. Failures are logged at `warn` level but do not retry or block other channels. If reliability is needed, external services should handle their own delivery guarantees (ntfy supports message caching, webhooks can use retry-capable endpoints).

---

## Agent Self-Evolution

The agent edits `NOTIFY.yml` the same way it edits `HEARTBEAT.yml` — using the `write` or `edit` tool. Since the file is re-read from disk on every `route()` call, changes take effect immediately.

Examples of agent-driven routing changes:

- User says "stop pinging me about PR reviews" → agent removes `github_prs` from the `ntfy` channel list.
- User responds urgently to deploy failures → agent adds `deploy_check` to `agent_wake`.
- Agent notices a task consistently produces results the user ignores → agent moves it from `agent_feed` to `inbox` or removes it entirely.
- User sets up a new ntfy topic → user adds the channel to `config.toml`, agent adds task names to `NOTIFY.yml`.

The agent's routing decisions are visible and reversible — the user can always open `NOTIFY.yml` and adjust.

---

## Interaction with Other Systems

### Background tasks

`NOTIFY.yml` is consumed by the gateway's result routing step for pulse results. When a `BackgroundResult` arrives, the gateway checks the task's `ResultRouting`: `Notify` routes (pulses) look up the pulse name in NOTIFY.yml, while `Direct` routes (scheduled actions and agent-spawned subagents) dispatch to the channels specified at creation time.

### Pulse system

Pulse evaluation results are routed by pulse name. The `alert` field on individual pulse tasks in `HEARTBEAT.yml` is removed — it served no purpose once routing is by pulse name, not by urgency level. If different tasks within a pulse need different routing, they should be separate pulses.

### Scheduled actions system

Scheduled action results use direct channel routing specified at creation time via the `channels` parameter on `schedule_action` (defaults to `["agent_feed"]`). They do not route through NOTIFY.yml. The `agent_name` parameter controls execution mode: omitted for a default sub-agent, `"main"` for a full wake turn with conversation context, or a named preset for a specialized sub-agent. The `model_tier` parameter (`"small"`, `"medium"`, `"large"`) controls the model used for sub-agent actions.

### Agent-spawned subagents

Subagent results bypass NOTIFY.yml entirely. The main agent specifies output channels in the `subagent_spawn` tool call, and the gateway validates them at spawn time. Default: `["agent_feed"]`.

### Observational Memory

Background results enter the observation log through the standard path: results routed to `agent_wake` or `agent_feed` are injected into the message stream, and the observer compresses them naturally. Results routed only to external channels or inbox enter the observation stream when the agent reviews them (via `inbox_list` or by seeing a summary in a future turn).

### Hot reload

NOTIFY.yml is re-read from disk on every `route()` call. There is no filesystem watcher or cached dispatch table — the file is loaded fresh each time a notification needs routing. Changes take effect immediately without gateway restart.

---

## Default NOTIFY.yml

Bootstrapped on workspace creation with sensible defaults:

```yaml
# NOTIFY.yml — Notification routing
# Maps channels to the heartbeat pulses they receive.
# Edit this file to control where pulse results are delivered.
# The agent will also evolve this file based on your preferences.
#
# Built-in channels:
#   agent_wake  — inject into agent feed, start a turn if idle
#   agent_feed  — inject into agent feed, wait for next interaction
#   inbox       — store silently, surface as unread count
#
# External channels (ntfy, webhook, etc.) are defined in config.toml
# under [notifications.channels].

agent_feed: []

inbox: []
```

Minimal — no tasks routed anywhere until the user or agent configures them. This avoids surprising behavior on first run.

---

## What This Replaces

| Old concept | New equivalent |
|-------------|---------------|
| `Alerts.md` (prose playbook) | Removed entirely. No LLM-interpreted routing prose. |
| `AlertLevel` enum (High/Medium/Low) | Removed entirely. No urgency tiers. |
| `alert` field on pulse tasks | Removed. Routing is by pulse name in `NOTIFY.yml`. |
| `alert_level` on `BackgroundTask` | Removed. `BackgroundResult` carries task name only. |
| High → inject + start turn | `agent_wake` channel |
| Medium → inject passively | `agent_feed` channel |
| Low → inbox item | `inbox` channel |
| `src/pulse/alerts.rs` | Removed. |
| `load_alerts()` in executor | Removed from pulse execution path. |
| `DEFAULT_ALERTS` in bootstrap | Replaced with `DEFAULT_NOTIFY` (NOTIFY.yml template). |
| `alerts_md()` on workspace layout | Replaced with `notify_yml()`. |
