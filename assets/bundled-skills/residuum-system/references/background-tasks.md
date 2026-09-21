# Background Tasks

Background tasks let the agent run work without blocking the main conversation. The execution model is sub-agents — ephemeral LLM turn loops that run independently and deliver results through notification channels.

For shell commands and scripts, the agent uses its own `write_file` and `exec` tools directly — there is no separate "script task" type.

## Sub-Agents

An ephemeral LLM turn loop with a minimal system prompt. The prompt includes `USER.md`, the root wiki index (`WIKI_INDEX`), and active skills. By default it **excludes** SOUL.md, AGENTS.md, and the observation log to keep context small — the spawn caller can opt back in with `include_identity: true` (see below).

Sub-agents share the MCP registry with the main agent.

## Model Tiers

Sub-agent tasks specify a model tier that maps to configured models in `[background]`:

| Tier | Default Use | Fallback |
|------|-------------|----------|
| `Small` | Heartbeat pulses, lightweight checks | Medium → Large → Main |
| `Medium` | Default for scheduled actions and agent-spawned tasks | Large → Main |
| `Large` | Complex analysis, multi-step reasoning | Main |

The fallback chain walks up tiers. If no background model is configured at any tier, the main model is used.

## Sub-Agent Roles

A sub-agent is an agent loop running off the main thread. Pass a **skill** name at spawn time and that skill's body becomes the sub-agent's role instructions. There is no separate preset format — a role is an ordinary skill in `skills/<name>/SKILL.md`, so the same file can be activated in-turn or handed to a sub-agent.

Four role skills ship bundled:

- **`introspection`** — backs the built-in `reflection` pulse.
- **`wiki`** — backs the built-in `memory_tending` and `wiki_lint` pulses. `memory_tending` ingests episodes since the last `ingest` entry in `wiki/log.md` into wiki pages and `USER.md`; `wiki_lint` fixes index drift, missing frontmatter, stale pages, old drafts, duplicates, contradictions, and missing links.
- **`learner`** — spawned by a subconscious `learn` signal (opt-in, cooldown-limited) or by the `[learning] nudge_after_turns` fallback. Corroborates a `preference` signal against episodic memory and files it as a wiki page (`status: draft` on a single episode, promoted to `stable` once a second independent episode corroborates it), adding only corroborated core facts to USER.md. For a `recovery` signal, prefers queuing a durable fix via the user inbox over baking the workaround into a skill.
- **`memory-analyst`** — the main agent spawns it for synthesized questions about the user/history instead of doing raw `memory_search` itself. Uses multiple search phrasings for enumeration questions, surfaces contradictions with dates, abstains rather than fabricates, cites episode IDs.

`include_identity` (boolean, default `false`) is set by the caller — when `true`, the sub-agent's prompt also includes SOUL.md and AGENTS.md alongside the usual USER.md/WIKI_INDEX. Use it for roles that need full identity context to make judgment calls (the `introspection` spawn sets it; `learner` spawns are fixed in code with identity included).

## Tools

| Tool | Key Parameters | Description |
|------|---------------|-------------|
| `subagent_spawn` | `task`, `skill`, `model` | Spawn a sub-agent task. Results route through the notification router. |
| `list_agents` | *(none)* | List active background tasks with elapsed time and prompt preview. |
| `stop_agent` | `task_id` | Cancel an active task by ID. |

### `subagent_spawn` Details

- **`task`**: The prompt/instructions for the sub-agent. Required.
- **`skill`**: Name of a skill to activate as the sub-agent's role. Omit to run on the task prompt alone. `"main"` is rejected. An unknown name fails immediately with the available list.
- **`model`**: `"small"`, `"medium"`, or `"large"`. Default: `"medium"`.

A sub-agent's result is a **self-report**, not a verified outcome. When the task is checkable, ask for concrete handles in the prompt (file paths, commit SHAs, URLs) and don't take "done" at face value until they check out.

## Result Routing

All background task results flow through the pub/sub bus to the notification router, which files them to the inbox and additionally pushes to every configured notification channel when the summary contains `HEARTBEAT_URGENT`. Agent-spawned task results are relayed back to the main agent instead.

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
- Tools excluded from sub-agents: `schedule_action`, `list_actions`, `cancel_action`, `subagent_spawn`, `stop_agent` (no sub-to-sub delegation, no action scheduling from background).
- The `memory/background/` directory is not created at bootstrap — it appears only after the first background task runs.
