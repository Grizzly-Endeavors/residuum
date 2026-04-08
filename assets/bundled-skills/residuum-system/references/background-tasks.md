# Background Tasks

Background tasks let the agent run work without blocking the main conversation. The execution model is sub-agents — ephemeral LLM turn loops that run independently and deliver results through notification channels.

For shell commands and scripts, the agent uses its own `write_file` and `exec` tools directly — there is no separate "script task" type.

## Sub-Agents

An ephemeral LLM turn loop with a minimal system prompt. The prompt includes `ENVIRONMENT.md`, `USER.md`, the project index, and active skills. It explicitly **excludes** SOUL.md, AGENTS.md, MEMORY.md, and the observation log to keep context small.

Sub-agents share the MCP registry with the main agent.

## Model Tiers

Sub-agent tasks specify a model tier that maps to configured models in `[background]`:

| Tier | Default Use | Fallback |
|------|-------------|----------|
| `Small` | Heartbeat pulses, lightweight checks | Medium → Large → Main |
| `Medium` | Default for scheduled actions and agent-spawned tasks | Large → Main |
| `Large` | Complex analysis, multi-step reasoning | Main |

The fallback chain walks up tiers. If no background model is configured at any tier, the main model is used.

## Subagent Presets

Presets are markdown files in `subagents/` with kebab-case filenames matching the preset name (e.g., `memory-agent.md`). YAML frontmatter can define: `name`, `description`, `model_tier`, `denied_tools`, `allowed_tools`. One built-in preset exists: `general-purpose`.

## Tools

| Tool | Key Parameters | Description |
|------|---------------|-------------|
| `subagent_spawn` | `task`, `agent_name`, `model_override` | Spawn a sub-agent task. Results route through the notification router. |
| `list_agents` | *(none)* | List active background tasks with elapsed time and prompt preview. |
| `stop_agent` | `task_id` | Cancel an active task by ID. |

### `subagent_spawn` Details

- **`task`**: The prompt/instructions for the sub-agent. Required.
- **`agent_name`**: Preset name from `subagents/`. Default: `"general-purpose"`. `"main"` is rejected.
- **`model_override`**: `"small"`, `"medium"`, or `"large"`. Overrides the preset's tier.

## Result Routing

All background task results flow through the pub/sub bus to the LLM notification router, which routes based on content analysis and `ALERTS.md` policy. Agent-spawned task results are also relayed back to the main agent as an interrupt.

## Concurrency

The `BackgroundTaskSpawner` enforces a configurable concurrency limit via a semaphore (`max_concurrent` in `[background]`). Tasks that exceed the limit wait for a slot. Each task gets a `CancellationToken` for graceful shutdown.

## Transcript Logging

Every background task writes a transcript log to:

```
memory/background/YYYY-MM/DD/bg-<task-id>.log
```

The directory is created on-demand when the first transcript is written.

## Gotchas

- Sub-agents have a **minimal system prompt** — they do not have access to the main agent's full identity or memory context.
- If a project is active in a sub-agent when it exits, the gateway force-deactivates with an auto-generated log entry.
- Tools excluded from sub-agents: `schedule_action`, `list_actions`, `cancel_action`, `subagent_spawn`, `stop_agent` (no sub-to-sub delegation, no action scheduling from background).
- The `memory/background/` directory is not created at bootstrap — it appears only after the first background task runs.
