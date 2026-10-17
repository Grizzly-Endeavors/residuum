# Heartbeats

Heartbeats are periodic background checks defined in `HEARTBEAT.yml`. The pulse scheduler evaluates them on a 60-second tick and forks a `scheduled` session for each due pulse.

## Built-in Pulses

Three pulses ship enabled by default in every workspace's `HEARTBEAT.yml`:

| Pulse | Schedule | Agent | What it does |
|-------|----------|-------|---------------|
| `reflection` | `7d` | `introspection` | Reviews recent episodes/observations for patterns (repeated manual tasks, recurring topics, unfinished requests, friction) and delivers findings via `user_inbox_add`. |
| `memory_tending` | `24h`, active `02:00-06:00` | `wiki` | Ingests episodes since the last `ingest` entry in `team/wiki/log.md` into team wiki pages and `team/USER.md` — adds durable facts, corrects or removes stale entries, and maintains the `team/USER.md` core-facts list (≤15 entries, replace-don't-append, ≥2 episodes to promote a page from `draft` to `stable`). |
| `wiki_lint` | `7d`, active `02:00-06:00` | `wiki` | Audits the team wiki for index drift, missing frontmatter, stale pages, old drafts, duplicates, contradictions, and missing links; fixes each problem in place; its summary is filed like any pulse result. |

`reflection` routes to the `introspection` skill (see `team/skills/introspection/SKILL.md`) with `model_tier: large`. Every session fork carries SOUL.md and the team's AGENTS.md in its system message, so `introspection` has the identity context it needs to judge what's worth surfacing without any special option — it can only *propose* SOUL.md and team/AGENTS.md changes via its inbox delivery, never edit them directly. `memory_tending` and `wiki_lint` route to the `wiki` skill (see `team/skills/wiki/SKILL.md`) with `model_tier: large`, and may edit team wiki pages and `team/USER.md` directly.

To disable either, set `enabled: false` on the pulse (don't delete it — the block documents what it does). To tune frequency or scope, edit the `schedule`, `active_hours`, or task prompts directly. A commented-out block of additional starter pulses (`inbox_check`, `morning_briefing`, `nightly_review`) follows the built-ins in the default file — optional add-ons, not enabled by default.

## HEARTBEAT.yml Format

```yaml
pulses:
  - name: check-inbox
    enabled: true
    schedule: 30m            # Duration: "30s", "5m", "2h", "1d"
    active_hours: "09:00-17:00"  # Optional — HH:MM-HH:MM window
    agent: ~                 # null → session with no skill
    model_tier: small        # Optional — small (default), medium, large, or a provider/model id
    tasks:
      - name: check_inbox
        prompt: "Check inbox for new items and summarize anything unread."

  - name: monitor-deploys
    enabled: true
    schedule: 1h
    agent: deploy-watcher    # Any string names a skill (yours or the team's), at the pulse's model_tier
    tasks:
      - name: check_status
        prompt: "Check deployment status."
```

## Output Chaining (`context_from`)

Set `context_from: <pulse_name>` on a pulse to have that named pulse's most recent delivered output injected into this pulse's prompt at fire time, under a `## Context from '<pulse_name>'` section — useful for simple pipelines (a nightly collector feeding a weekly summarizer) without a shared file as a hand-off point.

- "Delivered output" is the source pulse's last **completed** run whose summary wasn't `HEARTBEAT_OK`; a failed or cancelled run never updates it. It's recorded as each pulse's result arrives, so it reflects the source pulse's last successful, non-silent completion, however long ago.
- If the named pulse doesn't exist, has never fired, or has never produced a non-`HEARTBEAT_OK` result, nothing is injected — the prompt says so explicitly instead of silently omitting the section.
- Single-hop only: no chain resolution — only the named pulse's own stored output is used, even if that pulse itself sets `context_from`.
- Persisted alongside `last_run` in `pulse_state.json`, survives restarts, and is pruned when the source pulse is removed from HEARTBEAT.yml.

## Task Scripts

A task can set `script`, a shell command run when the pulse fires, before the session starts. Use it for the mechanical part of a check (fetch, diff, compute) so the model only reasons over what the script found — or, with `script_only: true`, so no model runs at all.

```yaml
pulses:
  - name: feed_watch
    schedule: 1h
    tasks:
      - name: new_posts
        script: "python3 scripts/new_posts.py"
        prompt: "Summarize any new posts below. If there are none, respond with HEARTBEAT_OK."

  - name: disk_space
    schedule: 30m
    tasks:
      - name: root_volume
        script: "df -h / | awk 'NR==2 && $5+0 > 90 {print \"Root volume at \" $5; print \"HEARTBEAT_URGENT\"}'"
        script_only: true      # no session; output is the result
        script_timeout_secs: 10  # default 60
        # script_max_output_kb: 64  (default)
```

- Scripts run in task order through the platform shell (`sh -c`, or `cmd /S /C` on Windows) from the workspace root, with the same `PATH` as your `exec` tool and with your own privileges — anything your `exec` tool can do, a pulse script can do. Agent keys are not passed in as environment variables; agent-key values in the output are redacted.
- Without `script_only`, the script's stdout is appended to its task's section of the session prompt (a task with a `script` doesn't need a `prompt`).
- With `script_only: true` on every task, no session runs. Empty output (or output ending in `HEARTBEAT_OK`) is silent; anything else is filed to the inbox as printed, and pushed to every notification channel when it ends with `HEARTBEAT_URGENT`. `agent`, `model_tier`, `context_from` and task prompts have no effect on such a pulse; setting them still loads the pulse but raises a warning (a diagnostic, or a reload note listing settings with no effect). Its fire shows as running in the Schedule place while its scripts run, and its outcome (`ok`, silent fires too; `failed`; `stopped`) is the pulse's last run, recorded in `pulse_state.json` under `last_fire`.
- Stdout past `script_max_output_kb` (default 64 KiB) is cut, with a line saying so.
- The time limit (default 60 s) covers background processes the script leaves holding its output; at the limit the whole process tree is killed.
- A non-zero exit, a timeout, or a script that can't be run is always reported: in a pulse with a session, the owner gets an inbox item (the reason, command, exit code and the last 2 KiB of stderr) and an owner notice, and the session still runs, told the script failed; in a `script_only` pulse, the result is a failed run, filed to the inbox with the reason and stderr tail, plus the usual failed-pulse notice.
- Stopping the agent kills running scripts and delivers nothing for that fire. One fire can also be stopped on its own from the Schedule place (`POST /api/agents/<agent>/scheduled/pulses/<pulse>/fires/<fire_id>/stop`): its running script is killed, it starts no session (or its session is stopped), and it is recorded as stopped.
- Load problems (the pulse doesn't run, and you get a diagnostic or reload note): `script_only` without `script`, an empty `script`, a `0` timeout or output limit, a task with neither `prompt` nor `script`, or a pulse mixing `script_only` tasks with session tasks — put those in separate pulses.

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

| Value | Execution |
|-------|-----------|
| `~` (null) | Session with no skill |
| `"<skill-name>"` | Session with that skill (agent or team) |

Either way the session runs on the pulse's `model_tier`: a tier (`small`, the default, `medium`, `large`) or one explicit model in `provider/model` form, the format `providers.toml` uses. An explicit model runs with no failover chain. A `model_tier` that is neither fails that pulse's load; one naming a provider `providers.toml` doesn't have skips the pulse, reported as a HEARTBEAT.yml problem, until the provider exists.

`agent: "main"` is removed: every session fork already carries the main agent's identity and a memory snapshot, so there is no separate "run on main" mode. A pulse still using `agent: "main"`, or setting `include_identity` (also removed), fails to load with an error naming the pulse. Rejection is also raised as an owner-facing notice (a web UI toast) naming every currently rejected pulse, the field to remove, and a link to `migrating-to-agent-sessions.md` — fired once when a pulse first becomes rejected, and again if the rejected set changes, not on every tick.

## Diagnostics

Editing `HEARTBEAT.yml` via `write_file`/`edit_file`, the Files editor, or `POST /api/agents/<agent>/workspace/validate` reports invalid YAML, a non-list `pulses` key, a pulse entry that fails to deserialize on its own, a removed option, tasks whose script options don't fit together, or a duplicate pulse name as a diagnostic alongside the save — one per problem, so one bad pulse doesn't hide problems in the others — and the write always goes through rather than being rejected.

## Behavior

- If you (the agent) just wrote `HEARTBEAT.yml` yourself, its outcome on the very next scheduler tick arrives as a system note in your own transcript — "reloaded cleanly", the current problem list, or a parse-failure message — so you don't need to ask the user to check for you. A tick that isn't the result of your own write never sends you this note.
- The scheduler **hot-reloads** `HEARTBEAT.yml` on every tick — edits take effect without restart. The file is parsed generically first, then each pulse entry is deserialized on its own: one bad pulse (a rejected option, a bad field type) is dropped individually and reported as a per-pulse problem, while every other pulse in the file still loads. It's also fully re-validated on every tick — a rejected pulse, a duplicate pulse name, an unparseable `schedule`/`active_hours` string, or a pulse that fails to deserialize — but each is only logged and notified about when the problem set actually changes, not on every tick of an unchanged file. A removed-option rejection links `migrating-to-agent-sessions.md`; every other kind of problem links this doc instead. A whole-document YAML syntax error (as opposed to one bad pulse in an otherwise-valid file) keeps the last pulse set that loaded successfully running, rather than firing nothing until it's fixed, and notifies the owner with the parse error (deduped the same way).
- A pulse fires **immediately on first run** after startup (no wait for the first interval).
- Last-run timestamps are persisted to `pulse_state.json`, so pulses resume their schedule across restarts. It also holds each pulse's last delivered output (`last_output`) and the outcome of its last fire that ran no session (`last_fire`).
- Disabled pulses (`enabled: false`) are skipped entirely.
- A pulse that comes due while an earlier fire of its own is still in flight (in its scripts, waiting for its session, or with its session live) starts anyway — never skipped, blocked, or cancelled — but is flagged as overlapping: visible on the pulse in the web UI's Schedule place and on the run itself, and logged at `info!` with the pulse name, the earlier fire's id and run id, and how long it had been going. No separate notice for this; the UI flag is the signal.
- Each task in `tasks` is an object with `name` (string) and `prompt` (string), and may also run a `script` (see "Task Scripts" below). Task prompts are joined into the session's prompt.
- Pulse sessions include a `"HEARTBEAT_OK"` instruction: the agent should respond with just that phrase if there is nothing to report. These results are silently discarded before reaching the notification router.
- Every pulse run is framed as **autonomous** in its prompt: no user is present, so it must not wait on a question, and it must not create/modify pulses itself. A pulse that concludes a new pulse is warranted should say so via the user inbox, not edit `HEARTBEAT.yml`.
- A failed pulse run's inbox item names the failure reason directly (never just a blank body), and also publishes its own owner-facing notice separate from the inbox item.

## Schedule Place

The web UI's Schedule place (under each agent in the rail, `/agent/<agent>/schedule`) lists every pulse by its name in words ("Inbox check") — how often it runs, active hours, skill, an on/off switch, when it runs next, last outcome (with error if failed), current run ("Running scripts", "Starting session" or "Running session", when it started, an Open session link once it has a session, any overlap, and Stop), and per-pulse loading problems — and every pending scheduled action with Cancel, and Stop for its run in flight. Stop on a pulse's run stops that one fire; an action's run, or a pulse session no fire tracks, stops through its session. It takes each `pulse_fire_changed` frame's run in flight and outcome at once, and refetches when HEARTBEAT.yml or scheduled_actions.json changes, when a scheduled run starts or ends, and after each fire frame. The schedule endpoints answer only while the agent runs, so a stopped agent's Schedule offers Start instead. A pulse's next fire time is the first moment at or after the later of now and the end of its `schedule` since its last run that falls inside its `active_hours`; it is `null` for a disabled pulse and for every pulse when `[pulse] enabled` is `false` in the agent's `config.toml`. Toggling a pulse's enabled switch edits just that value in HEARTBEAT.yml, leaving the rest of the file untouched. Backed by `GET/PUT /api/agents/<agent>/scheduled/pulses...` (with `POST .../pulses/<pulse>/fires/<fire_id>/stop` to stop one fire) and `GET/DELETE /api/agents/<agent>/scheduled/actions...`; each fire's start, phase change and finish also reaches the agent's socket as a `pulse_fire_changed` frame.

## Gotchas

- If multiple pulses are due simultaneously, they all fire (subject to background task concurrency limits).
- The 60-second tick means schedule precision is at best ~1 minute.
