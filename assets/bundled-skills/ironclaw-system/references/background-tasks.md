# Background Tasks

Background tasks run concurrently with the main agent conversation. They support two execution types — LLM-driven sub-agents and shell scripts — with result routing through the notification system.

## Execution Types

### SubAgent

An ephemeral LLM turn loop with a minimal system prompt. The prompt includes only `ENVIRONMENT.md`, `USER.md`, the project index, and active skills. It explicitly **excludes** SOUL.md, AGENTS.md, MEMORY.md, and the observation log to keep context small.

SubAgents receive an isolated copy of project state, skill state, path policy, and tool filter. They share the MCP registry with the main agent.

### Script

A child process with a configurable timeout. Defined by `command`, `args`, `working_dir`, and `timeout_secs`.

## Model Tiers

SubAgent tasks specify a model tier that maps to configured models in `[background]`:

| Tier | Default Use | Fallback |
|------|-------------|----------|
| `Small` | Heartbeat pulses, lightweight checks | Medium → Large → Main |
| `Medium` | Default for scheduled actions and agent-spawned tasks | Large → Main |
| `Large` | Complex analysis, multi-step reasoning | Main |

The fallback chain walks up tiers. If no background model is configured at any tier, the main model is used.

## Tools

| Tool | Key Parameters | Description |
|------|---------------|-------------|
| `subagent_spawn` | `preset`, `prompt`, `wait`, `channels` | Spawn a sub-agent task. |
| `list_agents` | *(none)* | List active background tasks with elapsed time and prompt preview. |
| `stop_agent` | `task_id` | Cancel an active task by ID. |

### `subagent_spawn` Details

- **`preset`**: Name of a preset from `subagents/`. Optional — if omitted, uses default configuration.
- **`prompt`**: The task prompt for the sub-agent.
- **`wait`**: If `true`, blocks until the sub-agent completes and returns the result directly (synchronous mode). Default `false`.
- **`channels`**: Notification channel names for result routing in async mode. Required when `wait: false` and not using `Notify` routing. Validated against known channels.

## Result Routing

| Mode | Behavior |
|------|----------|
| `Notify` | Routes through NOTIFY.yml by task name. Used by heartbeats and scheduled actions. |
| `Direct(channels)` | Routes to specified channels, bypassing NOTIFY.yml. Used by agent-spawned tasks with explicit channels. |

## Concurrency

The `BackgroundTaskSpawner` enforces a configurable concurrency limit via a semaphore (`max_concurrent` in `[background]`). Tasks that exceed the limit wait for a slot. Each task gets a `CancellationToken` for graceful shutdown.

## Transcript Logging

Every background task writes a transcript log to:

```
memory/background/YYYY-MM/DD/bg-<task-id>.log
```

The directory is created on-demand when the first transcript is written.

## Task Lifecycle

1. **Spawn**: Task is registered with a unique ID and CancellationToken.
2. **Acquire**: Waits for a semaphore slot (respects concurrency limit).
3. **Execute**: Runs SubAgent turn loop or Script process, racing against cancellation.
4. **Complete**: Produces a `BackgroundResult` with status (`Completed`, `Cancelled`, `Failed`), summary, and transcript path.
5. **Route**: Result is sent through the notification system.
6. **Cleanup**: Task is removed from the active tasks map.

## Gotchas

- SubAgents have a **minimal system prompt** — they do not have access to the main agent's full identity or memory context.
- `wait: true` blocks the calling agent's turn until the sub-agent finishes. Use for short tasks only.
- If a project is active in a sub-agent, it is force-deactivated before the sub-agent exits (with a retry turn and manual fallback).
- Script tasks that exceed their timeout are killed.
- The `memory/background/` directory is not created at bootstrap — it appears only after the first background task runs.
