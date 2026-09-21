# Background Tasks

Background tasks let the agent run work without blocking the main conversation. The execution model is **agent sessions** — temporary forks of the main agent that run independently and deliver results through notification channels.

For shell commands and scripts, the agent uses its own `write_file` and `exec` tools directly — there is no separate "script task" type.

## Sessions

A session is a fork of the main agent with its own identity, memory snapshot, and tool registry. The fork's system message carries the full main-agent identity — `SOUL.md`, `AGENTS.md`, `HARNESS`, `USER.md`, the wiki index, the skills index — assembled once, exactly as it is for the main agent, plus a snapshot of the global observation log and recent-context narrative taken at fork time. It never sees the main agent's live, unobserved conversation; the user message carries only the task prompt (or pulse/action/webhook input) and any explicit context the spawner passed.

Sessions share the MCP registry with the main agent.

## Categories and Lifecycle

Every session has a category — `scheduled` (pulses, actions), `external` (webhooks), or `spawned` (`subagent_spawn`, the `learner`) — and moves through `forking` → `running` → `idle` → `completing` → `completed`. A run holds a concurrency permit only while `running`; it lingers `idle` for its category's timeout (`idle_timeout_scheduled_minutes` / `_spawned_minutes` / `_external_minutes` in `[background]`, defaulting to 2 / 10 / 30 minutes; a webhook session uses the scheduled timeout) before completing. `stop_agent` cancels a session's stop token: a running turn ends at its next checkpoint with its transcript intact, an idle one completes immediately.

Each session has a stable address (e.g. `spawned-researcher-3f9a`) generated at spawn time.

## Model Tiers

Sessions specify a model tier that maps to configured models in `[background]`:

| Tier | Default Use | Fallback |
|------|-------------|----------|
| `Small` | Heartbeat pulses, lightweight checks | Medium → Large → Main |
| `Medium` | Default for scheduled actions and agent-spawned sessions | Large → Main |
| `Large` | Complex analysis, multi-step reasoning | Main |

The fallback chain walks up tiers. If no background model is configured at any tier, the main model is used.

## Session Roles

Pass a **skill** name at spawn time and that skill's body becomes the session's role instructions. There is no separate preset format — a role is an ordinary skill in `skills/<name>/SKILL.md`, so the same file can be activated in-turn or handed to a session.

Four role skills ship bundled:

- **`introspection`** — backs the built-in `reflection` pulse.
- **`wiki`** — backs the built-in `memory_tending` and `wiki_lint` pulses. `memory_tending` ingests episodes since the last `ingest` entry in `wiki/log.md` into wiki pages and `USER.md`; `wiki_lint` fixes index drift, missing frontmatter, stale pages, old drafts, duplicates, contradictions, and missing links.
- **`learner`** — spawned by a subconscious `learn` signal (opt-in, cooldown-limited) or by the `[learning] nudge_after_turns` fallback. Corroborates a `preference` signal against episodic memory and files it as a wiki page (`status: draft` on a single episode, promoted to `stable` once a second independent episode corroborates it), adding only corroborated core facts to USER.md. For a `recovery` signal, prefers queuing a durable fix via the user inbox over baking the workaround into a skill.
- **`memory-analyst`** — the main agent spawns it for synthesized questions about the user/history instead of doing raw `memory_search` itself. Uses multiple search phrasings for enumeration questions, surfaces contradictions with dates, abstains rather than fabricates, cites episode IDs.

Pulses and actions route by an `agent` field naming a skill; `agent: "main"` is removed — a pulse or action using it fails to load (pulses) or is rejected (the `schedule_action` tool) rather than silently running as something else.

## Tools

| Tool | Key Parameters | Description |
|------|---------------|-------------|
| `subagent_spawn` | `task`, `skill`, `model` | Fork a session. Returns its address immediately. Each turn's result relays through the notification router, tagged with the address. |
| `list_agents` | *(none)* | List main plus every live session, with category, state, depth, spawner, elapsed time, and purpose. |
| `stop_agent` | `address` | Stop a live session by address. |

### `subagent_spawn` Details

- **`task`**: The prompt/instructions for the session. Required.
- **`skill`**: Name of a skill to activate as the session's role. Omit to run on the task prompt alone. `"main"` is rejected. An unknown name fails immediately with the available list.
- **`model`**: `"small"`, `"medium"`, or `"large"`. Default: `"medium"`.

A session's result is a **self-report**, not a verified outcome. When the task is checkable, ask for concrete handles in the prompt (file paths, commit SHAs, URLs) and don't take "done" at face value until they check out.

## Result Routing

Every session result flows through the pub/sub bus to the notification router. `spawned` results relay back to the main agent, tagged with the session's address. `scheduled` and `external` results file to the inbox, additionally pushed to every configured notification channel when the summary contains `HEARTBEAT_URGENT`.

## Memory

A session merges what it learned into global memory when it completes — full model in [memory-system.md](memory-system.md#agent-sessions-and-memory). Short version: the run is checked against the same observer thresholds the main agent uses; crossing the force threshold mid-run stages observations locally; on completion the run produces an episode (tagged with its session address, run id, category) unless it staged nothing and either its final turn ended with `HEARTBEAT_OK` or its transcript is under the configurable `episode_skip_token_floor`. The transcript is kept in the session store either way, and the run's metadata records the episode id once merged.

## Concurrency

The session runtime enforces a configurable concurrency limit via a semaphore (`max_concurrent` in `[background]`). The permit is held only while a turn is running, not for the session's whole idle lifetime, so runs that exceed the limit wait for a slot rather than for another session to fully complete.

## Session Store

Every run's metadata is recorded under `memory/sessions/YYYY-MM/DD/<run-id>.json`, created on demand. While the run is live, its transcript is durably appended to a sibling `<run-id>.transcript.jsonl` file after every model response and tool result — a crash mid-turn loses at most the message in flight. On completion the full transcript is folded into the metadata file too, so a finished run's record is one self-contained file. A stopped run keeps its transcript up to the point it was stopped, and merges into memory like any other run. At startup, any run left incomplete by a prior process exit goes through the full completion pipeline (skip check, final observation, merge) from its persisted transcript before normal operation resumes, then is marked completed.

## Gotchas

- A session's fork always carries the main agent's full identity now — there is no minimal-context mode and no `include_identity` flag to opt in or out of.
- Tools excluded from sessions: `schedule_action`, `list_actions`, `cancel_action`, `subagent_spawn`, `switch_endpoint` (no nesting yet, no action scheduling from a session).
- The `memory/sessions/` directory is not created at bootstrap — it appears only after the first session run.
- A completed session is no longer listed by `list_agents`, but its address and transcript remain in the session store.
