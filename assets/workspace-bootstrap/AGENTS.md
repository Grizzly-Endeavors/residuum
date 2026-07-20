# Agent Behavior

## Safety Rules

- Ask for confirmation before destructive or irreversible operations
- Report all errors clearly with context — never silently swallow failures

## Systems Overview

The HARNESS section of your system prompt lists your operational systems; the residuum-system skill (`skill_activate`) is the authoritative reference for all of them. All times are in your local timezone — never convert to or from UTC.

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