# Projects

Projects are scoped work contexts that isolate tools, MCP servers, file access, and notes under a single directory.

## Lifecycle

```
project_create  →  project_activate  →  project_deactivate  →  project_archive
                        ↑                       |
                        └───────────────────────┘  (reactivate)
```

## PROJECT.md Format

Every project directory contains a `PROJECT.md` with YAML frontmatter:

```yaml
---
name: my-project
description: "Short description of the project"
status: active          # active | archived
created: 2026-02-20
tools:                  # optional — tool names to allow when active
  - exec
  - read_file
mcp_servers:            # optional — MCP servers to enable when active
  - name: my-server
    command: npx
    args: ["-y", "@my/mcp-server"]
    env:
      API_KEY: "..."
archived: ~             # set by project_archive (YYYY-MM-DD)
---

Free-form body text — overview, goals, conventions, etc.
```

## Directory Structure

`project_create` scaffolds:

```
projects/<dir-name>/
├── PROJECT.md
├── notes/log/          # Deactivation logs (YYYY-MM/log-DD.md)
├── references/         # Reference materials
├── workspace/          # Working files
└── skills/             # Project-specific skills
```

## Tools

| Tool | Description |
|------|-------------|
| `project_create` | Create a new project with name, description, and optional tool list. |
| `project_activate` | Load a project's context — frontmatter, body, manifest, tools, MCP servers, path policy. |
| `project_deactivate` | Deactivate the current project. **Requires a non-empty log entry** summarizing what was done. The log is written to `notes/log/YYYY-MM/log-DD.md`. |
| `project_archive` | Update status to `archived`, set the archived date, and move from `projects/` to `archive/`. |
| `project_list` | List all projects (active and archived) from the project index. |

## Activation Details

When a project is activated:
1. `PROJECT.md` frontmatter and body are loaded.
2. A manifest is built by scanning subdirectories (`notes`, `references`, `workspace`, `skills`).
3. The tool filter is updated to include only the tools listed in `tools:` (plus always-allowed tools).
4. Any `mcp_servers` entries are registered.
5. The path policy is scoped to the project directory.

Only one project can be active at a time. Activating a new project requires deactivating the current one first.

## Gotchas

- Directory names are sanitized from the project name: lowercased, special characters replaced with hyphens, consecutive hyphens collapsed, leading/trailing hyphens trimmed.
- `project_deactivate` **will reject empty log entries**. Always describe what was accomplished.
- The project index scans both `projects/` and `archive/` on startup. Invalid or missing frontmatter is warned and skipped.
- Project skills are discovered during activation and merged into the skill index (project source takes lowest priority in dedup).
