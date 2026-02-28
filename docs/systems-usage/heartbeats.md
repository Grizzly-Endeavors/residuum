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
| `name` | string | yes | Identifies the pulse for NOTIFY.yml routing |
| `enabled` | boolean | no | Default `true`. Set `false` to pause without deleting. |
| `schedule` | string | yes | Duration: `"30s"`, `"5m"`, `"2h"`, `"1d"` |
| `active_hours` | string | no | `"HH:MM-HH:MM"` in configured timezone. Supports overnight windows (e.g. `"22:00-06:00"`). |
| `agent` | string or null | no | See agent routing table below. |
| `tasks` | array of objects | yes | Each task has `name` (string) and `prompt` (string). |

### Agent Routing

| Value | Execution | Model Tier |
|-------|-----------|------------|
| `~` (null / omitted) | Sub-agent | Small |
| `"main"` | Main agent wake turn | Main model |
| `"<preset-name>"` | Sub-agent with named preset from `subagents/` | Preset's tier (default: small) |

**Use `"main"` sparingly** — it wakes the main agent and injects a full turn. Reserve for tasks that need conversation context or should produce a visible response.

### HEARTBEAT_OK Convention

Sub-agent pulses include an instruction: if nothing actionable was found, return the exact string `HEARTBEAT_OK`. Results containing this string are silently discarded and never routed, regardless of NOTIFY.yml configuration.

## Scheduling Behavior

- The scheduler runs on a **60-second tick**, so precision is at best ~1 minute
- HEARTBEAT.yml is **hot-reloaded** on every tick — changes take effect without restarting the gateway
- Last-run timestamps should be persisted to disk so pulses resume their schedule across gateway restarts *(currently in-memory only — persistence is a pending code change)*
- Multiple due pulses all fire simultaneously (subject to `max_concurrent` from `[background]` config)

## Result Routing

Pulse results are routed through NOTIFY.yml by **pulse name**. A pulse named `email_check` routes to whichever channels list `email_check` in their NOTIFY.yml entries.

If a pulse name doesn't appear in any NOTIFY.yml channel, its results are silently dropped (in addition to HEARTBEAT_OK results).

See [notifications.md](notifications.md) for the full routing model.
