# Heartbeats

Heartbeats are ambient scheduled checks the agent performs in the background. The gateway handles all scheduling; the LLM is only invoked when a pulse is due, and not at all for a pulse whose tasks are all `script_only` (see "Task Scripts" below).

## Built-in Pulses

The first agent's `HEARTBEAT.yml` ships with three pulses enabled by default. Agents created later start from a template with `reflection` and `memory_tending` only: `wiki_lint` checks the shared team wiki, so one agent runs it, and moving it to another agent means editing both agents' `HEARTBEAT.yml`.

| Pulse | Schedule | Agent | Purpose |
|-------|----------|-------|---------|
| `reflection` | `"7d"` | `introspection` | Reviews recent episodes/observations for recurring patterns, unfinished requests, and friction; delivers suggestions to the user inbox via `user_inbox_add`. |
| `memory_tending` | `"24h"`, active `02:00-06:00` | `wiki` | Ingests episodes since the last `ingest` entry in `team/wiki/log.md` into team wiki pages and `team/USER.md` — adds durable facts, corrects or removes stale entries, and maintains the `team/USER.md` core-facts list (capped ~15 entries, replace-don't-append). A page is created with `status: draft` on a single supporting episode and promoted to `stable` once a second independent episode corroborates it. See [wiki.md](wiki.md) for the page format and promotion rule. |
| `wiki_lint` | `"7d"`, active `02:00-06:00` | `wiki` | Audits the team wiki for index drift, missing frontmatter, stale pages (past `stale_after`), old drafts, duplicates, contradictions between pages, and missing links; fixes each problem in place; its summary is filed like any pulse result. |

`reflection` names the bundled `introspection` skill (`team/skills/introspection/SKILL.md`) with `model_tier: large`. Every session fork carries SOUL.md and the team's AGENTS.md in its system message, so `introspection` has the identity context it needs to judge what's worth surfacing without any special option — it can only propose SOUL.md/AGENTS.md changes through its inbox delivery, never edit them directly. `memory_tending` and `wiki_lint` name the bundled `wiki` skill (`team/skills/wiki/SKILL.md`) with `model_tier: large`, and may edit wiki pages and `USER.md` directly.

Disabling either is a matter of setting `enabled: false` on the pulse — the user or agent can do this during onboarding if the user opts out of background self-maintenance. A commented-out block of additional starter pulses (`inbox_check`, `morning_briefing`, `nightly_review`) ships alongside the built-ins as optional, off-by-default add-ons.

## HEARTBEAT.yml

The agent owns this file and evolves it over time — adding new pulses, adjusting schedules, disabling noisy ones, changing routing.

```yaml
pulses:
  - name: email_check
    enabled: true
    schedule: "30m"
    active_hours: "08:00-18:00"
    agent: ~                        # null = plain session
    model_tier: anthropic/claude-haiku-4-5   # a tier, or one explicit provider/model
    tasks:
      - name: check_inbox
        prompt: "Check my email for urgent messages. Report anything requiring action."

  - name: deploy_watch
    enabled: true
    schedule: "5m"
    agent: deploy-watcher           # named skill (agent or team)
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
| `model_tier` | string | no | The session's model: a tier (`small`, `medium`, `large`) or an explicit `provider/model` id, the format `providers.toml` uses. Default: `small`. See [Models](background-tasks.md#models). A value that is neither fails this pulse's load; an id whose provider isn't configured skips the pulse until it is. |
| `context_from` | string | no | Name of another pulse. Injects that pulse's most recent delivered output into this pulse's prompt at fire time. See "Output Chaining (`context_from`)" below. |
| `tasks` | array of objects | yes | Each task has a `name` (string) and a `prompt` (string), and may run a `script`. See "Task Fields" below. |

### Task Fields

| Field | Type | Required | Notes |
|-------|------|----------|-------|
| `name` | string | yes | Identifies the task in the session's prompt and in reports. |
| `prompt` | string | unless the task has a `script` | What the session does for this task. Unused by a `script_only` task. |
| `script` | string | no | A shell command run when the pulse fires, before the session starts. Its stdout is appended to this task's prompt. See "Task Scripts" below. |
| `script_only` | boolean | no | Default `false`. Deliver the script's stdout as the pulse's result and run no session. Needs `script`. |
| `script_timeout_secs` | integer | no | Default `60`. How long the script may run before its whole process tree is killed. |
| `script_max_output_kb` | integer | no | Default `64`. How much of the script's stdout is kept, in KiB. |

### Agent Routing

| Value | Execution |
|-------|-----------|
| `~` (null / omitted) | Session with no skill |
| `"<skill-name>"` | Session with that skill activated as its role |

Either way the session runs on the pulse's `model_tier` (default: small).

`agent: "main"` is removed: every session fork already carries the main agent's identity and a snapshot of its memory, so there is no separate "run on main" mode. A pulse still using `agent: "main"`, or setting `include_identity` (also removed), fails to load with an error naming the pulse — it is never silently reinterpreted as something else. This is logged (`residuum logs --level error`) and also raised as an owner-facing notice (a toast in the web UI, via the bus's system notification channel) naming every currently rejected pulse, the field to remove, and a link to [`migrating-to-agent-sessions.md`](../guides/migrating-to-agent-sessions.md). The notice fires once when a pulse first becomes rejected and again if the rejected set changes on a later edit — not on every scheduler tick, even though HEARTBEAT.yml re-validates on each one (see "Scheduling Behavior" below).

### HEARTBEAT_OK Convention

A pulse's session prompt includes an instruction: if nothing actionable was found, return the exact string `HEARTBEAT_OK`. Results ending with this string are silently discarded before reaching the notification router.

### Output Chaining (`context_from`)

A pulse can set `context_from: <pulse_name>` to receive the named pulse's most recent delivered output in its own prompt, under a `## Context from '<pulse_name>'` section — enabling simple pipelines (a nightly collector, then a weekly summarizer that reads its output) without a shared file or wiki page as a hand-off point.

```yaml
pulses:
  - name: nightly_collect
    schedule: "24h"
    tasks:
      - name: collect
        prompt: "Gather today's deployment notes and errors."

  - name: weekly_summary
    schedule: "7d"
    context_from: nightly_collect
    tasks:
      - name: summarize
        prompt: "Summarize the week using the injected context from nightly_collect."
```

- "Delivered output" means the source pulse's last **completed** run whose summary wasn't `HEARTBEAT_OK` — a failed or cancelled run never updates it. This is recorded as each pulse's result arrives, not read live at fire time, so it always reflects the source pulse's last successful, non-silent completion, however long ago that was.
- If the named pulse doesn't exist, has never fired, or has never produced a non-`HEARTBEAT_OK` result, nothing is injected — the prompt says so explicitly (`No prior output from '<name>' is available yet`) rather than silently omitting the section.
- This is single-hop only: only the named pulse's own stored output is used, even if that pulse itself sets `context_from` — there is no chain resolution or cycle handling.
- Each pulse's last delivered output is persisted alongside `last_run` in `pulse_state.json` and survives restarts. It is pruned when the pulse is removed from HEARTBEAT.yml, the same as `last_run`.

### Task Scripts

A task's `script` does the mechanical part of a check (fetch a page, diff a file, compute disk usage) so the model only reasons over what it found, or so no model runs at all. When the pulse fires, its scripts run one after another in task order, off the agent's event loop, before anything else happens for that fire.

```yaml
pulses:
  - name: feed_watch
    schedule: "1h"
    tasks:
      - name: new_posts
        script: "python3 scripts/new_posts.py"
        prompt: "Summarize any new posts below. If there are none, respond with HEARTBEAT_OK."

  - name: disk_space
    schedule: "30m"
    tasks:
      - name: root_volume
        script: "df -h / | awk 'NR==2 && $5+0 > 90 {print \"Root volume at \" $5; print \"HEARTBEAT_URGENT\"}'"
        script_only: true
        script_timeout_secs: 10
```

- **Where and how it runs.** Each script runs (logged at `info` as it starts) through the platform shell (`sh -c` on Linux and macOS, `cmd /S /C` on Windows) from the agent's workspace root, with the same `PATH` as the agent's `exec` tool (the configured tool directories first). It runs with the agent's own privileges, exactly as a command the agent runs with `exec` does, so it can do anything the agent's `exec` tool can. Agent keys are not exposed to it as environment variables, and any agent-key value in its output is redacted before the output goes anywhere.
- **With a session** (`script_only` unset). The script's stdout is appended to its task's section of the session's prompt, after the task's `prompt`; a script that printed nothing is noted as such. The session then runs as usual and decides `HEARTBEAT_OK`, a normal report, or `HEARTBEAT_URGENT`.
- **Without a session** (`script_only: true` on every task). No model runs. When every script printed nothing, or its output ends with `HEARTBEAT_OK`, the fire is silent, the same as a session answering `HEARTBEAT_OK`. Otherwise the output is the pulse's result, filed to the inbox as printed (one task's output on its own; several tasks' output each under the task's name), and it is pushed to every notification channel as well when it ends with `HEARTBEAT_URGENT`. The result counts as the pulse's delivered output for `context_from`. `agent`, `model_tier`, `context_from` and the tasks' `prompt` have no effect on such a pulse; a pulse that sets any of them still runs, and the setting is reported the way a load problem is (a warning diagnostic when the file is edited, a log line and an owner notice listing it under settings with no effect).
- **Output limit.** Stdout past `script_max_output_kb` is read and discarded, and the kept output ends with a line saying it was cut (`[output cut: the script printed more than 64 KiB; only the first 64 KiB is shown]`).
- **Time limit.** The limit covers the whole run, including any process the script started in the background that still holds its output open: when the limit passes, the whole process tree is killed and whatever the script printed is kept. (On Windows, a background process `taskkill` can no longer find once the script's shell has exited may survive the kill; the run still ends at the limit.)
- **Failures.** A script that exits with a non-zero code, runs past `script_timeout_secs`, or can't be run is never silent. It is logged at `warn` with the pulse, the task, the reason, and the last part of stderr (its last 2 KiB, where the final error usually is). In a pulse with a session, the owner gets an inbox item saying which task's script failed and how, followed by the command, the exit code or time limit, and the last part of its error output, and an owner notice in the web UI naming the pulse, the task and the reason, the same two surfaces a failed pulse run gets; the session still runs, and its prompt says the script failed, shows whatever it printed, and the last part of its error output. In a `script_only` pulse, the result is a failed run: its inbox item carries what the script printed and the last part of its error output (with several tasks, also each failed task's name and reason, and the other tasks' output), and the owner gets the usual failed-pulse notice naming the pulse and the reason (see "Result Routing" below).
- **Stopping.** Stopping or restarting the agent kills any script still running, and a fire whose scripts finish once the agent has begun stopping delivers nothing: no result, no failure report, no session, and no recorded outcome. One fire can also be stopped on its own, which kills its running script the same way and delivers nothing, but records the fire as `stopped` (see "Pulse Fires" below).
- **Validation.** A task with `script_only` but no `script`, an empty `script`, a `script_timeout_secs` or `script_max_output_kb` of `0`, a task with neither `prompt` nor `script`, or a pulse that mixes `script_only` tasks with tasks that run a session is a load problem: the pulse doesn't run, and the problem is reported like any other HEARTBEAT.yml problem (a diagnostic when the file is edited, a log line and an owner notice when it loads).
- **Schedule place.** A fire shows as the pulse's current run in the `script` phase while its scripts run. A `script_only` fire's outcome is the pulse's last run: `ok` (a silent fire too), `failed` with the reason, or `stopped`; its results and failures also reach the inbox and notices as described above. See "Pulse Fires" below.

### Autonomous Framing

Every pulse-triggered session run is framed in its prompt as autonomous: no user is present to answer a question, so the run must not pause waiting on one. Pulse prompts also explicitly forbid the run from creating or modifying pulses — a pulse that could edit other pulses risks a runaway self-scheduling loop with no user in the loop to notice. If a pulse run concludes that a new or different pulse is warranted, the correct move is to say so via the user inbox, not to write `HEARTBEAT.yml` itself.

## Diagnostics

Editing `HEARTBEAT.yml` through the agent's `write_file`/`edit_file` tools, the Files editor, or `POST /api/agents/{name}/workspace/validate` reports the same problems `load_heartbeat` would reject or drop, one diagnostic per problem: a whole-document YAML syntax error (with the parser's line/column), a non-list top-level `pulses` key, a pulse entry that fails to deserialize on its own (e.g. a `schedule` given as something other than a string), a pulse using a removed option (`agent: "main"`, `include_identity`), a pulse whose tasks don't fit together (see "Task Scripts" above), or a duplicate pulse name. One bad pulse doesn't hide problems in the others — each gets its own diagnostic naming that pulse. The save always goes through — a diagnostic names the problem instead of the write being rejected, so a mistake is visible immediately rather than only showing up as a scheduler warning after the fact.

