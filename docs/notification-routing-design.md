# Notification Routing — CHANNELS.yml

## Overview

This document describes how background task results get delivered to the user. It replaces the previous `Alerts.md` + `AlertLevel` system with a simpler, extensible model: a flat YAML file that maps notification channels to the task names they receive.

The previous design used a three-tier alert level (high/medium/low) baked into each task, with an LLM-facing prose playbook (`Alerts.md`) defining behavior at each level. This conflated routing (where results go) with urgency assessment (how important they are), made external notification channels impossible without bolting on side-channels, and put routing logic in prose that the LLM had to interpret.

The new design separates concerns cleanly:

1. **Actionability** — The SubAgent makes one binary judgment: is this result worth reporting, or is it HEARTBEAT_OK? This is the only gate.
2. **Routing** — `CHANNELS.yml` defines the channel registry (what channels exist and their configuration). Each pulse in `HEARTBEAT.yml` declares its output channels via a `channels:` field. Scheduled actions and agent-spawned subagents specify channels directly at creation time. The gateway dispatches results to the resolved channels. No urgency assessment, no alert levels.
3. **Channel infrastructure** — `channels.toml` defines what channels exist and how to reach them. This is user-managed infrastructure config, not agent-editable policy.

---

## CHANNELS.yml

A workspace file at `~/.residuum/workspace/CHANNELS.yml`. This file defines the channel registry — what channels exist and any channel-specific configuration. The agent reads and edits this file autonomously, following the same self-evolution pattern as `HEARTBEAT.yml`.

### Structure

Channels are top-level keys. Each channel entry can hold configuration metadata (or be empty for built-in channels that need no extra config).

```yaml
# CHANNELS.yml — Channel registry
# Declares available notification channels.
# This file is yours to evolve based on user preferences.

agent_wake: {}
agent_feed: {}
inbox: {}
ntfy: {}
```

### Pulse routing

Pulse routing is declared on each pulse in `HEARTBEAT.yml` via the `channels:` field. For example:

```yaml
pulses:
  - name: work_check
    schedule: "30m"
    channels: [agent_wake, ntfy]
    tasks:
      - name: check
        prompt: "Check for work updates."

  - name: system_health
    schedule: "1h"
    channels: [inbox]
    tasks:
      - name: check
        prompt: "Check system health."
```

A pulse with no `channels:` field has its results dropped after transcript storage, and a warn-level log is emitted to surface the misconfiguration.

### Direct routing

Scheduled actions and agent-spawned subagents do **not** use CHANNELS.yml for routing. Both specify their output channels directly at creation time:

- **Scheduled actions**: `channels` parameter on `schedule_action` (defaults to `["agent_feed"]`)
- **Agent-spawned subagents**: `channels` parameter on `subagent_spawn` (defaults to `["agent_feed"]`)

The gateway validates channel names at spawn time against built-in channels and `channels.toml` definitions.

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

External channels deliver results outside the gateway — push notifications, webhooks, or any service that accepts a message. They are defined in `channels.toml` and referenced by name in `CHANNELS.yml`, in the `channels` field on pulses in `HEARTBEAT.yml`, or in the `channels` parameter of `schedule_action` / `subagent_spawn`.

### Configuration

```toml
[channels.ntfy]
type = "ntfy"
url = "https://ntfy.sh"
topic = "residuum"

[channels.ops_webhook]
type = "webhook"
url = "https://hooks.example.com/residuum"
method = "POST"
```

Channel names in `channels.toml` must match the keys used in `CHANNELS.yml`. If a channel name referenced in `HEARTBEAT.yml` or `CHANNELS.yml` is not defined in `channels.toml` and not a built-in, the gateway logs a warning and skips that channel during dispatch.

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
      ├── Pulse → Use channels declared on the pulse in HEARTBEAT.yml
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

The agent edits `CHANNELS.yml` and the `channels:` field on pulses in `HEARTBEAT.yml` the same way it edits any workspace file — using the `write` or `edit` tool. Since both files are re-read from disk on every routing call, changes take effect immediately.

Examples of agent-driven routing changes:

