# Background Tasks

Background tasks let the agent run work without blocking the main conversation. The execution model is **agent sessions** — temporary forks of the main agent that run independently and deliver results through notification channels.

For shell commands and scripts, the agent uses its own `write_file` and `exec` tools directly — there is no separate "script task" type.

## Sessions

A session is a fork of the main agent with its own identity, memory snapshot, and tool registry. The fork's system message carries the full main-agent identity — `SOUL.md`, `AGENTS.md`, `HARNESS`, `USER.md`, the wiki index, the skills index — assembled once, exactly as it is for the main agent, plus a snapshot of the global observation log and recent-context narrative taken at fork time. It never sees the main agent's live, unobserved conversation; the user message carries only the task prompt (or pulse/action/webhook input) and any explicit context the spawner passed.

Sessions share the MCP registry with the main agent.

## Categories and Lifecycle

Every session has a category — `scheduled` (pulses, actions), `external` (webhooks, and non-owner-DM conversations on Discord/Telegram/Teams — see Conversation Routing below), or `spawned` (`subagent_spawn`, the `learner`) — and moves through `forking` → `running` → `idle` → `completing` → `completed`. A run holds a concurrency permit only while `running`; it lingers `idle` for its category's timeout (`idle_timeout_scheduled_minutes` / `_spawned_minutes` / `_external_minutes` in `[background]`, defaulting to 2 / 10 / 30 minutes; a webhook session uses the scheduled timeout) before completing. A `completing` run no longer accepts messages into itself — see Messaging below. `stop_agent` cancels a session's stop token: a running turn ends at its next checkpoint with its transcript intact, an idle one completes immediately. Either way the run is reported as cancelled; a run that simply idles out keeps its last turn's outcome.

Each `scheduled`/`spawned`/webhook session has a stable address (e.g. `spawned-researcher-3f9a`) generated at spawn time. A conversation session instead gets a **deterministic** address derived from its interface and conversation id (e.g. `external-discord-3f9a2c1b0d4e5f6a`, the id hashed for URL/filename safety), so the same conversation always resolves to the same session. Messaging a completed session's address (see Messaging below) resumes it as a new run at the same address.

**Nesting**: sessions can spawn sessions via their own `subagent_spawn`, up to `subagent_depth_cap` (default 2; main is depth 0). See `subagent_spawn` Details below.

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

## Messaging

`message_agent` sends text to an address, available to main and every session. Delivery depends on the target's state: `main` and a running session get it as an interrupt at the next tool-call boundary (a saturated channel — vanishingly unlikely — errors back to the sender rather than silently resuming a duplicate run); an idle session starts another turn in the same run with it as input (so a run can span several turns, and per-turn memory staging runs after each one); a completing session's message is queued — the tool call returns immediately rather than blocking on that run's own completion pipeline (memory merge, transcript write), and a background task waits for it to clear, then re-checks the address: if another queued message already resumed it, this one delivers straight into that live run; otherwise it resumes the session as a new run carrying the previous run's model tier, spawner, and depth, with a pointer to its previous run's episode (or run id, for `memory_get`) in the new run's context. That re-check is what lets two messages queued to one completing session both arrive as exactly one new run, instead of the second silently losing a race. An address that has never run reports an error pointing at `list_agents`. Every delivered message names the sender's address and category, and a failed publish (to main, or as a resume) errors back to the sender instead of reporting success.

**Hop counts.** Every agent message carries a hop count, bounding message loops. External-origin input (user message, pulse/action, webhook, web sidebar) is hop 0; a message sent during a turn carries one more than the highest hop count among that turn's inputs (kickoff plus any drained agent-message interrupts); a `subagent_spawn` task brief carries the spawning turn's hop count plus one, and a resumed session starts at the triggering message's hop count. This carries across a turn boundary too: a message that arrives mid-turn but isn't consumed before the turn ends still has its hop count picked up by whichever turn handles it next, rather than resetting to zero. Two configurable limits in `[background]`: `hop_soft_limit` (default 8) adds a "reply only if needed" note to the delivered message; `hop_hard_limit` (default 32) refuses delivery outright, with a `warn` log and a best-effort transcript note on whichever side is a live session (also shown as an error on that session in the web UI).

**Messages from the owner.** The web UI can follow every session live, message one, and stop it. A message the owner sends a session this way arrives like any agent message (hop 0, same interrupt/new-turn/resume rules) but is labelled `[Message from the owner via the web UI ...]` and comes from the reserved sender address `owner`, which is not an agent: answer in your turn's response — the owner reads it there — rather than with `message_agent`.

## Conversation Routing

The main agent handles only the owner's own DM and the web UI. Every other conversation Discord/Telegram/Teams admits — a group chat, a channel, or a non-owner's DM — routes to that conversation's own `external` session instead, even when the owner is the one talking there. Admission (owner claim, `respond_to_others`) is unchanged and happens first; routing only decides who handles an already-admitted message.

