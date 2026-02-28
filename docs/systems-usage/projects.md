# Projects

Projects are scoped workspaces for ongoing areas of work. Each project has its own notes, references, tools, MCP servers, and skills. When the agent recognizes that a conversation relates to a project, it activates that project's context using the `project_activate` tool.

## Lifecycle

`project_create` → `project_activate` → `project_deactivate` → `project_archive`

- Only one project can be active at a time. Activating a new project requires deactivating the current one first.
- When a project is active the agent cannot *write* to directories outside of the project, however, it can *read* from any directory freely.
- Archived projects can be reactivated from the deactivated state but are moved out of the main `projects/` directory into `archive/`.
- Discovery is by directory scan — no central registry. The gateway scans `projects/` on startup and on filesystem changes.

## Activation

Activation is agent-autonomous. The agent decides to activate a project based on conversational cues (the user mentions a project by name, discusses related topics, etc.). This is a deliberate tool call (`project_activate`), not automatic pattern matching — the agent uses judgment about when project context is relevant.

**What happens on activation:**
1. PROJECT.md frontmatter and body loaded
2. Manifest built by scanning subdirectories
3. Recent session logs (~2000 tokens from `notes/log/`) loaded for immediate continuity context
4. Tool filter updated to the project's listed tools (plus always-allowed tools)
5. MCP servers from frontmatter started
6. Path policy scoped to the project directory
7. Project skills added to the skill index

Note: Files beyond PROJECT.md are NOT loaded, they must be read by the agent.

## Directory Structure

```
projects/<dir-name>/
  PROJECT.md          # Frontmatter (name, description, status, tools, mcp_servers) + body
  notes/
    log/              # Deactivation logs (YYYY-MM/log-DD.md)
  references/
  workspace/
  skills/             # Project-scoped skills
```

Directory names are sanitized from the project name (lowercased, special chars replaced with hyphens).

## PROJECT.md Frontmatter

```yaml
name: homelab
description: Home server infrastructure and automation
status: active        # active | archived
created: 2026-02-15
tools:                # optional — scoped tool allowlist
  - exec
  - read_file
  - write_file
mcp_servers:          # optional — started on activation
  - name: docker
    command: npx
    args: ["@modelcontextprotocol/server-docker"]
archived:             # optional — set by project_archive
```

## Deactivation

`project_deactivate` requires a non-empty log entry. The gateway enforces this — it cannot be bypassed via prompt. Logs are written to `notes/log/YYYY-MM/log-DD.md`.

This ensures the agent always records what it was working on and what state things are in, which feeds back into the memory system (episodes carry a `context` field tagging the active project).

## Tools

| Tool | Parameters | Notes |
|------|-----------|-------|
| `project_create` | `name`, `description`, optional `tools` | Creates directory structure and PROJECT.md |
| `project_activate` | `name` | Loads context, starts MCP servers, scopes tools |
| `project_deactivate` | `name`, `log` (non-empty) | Writes log entry, tears down project context |
| `project_archive` | `name` | Updates status, sets archived date, moves to `archive/` |
| `project_list` | *(none)* | Lists all projects from the project index |

## Interaction with Other Systems

- **Memory**: Episodes carry a `context` field tagging the active project, enabling filtered search by project via `project_context` on `memory_search`.
- **Skills**: Project skills (in `projects/<name>/skills/`) are discovered during activation and removed when the project deactivates. Project-source skills have **highest** priority in deduplication (project > workspace > user-global > bundled).
- **MCP**: MCP servers defined in PROJECT.md frontmatter use reference counting — multiple sub-agents can have the same project active simultaneously without premature teardown.
- **Background tasks**: If a sub-agent's turn loop ends with a project still active, the gateway force-deactivates with an auto-generated log entry.
