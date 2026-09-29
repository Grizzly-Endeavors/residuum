# Team Directory

Every agent on a hub shares one team directory, `~/.residuum/team/`, next to `hub/` and the agents' own directories. It holds what belongs to the whole team rather than to one agent: the team's behavioral rules, the user's core facts, the knowledge wiki, the shared workbench, and the shared skills.

```
team/
├── AGENTS.md             # Behavioral rules every agent follows
├── USER.md               # The user's core facts (short, capped list)
├── wiki/                 # The one shared Open Knowledge Format wiki
│   ├── index.md          # Root catalog
│   ├── log.md            # Append-only history of wiki changes
│   └── agents/
│       ├── index.md      # Roster: one line per agent
│       └── <name>.md     # One role page per agent
├── workbench/            # Workbench artifacts shared by every agent
└── skills/               # Shared skills, including the bundled ones
```

An agent keeps everything else in its own directory: `SOUL.md`, `HEARTBEAT.yml`, `SUBCONSCIOUS.md`, `BOOTSTRAP.md` (the first agent only, until its getting-started interview is done), `memory/`, the inboxes, and `config/`.

## Bootstrap

The team directory is created and filled by the same bootstrap that prepares an agent's workspace, so it happens at onboarding (the setup wizard, the CLI `residuum setup`, and the web UI's complete-setup) and again on every start:

- The directories above are created if missing, and the bundled skills are written into `skills/` when missing.
- `AGENTS.md`, `USER.md`, `wiki/index.md`, `wiki/log.md` and `wiki/agents/index.md` are written only when missing. A file that exists is never modified, so edits by the user or an agent survive every restart.
- Onboarding writes the user's name (and the timezone) into `USER.md` when it is first created. Later starts do not touch it.
- The running agent's role page is written if missing.

## Role pages

Each agent has a role page at `wiki/agents/<name>.md`:

```markdown
---
type: Agent
title: scout
description: "Role not described yet; this agent fills it in."
---

# scout

scout maintains this page with its role and responsibilities: what it is for, what it owns, and what teammates should hand it.
```

The `description` is the agent's one-line role. The agent owns the page and keeps it accurate. Creating a role page also adds `- [name](/agents/name.md) — description` to `wiki/agents/index.md` and appends an `edit` entry to `wiki/log.md`. Creation is idempotent: when the page already exists nothing is written, so the agent's own words are never replaced. The page is created with an optional description (a placeholder otherwise); the description is collapsed to one line.

## Pulses

The first agent's `HEARTBEAT.yml` carries the built-in `reflection`, `memory_tending` and `wiki_lint` pulses. `wiki_lint` checks the shared wiki, so only one agent runs it. The template for created agents carries `reflection` and `memory_tending` without `wiki_lint`; moving `wiki_lint` to another agent means editing both agents' `HEARTBEAT.yml`.

## Checkpoints

The team directory has its own checkpoint repository, `hub/checkpoints/team.git`, tracking `team/` minus the wiki search index (`.index/`), the vector store (`vectors.db` and its sidecars), and atomic-write temp files. See [Checkpoints](checkpoints.md).
