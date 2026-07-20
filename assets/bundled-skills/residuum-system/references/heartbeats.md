# Heartbeats

Heartbeats are periodic background checks defined in `HEARTBEAT.yml`. The pulse scheduler evaluates them on a 60-second tick and fires due pulses as background tasks.

## Built-in Pulses

Two pulses ship enabled by default in every workspace's `HEARTBEAT.yml`:

| Pulse | Schedule | Agent | What it does |
|-------|----------|-------|---------------|
| `reflection` | `7d` | `introspection` | Reviews recent episodes/observations for patterns (repeated manual tasks, recurring topics, unfinished requests, friction) and delivers findings via `user_inbox_add`. |
| `memory_tending` | `24h`, active `02:00-06:00` | `introspection` | Reconciles `MEMORY.md`/`USER.md` against recent episode evidence — adds durable facts, corrects or removes stale entries, and maintains the `USER.md` Core Facts tier (≤15 entries, replace-don't-append, ≥2 observations to promote). |

Both route to the `introspection` subagent preset (see `subagents/introspection.md`), which runs at `model_tier: large` and includes full identity context (`include_identity: true`) so it has SOUL.md/AGENTS.md/MEMORY.md available when judging what to tend. It edits MEMORY.md/USER.md directly but only *proposes* SOUL.md/AGENTS.md changes in its inbox delivery — it cannot edit those files itself.

To disable either, set `enabled: false` on the pulse (don't delete it — the block documents what it does). To tune frequency or scope, edit the `schedule`, `active_hours`, or task prompts directly. A commented-out block of additional starter pulses (`inbox_check`, `morning_briefing`, `nightly_review`) follows the built-ins in the default file — optional add-ons, not enabled by default.

## HEARTBEAT.yml Format

```yaml
pulses:
  - name: check-inbox
    enabled: true
    schedule: 30m            # Duration: "30s", "5m", "2h", "1d"
    active_hours: "09:00-17:00"  # Optional — HH:MM-HH:MM window
    agent: ~                 # null → SubAgent (Small tier)
    tasks:
      - name: check_inbox
        prompt: "Check inbox for new items and summarize anything unread."

  - name: daily-review
    enabled: true
    schedule: 1d
    active_hours: "08:00-09:00"
    agent: main              # "main" → MainWakeTurn (runs on main agent)
    tasks:
      - name: morning_plan
        prompt: "Review memory and plan for today."

  - name: monitor-deploys
    enabled: true
    schedule: 1h
    agent: deploy-watcher    # Any other string → SubAgent with named preset from subagents/, using the preset's own model_tier
    trigger_count: 5         # Max 5 firings per active period
    tasks:
      - name: check_status
        prompt: "Check deployment status."
```

## Schedule Parsing

Durations are a number followed by a unit suffix:

| Suffix | Unit |
|--------|------|
| `s` | seconds |
| `m` | minutes |
| `h` | hours |
| `d` | days |

Multi-day intervals work the same way — `"7d"` for a weekly pulse, e.g.

## Active Hours

- Format: `"HH:MM-HH:MM"` in the configured timezone.
- Supports overnight windows: `"22:00-06:00"` means 10 PM to 6 AM.
- If omitted, the pulse can fire at any time.

## Execution Routing

The `agent` field controls how the pulse executes:

| Value | Execution | Model Tier |
|-------|-----------|------------|
| `~` (null) | SubAgent | Small |
| `"main"` | MainWakeTurn (main agent conversation) | Main model |
| `"<preset-name>"` | SubAgent with preset from `subagents/` | Preset's own `model_tier` (from its frontmatter) |

## Behavior

- The scheduler **hot-reloads** `HEARTBEAT.yml` on every tick — edits take effect without restart.
- A pulse fires **immediately on first run** after startup (no wait for the first interval).
- Last-run timestamps and run counts are persisted to `pulse_state.json`, so pulses resume their schedule across restarts.
- Disabled pulses (`enabled: false`) are skipped entirely.
- Each task in `tasks` is an object with `name` (string) and `prompt` (string). Task prompts are joined into the SubAgent prompt.
- SubAgent pulses include a `"HEARTBEAT_OK"` instruction: the agent should respond with just that phrase if there is nothing to report. These results are silently discarded before reaching the notification router.
- `trigger_count` limits how many times a pulse fires within its `active_hours` window. When set, firings are spaced evenly across the active period. Omit for unlimited.
- Every pulse run is framed as **autonomous** in its prompt: no user is present, so it must not wait on a question, and it must not create/modify pulses or schedule further background work itself. A pulse that concludes a new pulse is warranted should say so via the user inbox, not edit `HEARTBEAT.yml`.

## Gotchas

- If multiple pulses are due simultaneously, they all fire (subject to background task concurrency limits).
- The 60-second tick means schedule precision is at best ~1 minute.
- Main-turn pulses (`agent: "main"`) wake the main agent and inject a turn — use sparingly to avoid interrupting user conversations.
