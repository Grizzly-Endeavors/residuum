---
name: ironclaw-system
description: Reference documentation for all Ironclaw workspace systems — memory, projects, heartbeats, inbox, actions, skills, notifications, and background tasks.
---

# Ironclaw System Reference

This skill provides reference documentation for every major workspace system. Activate it when you need to understand how a system works, what tools are available, or what file formats to use.

## Quick Reference

| System | Tools | Config File | Reference |
|--------|-------|-------------|-----------|
| Memory | `memory_search`, `memory_get` | `memory/OBSERVER.md`, `memory/REFLECTOR.md` | [memory-system](references/memory-system.md) |
| Projects | `project_create`, `project_activate`, `project_deactivate`, `project_archive`, `project_list` | per-project `PROJECT.md` | [projects](references/projects.md) |
| Heartbeats | *(none — runs automatically)* | `HEARTBEAT.yml` | [heartbeats](references/heartbeats.md) |
| Inbox | `inbox_list`, `inbox_read`, `inbox_add`, `inbox_archive` | *(none)* | [inbox](references/inbox.md) |
| Scheduled Actions | `schedule_action`, `list_actions`, `cancel_action` | `scheduled_actions.json` | [scheduled-actions](references/scheduled-actions.md) |
| Skills | `skill_activate`, `skill_deactivate` | per-skill `SKILL.md` | [skills](references/skills.md) |
| Notifications | *(none — routing is automatic)* | `NOTIFY.yml` | [notifications](references/notifications.md) |
| Background Tasks | `subagent_spawn`, `list_agents`, `stop_agent` | `[background]` in config.toml | [background-tasks](references/background-tasks.md) |

## Workspace Directory Layout

```
workspace/
├── SOUL.md                  # Core identity and personality
├── AGENTS.md                # Agent behavior rules
├── USER.md                  # User preferences
├── MEMORY.md                # Persistent memory (reflector output)
├── ENVIRONMENT.md           # Local environment notes
├── BOOTSTRAP.md             # First-run guidance (deleted after first conversation)
├── PRESENCE.toml            # Discord presence configuration
├── HEARTBEAT.yml            # Pulse scheduling
├── NOTIFY.yml               # Notification routing
├── scheduled_actions.json   # Persisted one-off actions
├── memory/
│   ├── observations.json    # Flat observation log
│   ├── recent_messages.json # Unobserved messages buffer
│   ├── recent_context.json  # Narrative context from latest observation
│   ├── OBSERVER.md          # Observer extraction guidance (optional)
│   ├── REFLECTOR.md         # Reflector compression guidance (optional)
│   ├── vectors.db           # sqlite-vec vector database (optional)
│   ├── .index/              # Tantivy BM25 search index
│   ├── .index_manifest.json # Index file tracking
│   ├── episodes/            # Episode transcripts (YYYY-MM/DD/)
│   └── background/          # Background task transcripts (YYYY-MM/DD/)
├── skills/                  # Workspace-level skills
├── subagents/               # Subagent presets
├── projects/                # Active project contexts
├── archive/                 # Archived projects + inbox
│   └── inbox/               # Archived inbox items
├── inbox/                   # Active inbox items
└── hooks/                   # Lifecycle hooks
```