Delivery follows the same rules as Messaging below — interrupt if running, new turn if idle, deferred resume if completing — except a conversation message is always hop 0, and an address with no prior run starts a brand-new session instead of erroring as unknown. The session sees the same `[From: name via interface (location)]` attribution and buffered chatter each interface page describes. A message that arrives while its session's interrupt channel is saturated is not delivered: an error is logged and main gets a notice naming the session and conversation.

A conversation session's turn output — final response and any intermediate pre-tool-call text, the same as main posts mid-turn — goes straight back to its own conversation and **never falls back to the owner's DM** — unlike main's own proactive output. If the interface can't deliver it, the output is dropped, an error is logged naming the session and conversation, and main gets a notice to decide whether the owner needs telling.

## Tools

| Tool | Key Parameters | Description |
|------|---------------|-------------|
| `message_agent` | `to`, `message` | Send text to `main` or a session address. See Messaging above. |
| `subagent_spawn` | `task`, `skill`, `model` | Fork a session. Returns its address immediately. Each turn's result relays to its direct spawner via the agent-messaging path, tagged with the address. |
| `list_agents` | *(none)* | List main plus every live session, with category, state, depth, spawner, elapsed time, and purpose. |
| `stop_agent` | `address` | Stop a live session by address. |

### `subagent_spawn` Details

- **`task`**: The prompt/instructions for the session. Required.
- **`skill`**: Name of a skill to activate as the session's role. Omit to run on the task prompt alone. `"main"` is rejected. An unknown name fails immediately with the available list.
- **`model`**: `"small"`, `"medium"`, or `"large"`. Default: `"medium"`.

Available to the main agent and to every session — sessions can spawn sessions. Depth counts from main (depth 0); a `scheduled`/`external` session is depth 1; a spawned session is its spawner's depth plus 1, whatever the spawner's category, and its spawner is recorded as the calling agent's address. Depth is capped by `subagent_depth_cap` in `[background]` (default 2) — spawning past the cap is refused with an explanatory error.

A session's result is a **self-report**, not a verified outcome. When the task is checkable, ask for concrete handles in the prompt (file paths, commit SHAs, URLs) and don't take "done" at face value until they check out.

## Result Routing

A `spawned` session's turn result relays to its **direct spawner** (main, or whichever session spawned it) via the agent-messaging path, hop counts included, after every turn — not just at completion. Every outcome relays, not just a completed turn with output: a turn that produced no text, failed, was cancelled, or panicked (reported as failed) all relay a clear status line naming the session and what happened, so the spawner is never left not knowing. A relay failure — the spawner is busy, or unreachable (e.g. it restarted and lost its resume point) — is logged and noted in the session's own transcript, never silently dropped. `scheduled` and `external` results still flow through the pub/sub bus to the notification router: filed to the inbox, additionally pushed to every configured notification channel when the summary contains `HEARTBEAT_URGENT`. `spawned` results no longer pass through that router.

## Memory

A session merges what it learned into global memory when it completes — full model in [memory-system.md](memory-system.md#agent-sessions-and-memory). Short version: after every turn in the run, its accumulated messages are checked against the same observer thresholds the main agent uses; crossing the force threshold stages observations locally, for each turn a multi-turn run has, not just once; on completion the run produces an episode (tagged with its session address, run id, category) unless it staged nothing and either its final turn ended with `HEARTBEAT_OK` or its transcript is under the configurable `episode_skip_token_floor`. The transcript is kept in the session store either way, and the run's metadata records the episode id once merged.

## Concurrency

The session runtime enforces a configurable concurrency limit via a semaphore (`max_concurrent` in `[background]`). The permit is held only while a turn is running, not for the session's whole idle lifetime, so runs that exceed the limit wait for a slot rather than for another session to fully complete.

## Session Store

Every run's metadata is recorded under `memory/sessions/YYYY-MM/DD/<run-id>.json`, created on demand. While the run is live, its transcript is durably appended to a sibling `<run-id>.transcript.jsonl` file after every model response and tool result — a crash mid-turn loses at most the message in flight. On completion the full transcript is folded into the metadata file too, so a finished run's record is one self-contained file. A stopped run keeps its transcript up to the point it was stopped, and merges into memory like any other run. At startup, any run left incomplete by a prior process exit goes through the full completion pipeline (skip check, final observation, merge) from its persisted transcript before normal operation resumes, then is marked completed.

## Gotchas

- A session's fork always carries the main agent's full identity now — there is no minimal-context mode and no `include_identity` flag to opt in or out of.
- The only tool excluded from sessions is `switch_endpoint` — it only makes sense for the main agent's own output routing. `subagent_spawn`, the action-scheduling tools, and `message_agent` are all available to sessions.
- A session's `send_message` refuses the WebSocket endpoint and the owner's DM on every chat interface (named explicitly, or reached through the no-conversation default) — only `main` talks to the owner. See [notifications.md](notifications.md).
- The `memory/sessions/` directory is not created at bootstrap — it appears only after the first session run.
- A completed session is no longer listed by `list_agents`, but its address and transcript remain in the session store — and the web UI's session listing includes finished runs.
