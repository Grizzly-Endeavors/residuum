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

**What's excluded:**
- `SOUL.md` (no identity)
- `AGENTS.md` (no behavioral rules)
- Observation log
- `MEMORY.md`
- Recent conversation messages

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
| `model_tier` | string | no | `"small"`, `"medium"`, `"large"`. Default: inherited from spawn call or `"medium"`. |
| `denied_tools` | string[] | no | Tools this preset cannot use. |
| `allowed_tools` | string[] | no | If set, only these tools are available (allowlist). |

One built-in preset exists: `general-purpose`. A user-created file with `name: general-purpose` overrides the built-in.

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
