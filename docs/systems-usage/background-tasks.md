# Background Tasks

Background tasks let the agent run work without blocking the main conversation. The execution model is sub-agents — ephemeral LLM turn loops that run independently and deliver results through notification channels.

## Sub-Agents

An ephemeral LLM turn loop with its own context. Sub-agents are lightweight workers — they get enough context to do their job but don't carry the full agent identity.

**What's included in sub-agent context:**
- Task prompt
- `USER.md`
- `ENVIRONMENT.md`
- Projects index
- Active skills
- Full tool set (with exceptions below)
- Optional inline context and file references

**What's excluded by default:**
- `SOUL.md` (no identity)
- `AGENTS.md` (no behavioral rules)
- Observation log
- `MEMORY.md`
- Recent conversation messages

A preset can opt back into identity context with `include_identity: true` in its frontmatter — this adds `SOUL.md`, `AGENTS.md`, and `MEMORY.md` to the sub-agent's prompt alongside the usual `ENVIRONMENT.md`/`USER.md`. The bundled `introspection` preset (used by the built-in `reflection` and `memory_tending` pulses) sets this, since it needs full identity context to judge what belongs in memory.

**Tools excluded from sub-agents:** `schedule_action`, `list_actions`, `cancel_action`, `subagent_spawn`, `stop_agent` (no sub-to-sub delegation, no action scheduling from background).

Sub-agents share the MCP registry with the main agent.

For shell commands and scripts, the agent uses its own `write_file` and `exec` tools directly — there is no separate "script task" type.

## Tools

### `subagent_spawn`

| Parameter | Type | Required | Notes |
|-----------|------|----------|-------|
| `task` | string | yes | The prompt/instructions for the sub-agent. Must not be empty. |
| `agent_name` | string | no | Preset name from `subagents/`. Default: `"general-purpose"`. `"main"` is rejected — you cannot spawn main as a sub-agent. |
| `model_override` | string enum | no | `"small"`, `"medium"`, `"large"`. Overrides the preset's tier. |

A sub-agent's final result is a **self-report** — it describes what the sub-agent believes it did, not a verified outcome. When the task involves something checkable (a file written, a command run, a deployment, an external change), the spawning agent should ask for concrete handles in the task prompt (file paths, commit SHAs, URLs, ticket IDs) and treat the result as unverified until those handles check out.

### `list_agents`

No parameters. Lists all currently active background tasks.

### `stop_agent`

| Parameter | Type | Required | Notes |
|-----------|------|----------|-------|
| `task_id` | string | yes | Cancels the task by ID. |

## Model Tiers

| Tier | Default Use | Fallback Chain |
|------|-------------|----------------|
| Small | Heartbeat pulses, lightweight checks | Medium → Large → Main |
| Medium | Default for `subagent_spawn` and scheduled actions | Large → Main |
| Large | Complex analysis, multi-step reasoning | Main |

Model tiers are configured in `[background]` config section (`models.small`, `models.medium`, `models.large`).

## Subagent Presets

Presets are markdown files in the workspace `subagents/` directory, these presets are used to populate the subagent registry. Filenames should be kebab-case matching the preset name (e.g., `memory-agent.md` for a preset named `memory-agent`).

```yaml
---
name: memory-agent
description: Lightweight agent with only memory tools
model_tier: small
denied_tools:
  - exec
  - write_file
allowed_tools:
  - memory_search
  - memory_get
  - read_file
---

(Optional body — additional system prompt content for this preset)
```

### Preset Frontmatter

| Field | Type | Required | Notes |
|-------|------|----------|-------|
| `name` | string | yes | Must match the filename (kebab-case) |
| `description` | string | yes | Shown when listing available presets |
| `model_tier` | string | no | `"small"`, `"medium"`, `"large"`. Default: inherited from spawn call or `"medium"`. Also determines the effective model tier when a heartbeat pulse names this preset via `agent:`. |
| `denied_tools` | string[] | no | Tools this preset cannot use. |
| `allowed_tools` | string[] | no | If set, only these tools are available (allowlist). |
| `include_identity` | boolean | no | Default `false`. When `true`, adds `SOUL.md`, `AGENTS.md`, and `MEMORY.md` to the sub-agent's prompt in addition to the default `ENVIRONMENT.md`/`USER.md`. |

Four built-in presets exist. A user-created file with the same name overrides the built-in.

| Preset | Tier | `include_identity` | Spawned by |
|--------|------|---------------------|------------|
| `general-purpose` | — | `false` | Default for `subagent_spawn` when `agent_name` is omitted. |
| `introspection` | `large` | `true` | The built-in `reflection`/`memory_tending` pulses. |
| `learner` | `large` | `true` | A subconscious `learn` signal (subject to `learning_cooldown_minutes`), or the `[learning] nudge_after_turns` fallback. Corroborates the signal against episodic memory and, for `preference` signals, promotes it to `USER.md` once at least two supporting observations exist (annotating the evidence count); single sightings go to `MEMORY.md` as provisional. For `recovery` signals, it prefers queuing a durable fix via the user inbox over encoding the workaround into a skill — a skill is only warranted when the obstacle is an external constraint that can't be fixed. Reports via at most one user-inbox item. See [subconscious.md](subconscious.md#learning-trigger). |
| `memory-analyst` | `medium` | `true` | The main agent, when it needs a synthesized answer about the user or past history rather than raw search results. Read-only (`write_file`/`edit_file` denied); uses multiple search phrasings for enumeration questions, surfaces contradictions with dates instead of silently picking one, abstains rather than fabricating when the record is silent, and cites episode IDs. |

## Concurrency

`BackgroundTaskSpawner` uses a semaphore bounded by `max_concurrent` in the `[background]` config section. Tasks that exceed the limit wait for a permit.

## Result Routing

All background task results flow through the pub/sub bus to the LLM notification router, which decides where each result goes based on content analysis and the `ALERTS.md` policy file. Agent-spawned task results are also relayed back to the main agent as an interrupt (Layer 1 programmatic rule).

See [notifications.md](notifications.md) for the full routing model.

## Transcript Logging

Every background task writes a transcript to `memory/background/YYYY-MM/DD/bg-<task-id>.log`. The directory is created on-demand (not at bootstrap).

Transcripts contain the full turn history: tool calls, tool results, intermediate messages, and the final response, serialized as JSON. This provides an auditable record of everything the sub-agent did.

## Task Lifecycle

Spawn → Acquire semaphore permit → Execute → Complete → Route result → Cleanup

### Project Interaction

- No locking on project activation — multiple sub-agents can have the same project active simultaneously
- Last-write-wins for files
- MCP servers use reference counting per project (no premature teardown)
- If a sub-agent ends with a project still active, the gateway force-deactivates with an auto-generated log entry
- Cancellation also triggers force-deactivation

All spawns are asynchronous — `subagent_spawn` returns immediately with a task ID. Results are routed through the notification system when the sub-agent completes.
