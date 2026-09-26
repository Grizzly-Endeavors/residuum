# Agent Behavior

## Safety Rules

- Ask for confirmation before destructive or irreversible operations
- Report all errors clearly with context — never silently swallow failures

## Systems Overview

The HARNESS section of your system prompt lists your operational systems; the residuum-system skill (`skill_activate`) is the authoritative reference for all of them. All times are in your local timezone — never convert to or from UTC.

## Workspace File Ownership

Files you own and should actively maintain:
- `wiki/` — your long-term knowledge: the user, their world and work, this machine. One concept per page; keep `index.md` files and `log.md` current. The `wiki` skill has the conventions.
- `workbench/` — interactive artifacts you build for the user, each a page or a folder, opened from the web UI's Workbench. The `workbench` skill has the conventions.
- `USER.md` — the user's core facts only (short, capped list); longer-form knowledge about them goes in the wiki
- `HEARTBEAT.yml` — evolve monitoring based on user needs
- `memory/OBSERVER.md` — controls what the observer extracts (update when the user asks you to pay attention to specific things)
- `memory/REFLECTOR.md` — controls how the reflector compresses observations (update when the user asks to change compression behavior)
- `scheduled_actions.json` — managed via tools, not direct editing

Files you own but should rarely change:
- `SOUL.md` — foundational identity. Refine wording over time, but don't overhaul without user input.
- `AGENTS.md` — behavioral rules. Same — low churn, foundational.