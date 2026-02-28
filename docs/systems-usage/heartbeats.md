# Heartbeats

Heartbeats are ambient scheduled checks the agent performs in the background. The gateway handles all scheduling; the LLM is only invoked when a pulse is due.

## HEARTBEAT.yml

The agent owns this file and evolves it over time — adding new pulses, adjusting schedules, disabling noisy ones, changing routing.

```yaml
pulses:
  - name: email_check
    enabled: true
    schedule: "30m"
    active_hours: "08:00-18:00"
    agent: ~                        # null = sub-agent, small tier
    tasks:
      - name: check_inbox
        prompt: "Check my email for urgent messages. Report anything requiring action."

  - name: daily_plan
    enabled: true
    schedule: "24h"
    active_hours: "07:00-08:00"
    agent: main                     # full wake turn on main agent
    tasks:
      - name: plan
        prompt: "Review today's calendar and inbox. Draft a plan for the day."

  - name: deploy_watch
    enabled: true
    schedule: "5m"
    agent: deploy-watcher           # named preset from subagents/
    tasks:
      - name: check_status
        prompt: "Check deployment pipeline status. Report failures."
```

### Fields

| Field | Type | Required | Notes |
|-------|------|----------|-------|
| `name` | string | yes | Identifies the pulse |
| `enabled` | boolean | no | Default `true`. Set `false` to pause without deleting. |
| `schedule` | string | yes | Duration: `"30s"`, `"5m"`, `"2h"`, `"1d"` |
| `active_hours` | string | no | `"HH:MM-HH:MM"` in configured timezone. Supports overnight windows (e.g. `"22:00-06:00"`). |
| `agent` | string or null | no | See agent routing table below. |
| `trigger_count` | integer or null | no | Max firings per active period. When set, firings are spaced evenly across the `active_hours` window. Omit for unlimited. |
| `channels` | array of strings | no | Notification channels to receive results (e.g. `[agent_feed, inbox]`). If omitted, results are dropped with a warning. |
| `tasks` | array of objects | yes | Each task has `name` (string) and `prompt` (string). |

### Agent Routing

| Value | Execution | Model Tier |
|-------|-----------|------------|
| `~` (null / omitted) | Sub-agent | Small |
| `"main"` | Main agent wake turn | Main model |
| `"<preset-name>"` | Sub-agent with named preset from `subagents/` | Preset's tier (default: small) |

**Use `"main"` sparingly** — it wakes the main agent and injects a full turn. Reserve for tasks that need conversation context or should produce a visible response.

### HEARTBEAT_OK Convention

Sub-agent pulses include an instruction: if nothing actionable was found, return the exact string `HEARTBEAT_OK`. Results containing this string are silently discarded and never routed, regardless of the pulse's `channels` configuration.

## Scheduling Behavior

- The scheduler runs on a **60-second tick**, so precision is at best ~1 minute
- HEARTBEAT.yml is **hot-reloaded** on every tick — changes take effect without restarting the gateway
- Last-run timestamps and run counts are persisted to `pulse_state.json` in the workspace, so pulses resume their schedule across gateway restarts. Missing or corrupt state files are treated as empty state (logged at warn level).
- Multiple due pulses all fire simultaneously (subject to `max_concurrent` from `[background]` config)

## Trigger Count

The `trigger_count` field limits how many times a pulse fires within its `active_hours` window. When set, the scheduler spaces firings evenly across the active period with ±15% jitter (deterministic per pulse name and date).

```yaml
pulses:
  - name: standup_check
    enabled: true
    schedule: "10m"           # minimum interval between fires
    active_hours: "09:00-17:00"
    trigger_count: 3          # fire at most 3 times across the 8h window
    tasks:
      - name: check
        prompt: "Any blockers or updates?"
```

In this example, the 8-hour window divided by 3 gives ~2h40m spacing. The `schedule` field acts as a floor — the effective interval is `max(schedule, spacing_with_jitter)`.

**Behavior:**
- Run counts are tracked per-pulse in `pulse_state.json` alongside last-run timestamps
- Counts reset when the active period rolls over (new calendar day, or last run was outside the current window)
- If `trigger_count` is set without `active_hours`, the active period defaults to 24 hours
- Omitting `trigger_count` (or setting it to null) means the pulse fires on its `schedule` interval with no cap

## Result Routing

Pulse results are routed to the channels declared on the pulse via the `channels:` field in HEARTBEAT.yml. For example, a pulse with `channels: [agent_feed, inbox]` delivers results to both `agent_feed` and `inbox`.

If a pulse has no `channels:` field, its results are dropped and a warn-level log is emitted to surface the misconfiguration.

See [notifications.md](notifications.md) for the full routing model.

### Channels Field

Each pulse can declare a `channels` field — an array of channel names to receive the pulse's results:

```yaml
pulses:
  - name: email_check
    schedule: "30m"
    channels: [agent_feed, inbox]
    tasks:
      - name: check
        prompt: "Check email."
```

Channel names must correspond to built-in channels (`agent_wake`, `agent_feed`, `inbox`) or external channels defined in `config.toml` and registered in `CHANNELS.yml`.
