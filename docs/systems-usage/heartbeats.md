# Heartbeats

Heartbeats are ambient scheduled checks the agent performs in the background. The gateway handles all scheduling; the LLM is only invoked when a pulse is due.

## Built-in Pulses

Every bootstrapped workspace ships `HEARTBEAT.yml` with three pulses enabled by default:

| Pulse | Schedule | Agent | Purpose |
|-------|----------|-------|---------|
| `reflection` | `"7d"` | `introspection` | Reviews recent episodes/observations for recurring patterns, unfinished requests, and friction; delivers suggestions to the user inbox via `user_inbox_add`. |
| `memory_tending` | `"24h"`, active `02:00-06:00` | `wiki` | Ingests episodes since the last `ingest` entry in `wiki/log.md` into wiki pages and `USER.md` — adds durable facts, corrects or removes stale entries, and maintains the `USER.md` core-facts list (capped ~15 entries, replace-don't-append). A page is created with `status: draft` on a single supporting episode and promoted to `stable` once a second independent episode corroborates it. See [wiki.md](wiki.md) for the page format and promotion rule. |
| `wiki_lint` | `"7d"`, active `02:00-06:00` | `wiki` | Audits the wiki for index drift, missing frontmatter, stale pages (past `stale_after`), old drafts, duplicates, contradictions between pages, and missing links; fixes each problem in place; its summary is filed like any pulse result. |

`reflection` names the bundled `introspection` skill (`skills/introspection/SKILL.md`) with `model_tier: large`. Every session fork carries SOUL.md/AGENTS.md in its system message now, so `introspection` has the identity context it needs to judge what's worth surfacing without any special option — it can only propose SOUL.md/AGENTS.md changes through its inbox delivery, never edit them directly. `memory_tending` and `wiki_lint` name the bundled `wiki` skill (`skills/wiki/SKILL.md`) with `model_tier: large`, and may edit wiki pages and `USER.md` directly.

Disabling either is a matter of setting `enabled: false` on the pulse — the user or agent can do this during onboarding if the user opts out of background self-maintenance. A commented-out block of additional starter pulses (`inbox_check`, `morning_briefing`, `nightly_review`) ships alongside the built-ins as optional, off-by-default add-ons.

## HEARTBEAT.yml

The agent owns this file and evolves it over time — adding new pulses, adjusting schedules, disabling noisy ones, changing routing.

```yaml
pulses:
  - name: email_check
    enabled: true
    schedule: "30m"
    active_hours: "08:00-18:00"
    agent: ~                        # null = plain session, small tier
    tasks:
      - name: check_inbox
        prompt: "Check my email for urgent messages. Report anything requiring action."

  - name: deploy_watch
    enabled: true
    schedule: "5m"
    agent: deploy-watcher           # named skill from skills/
    tasks:
      - name: check_status
        prompt: "Check deployment pipeline status. Report failures."
```

### Fields

| Field | Type | Required | Notes |
|-------|------|----------|-------|
| `name` | string | yes | Identifies the pulse |
| `enabled` | boolean | no | Default `true`. Set `false` to pause without deleting. |
| `schedule` | string | yes | Duration: `"30s"`, `"5m"`, `"2h"`, `"1d"`, `"7d"` — any number plus `s`/`m`/`h`/`d` |
| `active_hours` | string | no | `"HH:MM-HH:MM"` in configured timezone. Supports overnight windows (e.g. `"22:00-06:00"`). |
| `agent` | string or null | no | See agent routing table below. |
| `tasks` | array of objects | yes | Each task has `name` (string) and `prompt` (string). |

### Agent Routing

| Value | Execution | Model Tier |
|-------|-----------|------------|
| `~` (null / omitted) | Session with no skill | Small |
| `"<skill-name>"` | Session with that skill activated as its role | The pulse's `model_tier` (default: small) |

`agent: "main"` is removed: every session fork already carries the main agent's identity and a snapshot of its memory, so there is no separate "run on main" mode. A pulse still using `agent: "main"`, or setting `include_identity` (also removed), fails to load with an error naming the pulse — it is never silently reinterpreted as something else. This is logged (`residuum logs --level error`) and also raised as an owner-facing notice (a toast in the web UI, and the same message on any chat interface, via the bus's system notification channel) naming every currently rejected pulse, the field to remove, and a link to [`migrating-to-agent-sessions.md`](../guides/migrating-to-agent-sessions.md). The notice fires once when a pulse first becomes rejected and again if the rejected set changes on a later edit — not on every scheduler tick, even though HEARTBEAT.yml re-validates on each one (see "Scheduling Behavior" below).

