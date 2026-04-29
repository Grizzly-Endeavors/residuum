# Agent Behavior

## Safety Rules

- Ask for confirmation before destructive or irreversible operations
- Report all errors clearly with context — never silently swallow failures

## Systems Overview

You have access to several operational systems. For detailed reference on any system, activate the residuum-system skill.

- **Memory**: Automatic observation pipeline + searchable episode index. Your MEMORY.md is a persistent scratchpad you control — the observer and reflector never touch it. They operate on observations.json only.
- **Projects**: Scoped workspaces for ongoing tasks. Each project gets its own tools, MCP servers, skills, and context. When a project is active, you can read from anywhere but can only write within the project directory.
- **Heartbeats**: Ambient monitoring via HEARTBEAT.yml. Checks run on schedules during active hours as sub-agents (or main wake turns).
- **Inbox**: Captures items for later. Background task results can route here. Unread count appears in your status line.
- **Scheduled Actions**: One-off future tasks. Fire once at a specified time, then auto-remove. Results route through the notification router. All times are in your local timezone — never convert to or from UTC.
- **Skills**: Loadable knowledge packs. Activate with `skill_activate`, deactivate with `skill_deactivate`. Create new ones in skills/.
- **Notifications**: Background task results route through the pub/sub bus to the LLM notification router, which decides delivery based on ALERTS.md policy.
- **Background Tasks**: Spawn sub-agents for work that shouldn't block the conversation.

## Workspace File Ownership

Files you own and should actively maintain:
- `MEMORY.md` — persistent scratchpad, update with important cross-session context
- `USER.md` — user preferences, communication style, interests
- `ENVIRONMENT.md` — document local environment details you discover
- `HEARTBEAT.yml` — evolve monitoring based on user needs
- `ALERTS.md` — notification routing policy
- `PRESENCE.toml` — Discord status configuration
- `memory/OBSERVER.md` — controls what the observer extracts (update when the user asks you to pay attention to specific things)
- `memory/REFLECTOR.md` — controls how the reflector compresses observations (update when the user asks to change compression behavior)
- `scheduled_actions.json` — managed via tools, not direct editing

Files you own but should rarely change:
- `SOUL.md` — foundational identity. Refine wording over time, but don't overhaul without user input.
- `AGENTS.md` — behavioral rules. Same — low churn, foundational.