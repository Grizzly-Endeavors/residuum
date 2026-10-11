---
name: residuum-system
description: Reference documentation for all Residuum workspace systems — config, memory, heartbeats, inbox, actions, skills, MCP, notifications, background tasks, and subconscious.
---

# Residuum System Reference

This skill provides reference documentation for every major workspace system. Activate it when you need to understand how a system works, what tools are available, or what file formats to use.

## Quick Reference

| System | Tools | Config File | Reference |
|--------|-------|-------------|-----------|
| Config | `write_file`, `edit_file` | `config.toml`, `providers.toml` | [config](references/config.md) |
| Memory | `memory_search`, `memory_get` | `memory/OBSERVER.md`, `memory/REFLECTOR.md` | [memory-system](references/memory-system.md) |
| Checkpoints | `workspace_history`, `workspace_restore` | `~/.residuum/hub/checkpoints/` (outside the workspace) | [checkpoints](references/checkpoints.md) |
| Wiki | `read_file`, `write_file`, `edit_file` | `team/wiki/` (your role page is `team/wiki/agents/<name>.md`) | the `wiki` skill |
| Workbench | `read_file`, `write_file`, `edit_file` | `team/workbench/` (shared by every agent) | the `workbench` skill |
| Heartbeats | *(none — runs automatically)* | `HEARTBEAT.yml` | [heartbeats](references/heartbeats.md) |
| Inbox | `inbox_list`, `inbox_read`, `inbox_archive` | *(none)* | [inbox](references/inbox.md) |
| Scheduled Actions | `schedule_action`, `list_actions`, `cancel_action` | `scheduled_actions.json` | [scheduled-actions](references/scheduled-actions.md) |
| Skills | `skill_activate`, `skill_deactivate` | per-skill `SKILL.md` | [skills](references/skills.md) |
| Agent Keys | `agent_keys_list`, `agent_key_delete`, `exec` (`keys`, `store_output_as`) | `residuum agent-keys` CLI, Settings → All agents → Saved keys | [agent-keys](references/agent-keys.md) |
| Team files | `read_file`, `write_file`, `edit_file` (`team/...` paths) | `team/` (shared by every agent) | [team-files](references/team-files.md) |
| Tool PATH | `exec` (uses it) | `[tools]` in config.toml, `~/.residuum/hub/bin` | [tools](references/tools.md) |
| MCP | *(none — surfaced as regular tools)* | `config/mcp.json` | [mcp](references/mcp.md) |
| Notifications | `list_endpoints`, `list_conversations`, `switch_endpoint`, `send_message` | `config/channels.toml` | [notifications](references/notifications.md) |
| Background Tasks | `subagent_spawn`, `list_agents`, `stop_agent`, `message_agent` (also teammates: `agent:<name>`), `agent_create`, `agent_delete` | `[background]` in config.toml (idle timeouts, depth cap; the shared budget and hop limits are in the hub config) | [background-tasks](references/background-tasks.md) |
| Subconscious | *(none — automatic)* | `SUBCONSCIOUS.md`, `[subconscious]` in config.toml | [subconscious](references/subconscious.md) |
| A2A | `list_agents`, `message_agent`, `stop_agent` (address `a2a:<name>`); `a2a_task_update` (session-only, in `a2a` conversation sessions) | `config/agent-card.json`, `config/a2a.json`, `[a2a]` in config.toml | [a2a](references/a2a.md) |

## Agent Directory Layout

```
<agent>/
├── SOUL.md                  # Core identity and personality
├── BOOTSTRAP.md             # First-run guidance (deleted after first conversation)
├── HEARTBEAT.yml            # Pulse scheduling
├── scheduled_actions.json   # Persisted one-off actions
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
├── skills/                  # This agent's own skills (also sub-agent roles); team-wide skills live in team/skills/
├── archive/                 # Archived items
│   └── inbox/               # Archived inbox items
└── inbox/                   # Active inbox items
```

The team layer sits beside the agent directory and is shared by every agent. Reach it from file tools with the `team/` prefix:

```
team/
├── AGENTS.md                # Team-wide behavior rules
├── USER.md                  # User preferences (core facts only)
└── wiki/                    # Open Knowledge Format knowledge base (agent-maintained)
    ├── index.md             # Root index — the only page injected into prompts
    ├── log.md               # Append-only change log
    └── agents/              # One role page per agent (you maintain yours), plus index.md
```
