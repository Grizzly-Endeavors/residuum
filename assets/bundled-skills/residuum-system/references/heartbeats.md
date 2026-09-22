# Heartbeats

Heartbeats are periodic background checks defined in `HEARTBEAT.yml`. The pulse scheduler evaluates them on a 60-second tick and forks a `scheduled` session for each due pulse.

## Built-in Pulses

Three pulses ship enabled by default in every workspace's `HEARTBEAT.yml`:

| Pulse | Schedule | Agent | What it does |
|-------|----------|-------|---------------|
| `reflection` | `7d` | `introspection` | Reviews recent episodes/observations for patterns (repeated manual tasks, recurring topics, unfinished requests, friction) and delivers findings via `user_inbox_add`. |
| `memory_tending` | `24h`, active `02:00-06:00` | `wiki` | Ingests episodes since the last `ingest` entry in `wiki/log.md` into wiki pages and `USER.md` — adds durable facts, corrects or removes stale entries, and maintains the `USER.md` core-facts list (≤15 entries, replace-don't-append, ≥2 episodes to promote a page from `draft` to `stable`). |
| `wiki_lint` | `7d`, active `02:00-06:00` | `wiki` | Audits the wiki for index drift, missing frontmatter, stale pages, old drafts, duplicates, contradictions, and missing links; fixes each problem in place; its summary is filed like any pulse result. |

`reflection` routes to the `introspection` skill (see `skills/introspection/SKILL.md`) with `model_tier: large`. Every session fork carries SOUL.md/AGENTS.md in its system message now, so `introspection` has the identity context it needs to judge what's worth surfacing without any special option — it can only *propose* SOUL.md/AGENTS.md changes via its inbox delivery, never edit them directly. `memory_tending` and `wiki_lint` route to the `wiki` skill (see `skills/wiki/SKILL.md`) with `model_tier: large`, and may edit wiki pages and `USER.md` directly.

To disable either, set `enabled: false` on the pulse (don't delete it — the block documents what it does). To tune frequency or scope, edit the `schedule`, `active_hours`, or task prompts directly. A commented-out block of additional starter pulses (`inbox_check`, `morning_briefing`, `nightly_review`) follows the built-ins in the default file — optional add-ons, not enabled by default.

## HEARTBEAT.yml Format

```yaml
pulses:
  - name: check-inbox
    enabled: true
    schedule: 30m            # Duration: "30s", "5m", "2h", "1d"
    active_hours: "09:00-17:00"  # Optional — HH:MM-HH:MM window
    agent: ~                 # null → session with no skill (Small tier)
    tasks:
      - name: check_inbox
        prompt: "Check inbox for new items and summarize anything unread."

  - name: monitor-deploys
    enabled: true
    schedule: 1h
    agent: deploy-watcher    # Any string names a skill from skills/, at the pulse's model_tier
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
| `~` (null) | Session with no skill | Small |
| `"<skill-name>"` | Session with that skill from `skills/` | The pulse's `model_tier` (default: small) |

`agent: "main"` is removed: every session fork already carries the main agent's identity and a memory snapshot, so there is no separate "run on main" mode. A pulse still using `agent: "main"`, or setting `include_identity` (also removed), fails to load with an error naming the pulse. Rejection is also raised as an owner-facing notice (a web UI toast and the same message on any chat interface) naming every currently rejected pulse, the field to remove, and a link to `migrating-to-agent-sessions.md` — fired once when a pulse first becomes rejected, and again if the rejected set changes, not on every tick.

## Behavior

- The scheduler **hot-reloads** `HEARTBEAT.yml` on every tick — edits take effect without restart. It's also fully re-validated on every tick — a rejected pulse, a duplicate pulse name, or an unparseable `schedule`/`active_hours` string — but each is only logged and notified about when the problem set actually changes, not on every tick of an unchanged file. A removed-option rejection links `migrating-to-agent-sessions.md`; a duplicate name or bad schedule/active_hours links this doc instead.
- A pulse fires **immediately on first run** after startup (no wait for the first interval).
- Last-run timestamps are persisted to `pulse_state.json`, so pulses resume their schedule across restarts.
- Disabled pulses (`enabled: false`) are skipped entirely.
- Each task in `tasks` is an object with `name` (string) and `prompt` (string). Task prompts are joined into the session's prompt.
- Pulse sessions include a `"HEARTBEAT_OK"` instruction: the agent should respond with just that phrase if there is nothing to report. These results are silently discarded before reaching the notification router.
- Every pulse run is framed as **autonomous** in its prompt: no user is present, so it must not wait on a question, and it must not create/modify pulses itself. A pulse that concludes a new pulse is warranted should say so via the user inbox, not edit `HEARTBEAT.yml`.

## Gotchas

- If multiple pulses are due simultaneously, they all fire (subject to background task concurrency limits).
- The 60-second tick means schedule precision is at best ~1 minute.
