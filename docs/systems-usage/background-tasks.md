# Background Tasks

Background tasks let the agent run work without blocking the main conversation. The execution model is **agent sessions** — temporary forks of the main agent that run independently and deliver results through notification channels.

## Sessions

A session is a fork of the main agent with its own identity, memory snapshot, and tool registry, running off the main thread.

**What's included in a session's fork:**
- The main agent's full identity and system prompt content — `SOUL.md`, `AGENTS.md`, `HARNESS`, `USER.md`, the wiki root index, the skills index — assembled once in the system message, exactly as it is for the main agent.
- A snapshot of the global observation log and the recent-context narrative, taken at fork time. A session never sees observations merged after it forked.
- Its source-specific input as the user message: the task prompt (spawned), the pulse or action prompt (scheduled), or the webhook payload (webhook). A session resumed by a message to a completed address (see [Messaging](#messaging)) gets that message instead, plus a pointer back to its previous run's episode.
- The requested skill activated, when one was given. A resumed session keeps the skill its previous run used.
- The requested model tier.

**What's excluded:**
- The main agent's live, unobserved conversation. A session never sees what the user and main agent are currently discussing — the spawning agent writes a task prompt with whatever context the session needs.

**Tools excluded from sessions:** `schedule_action`, `list_actions`, `cancel_action`, `subagent_spawn`, `switch_endpoint` (no nesting yet, no action scheduling from a session, and `switch_endpoint` only makes sense for the main agent's own output routing). `message_agent` is available to both main and every session.

Sessions share the MCP registry with the main agent.

For shell commands and scripts, the agent uses its own `write_file` and `exec` tools directly — there is no separate "script task" type.

## Categories

Every session has a category, derived from what started it:

| Category | Started by | Address prefix |
|----------|-----------|-----------------|
| `scheduled` | Pulses and scheduled actions | `scheduled-` |
| `external` | Webhooks | `external-` |
| `spawned` | `subagent_spawn`, the subconscious `learner` | `spawned-` |

## Lifecycle

A session run moves through: `forking` → `running` → `idle` → `completing` → `completed`.

- **forking** — the session is registered and its fork resources (identity, memory snapshot, tools) are being built; no turn has started.
- **running** — a turn is executing. The run holds one concurrency permit.
- **idle** — the turn ended; the session is still discoverable via `list_agents` and lingers for its category's idle timeout. A message delivered to it (see [Messaging](#messaging)) starts another turn in the same run instead of waiting out the timeout, so a run can span several turns.
- **completing** — the idle timeout elapsed, or the session was stopped via `stop_agent`.
- **completed** — the run's final transcript and metadata are recorded in the session store, and the result is delivered. The session is no longer listed by `list_agents`, though its address stays meaningful: a message to it starts a new run at the same address (see [Messaging](#messaging)).

Stopping a session (`stop_agent`) cancels its stop token: a running turn ends at its next checkpoint (a model-call or tool-loop boundary) with its transcript up to that point intact, rather than being dropped; an idle session skips straight to completing.

### Idle Timeouts

Configurable in the `[background]` config section:

| Category | Config key | Default |
|----------|-----------|---------|
| `scheduled` (also used by `external` webhook sessions — a webhook call is one-shot) | `idle_timeout_scheduled_minutes` | 2 minutes |
| `spawned` | `idle_timeout_spawned_minutes` | 10 minutes |
| `external` (non-webhook) | `idle_timeout_external_minutes` | 30 minutes |

## Addresses

Every session has a stable, human-readable address, e.g. `spawned-researcher-3f9a`: the category, a slugified qualifier (skill, pulse, action, or webhook name), and a short random suffix. `subagent_spawn` generates the address synchronously and returns it immediately, before the session has actually started running.

A run id, distinct from the address, identifies the specific run within the session's lifecycle.

## Messaging

Agents message each other by address with the `message_agent` tool, available to the main agent and every session. Delivery depends on the target's current lifecycle state:

- **`main`** — delivered as an interrupt at the next tool-call boundary if a main turn is running, otherwise it starts a main turn.
- **running session** — delivered as an interrupt at the session's next tool-call boundary, through the same interrupt channel `stop_agent` uses to end a turn.
- **idle session** — starts another turn in the same run, with the message as that turn's input. The run's transcript and per-turn memory staging (see [Memory](#memory)) span every turn this way, not just the first.
- **completed session** — the session is resumed as a new run at the same address, forked the same way any other session is. The new run's context carries a pointer back to the previous run's episode id, or its run id if that run produced no episode, retrievable with `memory_get`. The sender's tool result says the session had completed and was resumed.
- **unknown address** — an address that has never run reports an error naming `list_agents` as the way to find live sessions.

Every delivered message names the sender's address and category, so the recipient knows who to reply to.

## Tools

### `message_agent`

| Parameter | Type | Required | Notes |
|-----------|------|----------|-------|
| `to` | string | yes | `"main"`, or a session address from `list_agents`. |
| `message` | string | yes | The message body. Must not be empty. |

Sends `message` to `to`, delivered per the rules in [Messaging](#messaging). Messaging yourself is rejected.

### `subagent_spawn`

| Parameter | Type | Required | Notes |
|-----------|------|----------|-------|
| `task` | string | yes | The prompt/instructions for the session. Must not be empty. |
| `skill` | string | no | Name of a skill to activate as the session's role. Omit to run on the task prompt alone. `"main"` is rejected. |
| `model` | string enum | no | `"small"`, `"medium"`, `"large"`. Default: `"medium"`. |

Returns the session's address immediately. A session's final result is a **self-report** — it describes what the session believes it did, not a verified outcome. When the task involves something checkable (a file written, a command run, a deployment, an external change), the spawning agent should ask for concrete handles in the task prompt (file paths, commit SHAs, URLs, ticket IDs) and treat the result as unverified until those handles check out.

### `list_agents`

No parameters. Lists the main agent plus every live (running or idle) session: address, category, source, state, depth, spawner, elapsed time, and purpose.

### `stop_agent`

| Parameter | Type | Required | Notes |
|-----------|------|----------|-------|
| `address` | string | yes | Stops the session at this address. |

The main agent cannot be stopped this way. To stop the main-agent turn itself — the conversation the user is having — see [Turn Control](turn-control.md) instead; that's a user-facing interface control, not a tool.

## Model Tiers

| Tier | Default Use | Fallback Chain |
|------|-------------|----------------|
| Small | Heartbeat pulses, lightweight checks | Medium → Large → Main |
| Medium | Default for `subagent_spawn` and scheduled actions | Large → Main |
| Large | Complex analysis, multi-step reasoning | Main |

Model tiers are configured in `[background]` config section (`models.small`, `models.medium`, `models.large`).

## Session Roles

The only thing that distinguishes one spawned session from another is what the caller passes at fork time: a prompt, a model tier, and optionally a **skill** whose body becomes the session's role instructions.

There is no separate preset format. A role is an ordinary skill in `skills/<name>/SKILL.md`, so the same file can be activated in-turn by the main agent or handed to a session as its brief.

```yaml
---
name: memory-analyst
description: Answers synthesized questions about the user and past history from episodic memory.
---

(Body — the instructions this session runs with.)
```

Spawn configuration lives at the call site, not in the file:

| Caller | Where the config lives |
|--------|------------------------|
| `subagent_spawn` | The tool's `skill` and `model` parameters. |
| Heartbeat pulses | `agent` and `model_tier` on the pulse in `HEARTBEAT.yml`. `agent: "main"` and `include_identity` are removed — a pulse using either fails to load with an error naming it. |
| Scheduled actions | `agent` and `model_tier` on the action. `agent_name: "main"` is rejected by `schedule_action` and dropped (loudly) from any stored action that predates this change. |
| Subconscious `learner` | Fixed in code: the `learner` skill, `large` tier. |

A spawn naming a skill that does not resolve fails loudly rather than running a session without the instructions that define its job.

### Bundled Role Skills

| Skill | Spawned by |
|-------|------------|
| `introspection` | The built-in `reflection` pulse, at `large`. |
| `wiki` | The built-in `memory_tending` and `wiki_lint` pulses, at `large`. `memory_tending` ingests episodes since the last `ingest` entry in `wiki/log.md` into wiki pages and `USER.md`; `wiki_lint` fixes index drift, missing frontmatter, stale pages, old drafts, duplicates, contradictions, and missing links. |
| `learner` | A subconscious `learn` signal (subject to `learning_cooldown_minutes`), or the `[learning] nudge_after_turns` fallback. Corroborates the signal against episodic memory and, for `preference` signals, files it as a wiki page — `status: draft` for a single episode, promoted to `stable` once a second independent episode supports it — and adds only corroborated core facts to `USER.md`. For `recovery` signals, it prefers queuing a durable fix via the user inbox over encoding the workaround into a skill — a skill is only warranted when the obstacle is an external constraint that can't be fixed. Reports via at most one user-inbox item. See [subconscious.md](subconscious.md#learning-trigger). |
| `memory-analyst` | The main agent, when it needs a synthesized answer about the user or past history rather than raw search results. Uses multiple search phrasings for enumeration questions, surfaces contradictions with dates instead of silently picking one, abstains rather than fabricating when the record is silent, and cites episode IDs. |

## Concurrency

The session runtime uses a semaphore bounded by `max_concurrent` in the `[background]` config section. The permit is held only while a turn is actually running — an idle session holds nothing, so lingering sessions cost memory, not throughput. Runs that can't get a permit wait for one.

## Result Routing

Every session's result flows through the pub/sub bus to the notification router, which delivers it according to the disposition the producing agent declared. A `spawned` session's result relays back to the main agent, tagged with its address; `scheduled` and `external` results go through the inbox/urgent-fanout rules.

See [notifications.md](notifications.md) for the full routing model.

## Memory

A session has its own working memory and merges what it learned into global memory when it completes — see [memory.md](memory.md#agent-sessions-and-memory) for the full model. In short: after every turn in the run, its accumulated messages are checked against the same observer thresholds the main agent uses, and crossing the force threshold stages observations locally — a multi-turn run (see [Messaging](#messaging)) stages after each of its turns, not just once. On completion the run produces an episode (tagged with its session address, run id, and category) unless it staged nothing and either its final turn ended with `HEARTBEAT_OK` or its transcript is under the configurable `episode_skip_token_floor`. Its transcript is kept in the session store either way. The run's metadata records the episode id once merged.

## Session Store

Every run's metadata is recorded under `memory/sessions/YYYY-MM/DD/<run-id>.json`, created on demand. While the run is live, its transcript is durably appended to a sibling `<run-id>.transcript.jsonl` file after every model response and tool result, so a crash mid-turn loses at most the message in flight; on completion the full transcript is folded into the metadata file too, so a finished run's record is a single, self-contained file. Stopped runs keep their transcript up to the point they were stopped and merge into memory like any other run.

At startup, any run left incomplete by a prior process exit goes through the full completion pipeline — skip check, final observation, merge — from its persisted transcript before normal operation resumes, then is marked completed.

The record carries the session's address, run id, category, source label, spawner, depth, purpose, lifecycle timestamps, the episode id once merged, and the full message transcript once the run completes.
