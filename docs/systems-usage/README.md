# Systems Usage — Intent & Ownership

This directory documents how each Residuum system is **intended to be used**, by both the agent and the user. It serves as the authoritative reference when writing onboarding content, reference skills, or default workspace files.

## Ownership Model

Everything inside the workspace directory is **agent-owned by default**. The agent creates, reads, updates, and evolves these files as part of normal operation. The user provides initial guidance during onboarding and occasional course corrections, but the goal is that users rarely need to intervene after the first conversation.

### Agent-owned files

| File | Churn | Notes |
|------|-------|-------|
| `MEMORY.md` | High | Persistent scratchpad. Agent updates frequently with cross-session context. |
| `USER.md` | Medium | Agent records user preferences, communication style, active interests. |
| `ENVIRONMENT.md` | Low | Agent documents local environment details it discovers. |
| `HEARTBEAT.yml` | Medium | Agent creates during onboarding, evolves autonomously (adds/removes pulses, adjusts schedules, moves routing). |
| `CHANNELS.yml` | Medium | Agent evolves channel registry based on what the user pays attention to. |
| `PRESENCE.toml` | Low | Discord presence. Agent updates when context changes. |
| `SOUL.md` | Rare | Foundational identity. Agent may refine wording but shouldn't overhaul without user input. |
| `AGENTS.md` | Rare | Behavioral rules. Same as SOUL.md — low-churn, foundational. |
| `memory/OBSERVER.md` | Low | Observer extraction prompt. Agent can improve over time via self-analysis. |
| `memory/REFLECTOR.md` | Low | Reflector compression prompt. Same — agent self-improves. |
| `scheduled_actions.json` | Managed via tools | Never edited directly. Created/removed by `schedule_action` / `cancel_action`. |
| `pulse_state.json` | Managed by gateway | Pulse last-run timestamps and run counts. Persisted across restarts. Never edited directly. |

### User-owned files

| File | Notes |
|------|-------|
| `config.toml` | Lives outside the workspace directory. Agent writes are blocked by `PathPolicy` — the gateway enforces this at the tool level, not by prompt instruction. |

### Key principle

All workspace `.md` files are presented to the agent as markdown files it owns. The observer and reflector prompts (`OBSERVER.md`, `REFLECTOR.md`) exist specifically so the agent can set up a heartbeat to analyze its own past episodes and extracted observations, then iteratively improve those prompts over time.

## Design Principles

These are drawn from [design-philosophy.md](../design-philosophy.md) and inform how every system should be documented:

1. **File-first**: System state lives in files the user can inspect, edit, and version control. No opaque databases (exception: `vectors.db` for embeddings, since raw vectors aren't human-parsable).

2. **Gateway schedules, LLM evaluates**: The gateway handles timing, concurrency, file watching, and schema validation. The LLM is only invoked when judgment is needed.

3. **Agent autonomy with transparency**: The agent acts on its own for routine operations. Every action is visible in the filesystem. Users can always see what the agent did by looking at files.

4. **No silent failures**: Every failure must be visible. Debug/trace logging is not sufficient. Partial failures must be reported, not ignored.

5. **Simple composition**: Systems are independent and compose through shared data (the workspace filesystem and observation log). No system depends on another system's internals.

## System Index

| System | Doc | Primary tools | Config |
|--------|-----|---------------|--------|
| [Memory](memory.md) | Automatic observation pipeline + searchable index | `memory_search`, `memory_get` | `memory/OBSERVER.md`, `memory/REFLECTOR.md` |
| [Projects](projects.md) | Scoped workspaces for ongoing work | `project_create`, `project_activate`, `project_deactivate`, `project_archive`, `project_list` | per-project `PROJECT.md` |
| [Heartbeats](heartbeats.md) | Ambient scheduled monitoring | *(automatic — no tools)* | `HEARTBEAT.yml` |
| [Inbox](inbox.md) | Capture and triage items | `inbox_list`, `inbox_read`, `inbox_add`, `inbox_archive` | *(none)* |
| [Scheduled Actions](scheduled-actions.md) | One-off future tasks | `schedule_action`, `list_actions`, `cancel_action` | `scheduled_actions.json` |
| [Skills](skills.md) | Loadable instruction modules | `skill_activate`, `skill_deactivate` | per-skill `SKILL.md` |
| [Notifications](notifications.md) | Result routing from background tasks | *(automatic — no tools)* | `CHANNELS.yml`, `config.toml` channels |
| [Background Tasks](background-tasks.md) | Sub-agents and scripts | `subagent_spawn`, `list_agents`, `stop_agent` | `[background]` in `config.toml`, `subagents/` presets |

## What This Is Not

- Not API documentation. Tool parameter schemas live in the code.
- Not onboarding content. The `residuum-getting-started` skill handles first-run UX.
- Not a design rationale. The `docs/*.md` design docs explain *why* decisions were made. This directory explains *how things are meant to work*.