### HEARTBEAT_OK Convention

A pulse's session prompt includes an instruction: if nothing actionable was found, return the exact string `HEARTBEAT_OK`. Results containing this string are silently discarded before reaching the notification router.

### Autonomous Framing

Every pulse-triggered session run is framed in its prompt as autonomous: no user is present to answer a question, so the run must not pause waiting on one. Pulse prompts also explicitly forbid the run from creating or modifying pulses — a pulse that could edit other pulses risks a runaway self-scheduling loop with no user in the loop to notice. If a pulse run concludes that a new or different pulse is warranted, the correct move is to say so via the user inbox, not to write `HEARTBEAT.yml` itself.

## Scheduling Behavior

- The scheduler runs on a **60-second tick**, so precision is at best ~1 minute
- HEARTBEAT.yml is **hot-reloaded** on every tick — changes take effect without restarting the gateway. The file is parsed generically first, then each pulse entry is deserialized on its own: a pulse with a bad field (e.g. `schedule` given as something other than a string) is dropped individually and reported as a per-pulse problem, while every other pulse in the file still loads. It's also fully re-validated on every tick — a pulse rejected for using a removed option (see "Agent Routing" above), a duplicate pulse name (dropped, keeping the first definition), an unparseable `schedule`/`active_hours` string (that pulse is skipped), or a pulse entry that fails to deserialize — but each is only logged and notified about when the problem set actually changes, not on every tick of an unchanged, still-broken file, otherwise a single bad pulse would log and toast once a minute for as long as it stays broken. A removed-option problem is logged at `error!` and links [`migrating-to-agent-sessions.md`](../guides/migrating-to-agent-sessions.md); every other kind of problem is logged at `warn!` and links this doc instead, since none of them have anything to do with the migration.
- A whole-document YAML syntax error (the file itself doesn't parse, as opposed to one bad pulse entry within an otherwise valid document) keeps the last pulse set that loaded successfully running, rather than firing nothing until the file is fixed. The owner is notified with the parse error, once per distinct error rather than every tick, the same dedup rule as any other HEARTBEAT.yml problem.
- Last-run timestamps are persisted to `pulse_state.json` in the workspace, so pulses resume their schedule across gateway restarts. Missing or corrupt state files are treated as empty state (logged at warn level).
- Multiple due pulses all fire simultaneously (subject to `max_concurrent` from `[background]` config)
- A pulse that comes due while its own previous run is still going (a slow run overlapping the next scheduled fire) starts anyway — it is never skipped, blocked, or cancelled for this — but the new run is flagged as overlapping: visible on the pulse in the web UI's Scheduled view and on the run itself in the session view, and logged at `info!` with the pulse name, the previous run's id, and how long that previous run had been going. There is no separate owner notice for an overlap; the flag in the UI is the signal.

## Scheduled View

The web UI's Scheduled view (opened from the hamburger menu, and at `/scheduled`) lists every pulse: its schedule, active hours, agent/skill, enabled flag, estimated next fire time, last outcome (with the time and, for a failure, the error), whether it's currently running (and whether that run is overlapping the previous one), and any HEARTBEAT.yml loading problems naming it. Toggling a pulse's enabled switch flips the `enabled` value for that one pulse in HEARTBEAT.yml in place — the rest of the file, including comments and formatting, is left byte-for-byte untouched. The same view lists pending scheduled actions (see [scheduled-actions.md](scheduled-actions.md)) with a cancel button for each.

Backed by `GET /api/scheduled/pulses`, `PUT /api/scheduled/pulses/{name}/enabled`, `GET /api/scheduled/actions`, and `DELETE /api/scheduled/actions/{id}`. The view refetches on a `workspace_changed` frame naming `HEARTBEAT.yml` or `scheduled_actions.json`, and on any `scheduled`-category session lifecycle frame, rather than polling.

## Result Routing

Pulse results flow through the pub/sub bus to the notification router, which delivers each result according to the disposition the session declared. A summary containing `HEARTBEAT_OK` is discarded; one containing `HEARTBEAT_URGENT` is pushed to every configured notification channel as well as filed to the inbox; anything else goes to the inbox alone.

See [notifications.md](notifications.md) for the full routing model.