## Scheduling Behavior

- The scheduler runs on a **60-second tick**, so precision is at best ~1 minute
- HEARTBEAT.yml is **hot-reloaded** on every tick — changes take effect without restarting the gateway. The file is parsed generically first, then each pulse entry is deserialized on its own: a pulse with a bad field (e.g. `schedule` given as something other than a string) is dropped individually and reported as a per-pulse problem, while every other pulse in the file still loads. It's also fully re-validated on every tick — a pulse rejected for using a removed option (see "Agent Routing" above), a duplicate pulse name (dropped, keeping the first definition), an unparseable `schedule`/`active_hours` string (that pulse is skipped), an explicit `model_tier` whose provider `providers.toml` doesn't have (that pulse is skipped until the model resolves, which a config reload that adds the provider makes happen on the next tick), or a pulse entry that fails to deserialize (including a `model_tier` that is neither a tier nor a `provider/model` id) — but each is only logged and notified about when the problem set actually changes, not on every tick of an unchanged, still-broken file, otherwise a single bad pulse would log and toast once a minute for as long as it stays broken. A removed-option problem is logged at `error!` and links [`migrating-to-agent-sessions.md`](../guides/migrating-to-agent-sessions.md); every other kind of problem is logged at `warn!` and links this doc instead, since none of them have anything to do with the migration.
- A whole-document YAML syntax error (the file itself doesn't parse, as opposed to one bad pulse entry within an otherwise valid document) keeps the last pulse set that loaded successfully running, rather than firing nothing until the file is fixed. The owner is notified with the parse error, once per distinct error rather than every tick, the same dedup rule as any other HEARTBEAT.yml problem.
- Last-run timestamps are persisted to `pulse_state.json` in the workspace, so pulses resume their schedule across gateway restarts. The file also holds each pulse's last delivered output (`last_output`, see "Output Chaining" above) and the outcome of its last fire that ended without a session run (`last_fire`, see "Pulse Fires" below). Missing or corrupt state files are treated as empty state (logged at warn level), and a file without one of these keys loads with that key empty.
- A pulse with no recorded run (a new install, or a pulse just added to HEARTBEAT.yml) does not run on its first tick. The scheduler records it as seen at that moment, persists that time, and runs it one `schedule` later. A pulse outside its `active_hours` is not recorded until a tick evaluates it inside the window.
- Multiple due pulses all fire simultaneously (subject to `max_concurrent` from `[background]` config)
- A pulse that comes due while an earlier fire of its own is still in flight (running its scripts, waiting for its session, or with its session still live) starts anyway — it is never skipped, blocked, or cancelled for this — but the new fire is flagged as overlapping: visible on the pulse in the web UI's Schedule place and on the run itself in the session panel, and logged at `info!` with the pulse name, the earlier fire's id and session run id (`none` while it is still in its scripts), and how long it had been going. A live session of the pulse that no fire tracks (one resumed by a message to its address) counts too. There is no separate owner notice for an overlap; the flag in the UI is the signal.

## Pulse Fires

Each time a pulse comes due, its **fire** is tracked from that moment until it ends: its id (`fire-` and 32 hex digits), when it started, its **phase** — `script` while its task scripts run, `session` once it has asked for its session (with the session's address, and the session's run id once it registers) — and any overlap. A pulse without scripts starts in `session`. Nothing about a fire in flight is persisted, so after a restart no fire is running.

- **How a fire ends.** When its `script_only` result has been delivered, when its session completes, when its session couldn't start (the spawn listener reports the failure, and the fire fails with "the session couldn't start: <reason>"), when it is stopped, or when the agent stops. A fire whose task ends without finishing it is taken out of the tracker anyway and logged at `warn!`, so it never shows as running forever.
- **Outcome.** A fire that ran a session is recorded by the session store, like any session run. A fire that ended without one — a `script_only` fire (silent ones included), one stopped before its session, or one whose session couldn't start — is recorded in `pulse_state.json` under `last_fire`, keyed by pulse name: `{ "status": "ok" | "failed" | "stopped", "at": "<RFC 3339>", "error": "<reason>" | null }`, only the newest per pulse, pruned when the pulse is removed from HEARTBEAT.yml. A pulse's last run in the Schedule place is the newer of the two, with `ok` read as `completed` and `stopped` as `cancelled`. A fire the agent's stop ends records nothing.
- **Stop.** `POST /api/agents/{name}/scheduled/pulses/{pulse}/fires/{fire_id}/stop` stops one fire. During its scripts, the running script's process tree is killed and the fire delivers nothing: no result, no failure report, no session. Once it has asked for its session, that session is stopped through the session registry, as soon as it registers if it hasn't yet. It answers `202` with `{ "pulse": "<pulse>", "fire_id": "<fire_id>" }` once the stop is asked for, and the fire's finish arrives as a `pulse_fire_changed` frame; a fire that has already finished, or never existed, answers `404` with `{ "error": "This run of the pulse \"<pulse>\" isn't running any more." }`. A fire stopped before its session is recorded as `stopped`; a stopped session's run is `cancelled` in the session store. Every fire's stop is a child of the agent's own, so stopping the agent stops every fire.
- **The `pulse_fire_changed` frame.** Each fire's start, its move to its session, its session's registration, and its end reach the agent's WebSocket as `{ "type": "pulse_fire_changed", "pulse", "fire_id", "current_run", "last_outcome" }`. `current_run` is the pulse's run in flight after the change, exactly as `GET .../scheduled/pulses` reports it: the newest fire of the pulse, which can be another fire than `fire_id`, or `null` when nothing of the pulse is running. `last_outcome` is the pulse's new last run when the fire finished with an outcome, and `null` otherwise, which leaves the last run as it was. The frames come from the agent's system notification channel, which delivers every one, in the order the fires changed.
- **Team events.** The hub records a fire that ended without a session run as a `scheduled_run_finished` entry pointing at the agent's Schedule place (see [hub.md](hub.md#team-event-log)); a fire that ran a session is recorded when its session finishes, as any scheduled run is.

## Schedule Place

The web UI's Schedule place (under each agent in the rail, at `/agent/<name>/schedule`) lists every pulse by its name in words, as Home does (`inbox_check` reads "Inbox check"): how often it runs and its active hours, its skill, its model when it names one, when it runs next in Home's words ("in 34m", "Today at 9:00 AM", "Due now"; "Paused", "No next run", or "Won't run until fixed" otherwise), how its last run went and when ("Last ran 1h ago", "Last run was stopped 5m ago", or "Last run failed 2m ago: <error>"; for a fire that ran no session, such as a `script_only` pulse's, the same words from `last_fire`), and any HEARTBEAT.yml loading problems naming it. A run in flight shows what it is doing ("Running scripts", "Starting session" until its session registers, then "Running session"), when it started, an Open session link once it has a session, which opens the run in the context panel over the place, and, for a run that overlaps the one before it, when that one started. When no enabled pulse has a next run, a notice says the agent's pulses are turned off in its settings or its settings can't be read, and opens its Schedule settings.

Each pulse has a switch. Turning it off or on flips the `enabled` value for that one pulse in HEARTBEAT.yml in place — the rest of the file, including comments and formatting, is left byte-for-byte untouched. The switch moves at once and moves back, with an error notice, if the change fails; a pulse that failed to load has no switch to flip. The same place lists pending scheduled actions (see [scheduled-actions.md](scheduled-actions.md)) with Cancel for each, and an action's run in flight the way a pulse's shows.

Each run in flight has Stop. A pulse's fire stops through its fire (see "Pulse Fires" above), in its scripts or its session; a scheduled action's run, and a pulse session no fire tracks, stop through their session. Stop shows it is working until the run is gone, which the next `pulse_fire_changed` frame or session frame shows, and the pulse's last run then reads as stopped. A run that had already finished raises an error notice saying so and the place reads the schedule again; any other refusal (an agent that has stopped) raises one with the reason, and Stop can be pressed again.

A load that fails shows the error with Try again, never the empty lists. The schedule endpoints answer only while the agent runs, so a stopped agent's Schedule says it is stopped, and a failed agent's that it can't start and why, with Start; the schedule loads once the agent runs.

A pulse's next fire time is the first moment at or after the later of now and the end of its `schedule` since its last run that falls inside its `active_hours` (a pulse that has never run is first seen by the scheduler now, or when its `active_hours` next open, and runs one `schedule` after that; one that is due already is told at the start of the current minute). It is `null` for a pulse that is disabled, whose schedule or active hours can't be read, or whose active hours never open, and for every pulse when the agent's `[pulse] enabled` is `false` or its `config.toml` can't be read. The hub's team overview lists an agent's soonest runs with the same calculation (see [hub.md](hub.md#team-overview)).

The agent's Schedule settings hold `[pulse] enabled` as Run pulses, which pauses every pulse at once, and link to this place ("Open <agent>'s Schedule"). They are in `config.toml`, so they stay editable while the agent is stopped.

Backed by `GET /api/agents/{name}/scheduled/pulses`, `PUT /api/agents/{name}/scheduled/pulses/{pulse}/enabled`, `POST /api/agents/{name}/scheduled/pulses/{pulse}/fires/{fire_id}/stop` (see "Pulse Fires" above), `GET /api/agents/{name}/scheduled/actions`, and `DELETE /api/agents/{name}/scheduled/actions/{id}`. A pulse's or action's `current_run` is `{ "fire_id", "phase", "started_at", "address", "run_id", "overlap" }`: `fire_id` is `null` for a scheduled action's run and for a pulse session no fire tracks, which are stopped through their session; `phase` is `script` or `session`; `address` and `run_id` are `null` until the fire has asked for its session and until that session registers. `overlap` is `{ "previous_run_id", "previous_fire_id", "previous_started_at" }`. A pulse's `last_outcome` is `{ "status": "completed" | "cancelled" | "failed", "at", "error" }`. While it is open the place watches `HEARTBEAT.yml` and `scheduled_actions.json` on the agent's socket, through the socket's watch registry, and refetches when either changes, when the change feed reports it lost track, and on any `scheduled`-category session lifecycle frame, rather than polling. A `pulse_fire_changed` frame lands at once: the pulse's run in flight becomes the frame's `current_run` (`null` included), and its last run the frame's `last_outcome` when that is not `null`; the place then refetches for the pulse's next run time, which the frame doesn't carry. A frame that arrives while a load is in flight is applied again on top of that load's answer, so an answer read before the change can't put the old run back.

## Result Routing

Pulse results flow through the pub/sub bus to the notification router, which delivers each result according to the disposition the session declared (or, for a `script_only` pulse, the disposition its scripts' output declares; see "Task Scripts" above). A summary ending with `HEARTBEAT_OK` is discarded; one ending with `HEARTBEAT_URGENT` is pushed to every configured notification channel as well as filed to the inbox; anything else goes to the inbox alone.

See [notifications.md](notifications.md) for the full routing model.