- User says "stop pinging me about PR reviews" → agent removes `ntfy` from the `github_prs` pulse's `channels` list in HEARTBEAT.yml.
- User responds urgently to deploy failures → agent adds `agent_wake` to the `deploy_check` pulse's `channels` list in HEARTBEAT.yml.
- Agent notices a task consistently produces results the user ignores → agent moves it from `agent_feed` to `inbox` in the pulse's `channels` list, or removes the channel entirely.
- User sets up a new ntfy topic → user adds the channel to `channels.toml`, agent adds it to `CHANNELS.yml` and references it in relevant pulses' `channels` lists in HEARTBEAT.yml.

The agent's routing decisions are visible and reversible — the user can always open `HEARTBEAT.yml` and `CHANNELS.yml` and adjust.

---

## Interaction with Other Systems

### Background tasks

`CHANNELS.yml` defines the channel registry, and pulse routing is declared on each pulse in `HEARTBEAT.yml` via the `channels:` field. When a `BackgroundResult` arrives, the gateway checks the task's `ResultRouting`: all routing is now `Direct` — pulses use the channels declared on the pulse definition, while scheduled actions and agent-spawned subagents use the channels specified at creation time.

### Pulse system

Pulse evaluation results are routed by pulse name. The `alert` field on individual pulse tasks in `HEARTBEAT.yml` is removed — it served no purpose once routing is by pulse name, not by urgency level. If different tasks within a pulse need different routing, they should be separate pulses.

### Scheduled actions system

Scheduled action results use direct channel routing specified at creation time via the `channels` parameter on `schedule_action` (defaults to `["agent_feed"]`). The `agent_name` parameter controls execution mode: omitted for a default sub-agent, `"main"` for a full wake turn with conversation context, or a named preset for a specialized sub-agent. The `model_tier` parameter (`"small"`, `"medium"`, `"large"`) controls the model used for sub-agent actions.

### Agent-spawned subagents

Subagent results use direct channel routing. The main agent specifies output channels in the `subagent_spawn` tool call, and the gateway validates them at spawn time. Default: `["agent_feed"]`.

### Observational Memory

Background results enter the observation log through the standard path: results routed to `agent_wake` or `agent_feed` are injected into the message stream, and the observer compresses them naturally. Results routed only to external channels or inbox enter the observation stream when the agent reviews them (via `inbox_list` or by seeing a summary in a future turn).

### Hot reload

CHANNELS.yml is re-read from disk on every `route()` call. There is no filesystem watcher or cached dispatch table — the file is loaded fresh each time a notification needs routing. Changes take effect immediately without gateway restart.

---

## Default CHANNELS.yml

Bootstrapped on workspace creation with sensible defaults:

```yaml
# CHANNELS.yml — Channel registry
# Declares available notification channels.
# Edit this file to register channels for pulse routing and notifications.
# The agent will also evolve this file based on your preferences.
#
# Built-in channels:
#   agent_wake  — inject into agent feed, start a turn if idle
#   agent_feed  — inject into agent feed, wait for next interaction
#   inbox       — store silently, surface as unread count
#
# External channels (ntfy, webhook, etc.) are defined in channels.toml
# under [channels.<name>].

agent_feed: {}

inbox: {}
```

Minimal — no external channels registered until the user or agent configures them. Pulse routing is declared on each pulse in HEARTBEAT.yml via the `channels:` field. This avoids surprising behavior on first run.

---

## What This Replaces

| Old concept | New equivalent |
|-------------|---------------|
| `Alerts.md` (prose playbook) | Removed entirely. No LLM-interpreted routing prose. |
| `AlertLevel` enum (High/Medium/Low) | Removed entirely. No urgency tiers. |
| `alert` field on pulse tasks | Removed. Routing is by `channels:` field on each pulse in `HEARTBEAT.yml`. |
| `alert_level` on `BackgroundTask` | Removed. `BackgroundResult` carries task name only. |
| High → inject + start turn | `agent_wake` channel |
| Medium → inject passively | `agent_feed` channel |
| Low → inbox item | `inbox` channel |
| `src/pulse/alerts.rs` | Removed. |
| `load_alerts()` in executor | Removed from pulse execution path. |
| `DEFAULT_ALERTS` in bootstrap | Replaced with `DEFAULT_CHANNELS` (CHANNELS.yml template). |
| `alerts_md()` on workspace layout | Replaced with `channels_yml()`. |
