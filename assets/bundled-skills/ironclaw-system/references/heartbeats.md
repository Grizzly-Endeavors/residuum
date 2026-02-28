# Heartbeats

Heartbeats are periodic background checks defined in `HEARTBEAT.yml`. The pulse scheduler evaluates them on a 60-second tick and fires due pulses as background tasks.

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
    agent: deploy-watcher    # Any other string → SubAgent with named preset from subagents/
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
| `"<preset-name>"` | SubAgent with preset from `subagents/` | Small (default) |

## Behavior

- The scheduler **hot-reloads** `HEARTBEAT.yml` on every tick — edits take effect without restart.
- A pulse fires **immediately on first run** after startup (no wait for the first interval).
- Last-run timestamps are in-memory only; they reset on restart.
- Disabled pulses (`enabled: false`) are skipped entirely.
- Each task in `tasks` is an object with `name` (string) and `prompt` (string). Task prompts are joined into the SubAgent prompt.
- SubAgent pulses include a `"HEARTBEAT_OK"` instruction: the agent should respond with just that phrase if there is nothing to report.

## Gotchas

- If multiple pulses are due simultaneously, they all fire (subject to background task concurrency limits).
- The 60-second tick means schedule precision is at best ~1 minute.
- Main-turn pulses (`agent: "main"`) wake the main agent and inject a turn — use sparingly to avoid interrupting user conversations.
