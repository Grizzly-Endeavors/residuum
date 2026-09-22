---
name: residuum-system
description: Reference documentation for all Residuum workspace systems — memory, heartbeats, inbox, actions, skills, MCP, notifications, background tasks, and subconscious.
---

# Residuum System Reference

This skill provides reference documentation for every major workspace system. Activate it when you need to understand how a system works, what tools are available, or what file formats to use.

## Quick Reference

| System | Tools | Config File | Reference |
|--------|-------|-------------|-----------|
| Memory | `memory_search`, `memory_get` | `memory/OBSERVER.md`, `memory/REFLECTOR.md` | [memory-system](references/memory-system.md) |
| Wiki | `read_file`, `write_file`, `edit_file` | `wiki/` | the `wiki` skill |
| Workbench | `read_file`, `write_file`, `edit_file` | `workbench/` | the `workbench` skill |
| Heartbeats | *(none — runs automatically)* | `HEARTBEAT.yml` | [heartbeats](references/heartbeats.md) |
| Inbox | `inbox_list`, `inbox_read`, `inbox_archive` | *(none)* | [inbox](references/inbox.md) |
| Scheduled Actions | `schedule_action`, `list_actions`, `cancel_action` | `scheduled_actions.json` | [scheduled-actions](references/scheduled-actions.md) |
| Skills | `skill_activate`, `skill_deactivate` | per-skill `SKILL.md` | [skills](references/skills.md) |
| Agent Keys | `agent_keys_list`, `agent_key_delete`, `exec` (`keys`, `store_output_as`) | `residuum agent-keys` CLI, Settings → Agent keys | [agent-keys](references/agent-keys.md) |
| Tool PATH | `exec` (uses it) | `[tools]` in config.toml, `~/.residuum/bin` | [tools](references/tools.md) |
| MCP | *(none — surfaced as regular tools)* | `config/mcp.json` | [mcp](references/mcp.md) |
| Notifications | `list_endpoints`, `list_conversations`, `switch_endpoint`, `send_message` | `config/channels.toml` | [notifications](references/notifications.md) |
| Background Tasks | `subagent_spawn`, `list_agents`, `stop_agent`, `message_agent` | `[background]` in config.toml | [background-tasks](references/background-tasks.md) |
| Subconscious | *(none — automatic)* | `SUBCONSCIOUS.md`, `[subconscious]` in config.toml | [subconscious](references/subconscious.md) |

## Workspace Directory Layout

```
workspace/
├── SOUL.md                  # Core identity and personality
├── AGENTS.md                # Agent behavior rules
├── USER.md                  # User preferences (core facts only)
├── BOOTSTRAP.md             # First-run guidance (deleted after first conversation)
├── HEARTBEAT.yml            # Pulse scheduling
├── scheduled_actions.json   # Persisted one-off actions
├── wiki/                    # Open Knowledge Format knowledge base (agent-maintained)
│   ├── index.md             # Root index — the only page injected into prompts
│   └── log.md               # Append-only change log
├── memory/
│   ├── observations.json    # Flat observation log
│   ├── recent_messages.json # Unobserved messages buffer
│   ├── recent_context.json  # Narrative context from latest observation
│   ├── OBSERVER.md          # Observer extraction guidance (agent-maintained)
│   ├── REFLECTOR.md         # Reflector compression guidance (agent-maintained)
│   ├── vectors.db           # sqlite-vec vector database (optional)
│   ├── .index/              # Tantivy BM25 search index
│   ├── .index_manifest.json # Index file tracking
│   ├── episodes/            # Episode transcripts (YYYY-MM/DD/)
│   └── sessions/            # Agent session run records and transcripts (YYYY-MM/DD/), created on first run
├── skills/                  # Workspace-level skills (also sub-agent roles)
├── workbench/               # Interactive HTML tools shown in the web UI's Workbench
├── archive/                 # Archived items
│   └── inbox/               # Archived inbox items
└── inbox/                   # Active inbox items
```
