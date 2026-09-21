# Background Tasks

Background tasks let the agent run work without blocking the main conversation. The execution model is sub-agents — ephemeral LLM turn loops that run independently and deliver results through notification channels.

## Sub-Agents

An ephemeral LLM turn loop with its own context. Sub-agents are lightweight workers — they get enough context to do their job but don't carry the full agent identity.

**What's included in sub-agent context:**
- Task prompt
- `USER.md`
- `WIKI_INDEX` (the root `wiki/index.md`)
- Active skills
- Full tool set (with exceptions below)
- Optional inline context and file references

**What's excluded by default:**
- `SOUL.md` (no identity)
- `AGENTS.md` (no behavioral rules)
- Observation log
- Recent conversation messages

The spawn caller can opt back into identity context with `include_identity: true` — this adds `SOUL.md` and `AGENTS.md` to the sub-agent's prompt alongside the usual `USER.md`/`WIKI_INDEX`. The built-in `reflection` pulse sets it, since the `introspection` skill needs full identity context to judge what's worth surfacing.

**Tools excluded from sub-agents:** `schedule_action`, `list_actions`, `cancel_action`, `subagent_spawn`, `stop_agent` (no sub-to-sub delegation, no action scheduling from background).

Sub-agents share the MCP registry with the main agent.

For shell commands and scripts, the agent uses its own `write_file` and `exec` tools directly — there is no separate "script task" type.

## Tools

### `subagent_spawn`

| Parameter | Type | Required | Notes |
|-----------|------|----------|-------|
| `task` | string | yes | The prompt/instructions for the sub-agent. Must not be empty. |
| `skill` | string | no | Name of a skill to activate as the sub-agent's role. Omit to run on the task prompt alone. `"main"` is rejected — you cannot spawn main as a sub-agent. |
| `model` | string enum | no | `"small"`, `"medium"`, `"large"`. Default: `"medium"`. |

A sub-agent's final result is a **self-report** — it describes what the sub-agent believes it did, not a verified outcome. When the task involves something checkable (a file written, a command run, a deployment, an external change), the spawning agent should ask for concrete handles in the task prompt (file paths, commit SHAs, URLs, ticket IDs) and treat the result as unverified until those handles check out.

### `list_agents`

No parameters. Lists all currently active background tasks.

### `stop_agent`

| Parameter | Type | Required | Notes |
|-----------|------|----------|-------|
| `task_id` | string | yes | Cancels the task by ID. |

Cancels a background sub-agent. To stop the main-agent turn itself — the conversation the user is having — see [Turn Control](turn-control.md) instead; that's a user-facing interface control, not a tool.

## Model Tiers

| Tier | Default Use | Fallback Chain |
|------|-------------|----------------|
| Small | Heartbeat pulses, lightweight checks | Medium → Large → Main |
| Medium | Default for `subagent_spawn` and scheduled actions | Large → Main |
| Large | Complex analysis, multi-step reasoning | Main |

Model tiers are configured in `[background]` config section (`models.small`, `models.medium`, `models.large`).

## Sub-Agent Roles

A sub-agent is an agent loop running off the main thread. The only thing that distinguishes one sub-agent from another is what the caller passes at spawn time: a prompt, a model tier, whether identity files are included, and optionally a **skill** whose body becomes the sub-agent's role instructions.

There is no separate preset format. A role is an ordinary skill in `skills/<name>/SKILL.md`, so the same file can be activated in-turn by the main agent or handed to a sub-agent as its brief.

```yaml
---
name: memory-analyst
description: Answers synthesized questions about the user and past history from episodic memory.
---

(Body — the instructions this sub-agent runs with.)
```

Spawn configuration lives at the call site, not in the file:

| Caller | Where the config lives |
|--------|------------------------|
| `subagent_spawn` | The tool's `skill` and `model` parameters. |
| Heartbeat pulses | `agent`, `model_tier`, and `include_identity` on the pulse in `HEARTBEAT.yml`. |
| Scheduled actions | `agent` and `model_tier` on the action. |
| Subconscious `learner` | Fixed in code: the `learner` skill, `large` tier, identity included. |

A spawn naming a skill that does not resolve fails loudly rather than running a sub-agent without the instructions that define its job.

### Bundled Role Skills

| Skill | Spawned by |
|-------|------------|
| `introspection` | The built-in `reflection` pulse, at `large` with identity included. |
| `wiki` | The built-in `memory_tending` and `wiki_lint` pulses, at `large` without identity. `memory_tending` ingests episodes since the last `ingest` entry in `wiki/log.md` into wiki pages and `USER.md`; `wiki_lint` fixes index drift, missing frontmatter, stale pages, old drafts, duplicates, contradictions, and missing links. |
| `learner` | A subconscious `learn` signal (subject to `learning_cooldown_minutes`), or the `[learning] nudge_after_turns` fallback. Corroborates the signal against episodic memory and, for `preference` signals, files it as a wiki page — `status: draft` for a single episode, promoted to `stable` once a second independent episode supports it — and adds only corroborated core facts to `USER.md`. For `recovery` signals, it prefers queuing a durable fix via the user inbox over encoding the workaround into a skill — a skill is only warranted when the obstacle is an external constraint that can't be fixed. Reports via at most one user-inbox item. See [subconscious.md](subconscious.md#learning-trigger). |
| `memory-analyst` | The main agent, when it needs a synthesized answer about the user or past history rather than raw search results. Uses multiple search phrasings for enumeration questions, surfaces contradictions with dates instead of silently picking one, abstains rather than fabricating when the record is silent, and cites episode IDs. |

## Concurrency

`BackgroundTaskSpawner` uses a semaphore bounded by `max_concurrent` in the `[background]` config section. Tasks that exceed the limit wait for a permit.

## Result Routing

All background task results flow through the pub/sub bus to the notification router, which delivers each result according to the disposition the producing agent declared. Agent-spawned task results are relayed back to the main agent instead.

See [notifications.md](notifications.md) for the full routing model.

## Transcript Logging

Every background task writes a transcript to `memory/background/YYYY-MM/DD/bg-<task-id>.log`. The directory is created on-demand (not at bootstrap).

Transcripts contain the full turn history: tool calls, tool results, intermediate messages, and the final response, serialized as JSON. This provides an auditable record of everything the sub-agent did.

## Task Lifecycle

Spawn → Acquire semaphore permit → Execute → Complete → Route result → Cleanup

All spawns are asynchronous — `subagent_spawn` returns immediately with a task ID. Results are routed through the notification system when the sub-agent completes.
