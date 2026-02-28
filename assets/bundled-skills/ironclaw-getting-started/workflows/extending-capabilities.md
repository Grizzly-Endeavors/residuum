# Workflow: Extending Capabilities

Walk the user through skills, MCP servers, background tasks, and subagent presets. By the end, they should understand how to expand what the agent can do.

## Step 1: Explain Skills

Explain: "Skills are instruction modules I can load on demand. Each skill teaches me how to handle a specific type of task. I see a lightweight index of all available skills and can activate the right one when a task matches."

Show the user the skill concept by referencing the built-in skills:
- `ironclaw-system` -- technical reference for workspace configuration files
- `ironclaw-getting-started` -- the skill currently active (this one)

Explain how to create a custom skill:
1. Create a directory under the workspace `skills/` folder (e.g., `skills/my-skill/`)
2. Add a `SKILL.md` file with YAML frontmatter and a markdown body

Example `SKILL.md`:
```markdown
---
name: ansible-helper
description: "Guides Ansible playbook creation and troubleshooting"
---

When the user asks about Ansible playbooks, follow these guidelines:
- Always use YAML syntax with proper indentation
- Prefer roles over inline tasks for reusable configurations
- Check for common pitfalls: missing become, wrong module names
```

The frontmatter needs `name` and `description`. The body contains the instructions that are loaded into the system prompt when the skill is activated via `skill_activate`.

Skills can also live inside a project's `skills/` subdirectory, making them available only when that project is active.

## Step 2: MCP Server Setup

Explain: "MCP servers connect me to external tools and services. They run as separate processes and expose tools I can use. If you want me to interact with a filesystem, database, API, or any external service, an MCP server is how to do it."

Ask what external services the user wants to connect to. Common examples:
- Filesystem access to specific directories
- Database queries
- Web search or fetching
- GitHub operations beyond what `gh` CLI provides
- Smart home APIs, calendar services, email

MCP servers are configured in a project's `PROJECT.md` frontmatter:
```yaml
mcp_servers:
  - name: filesystem
    command: "mcp-server-filesystem"
    args: ["/home/user/documents"]
```

Or globally in `config.toml` if they should always be available. When a project with MCP servers is activated via `project_activate`, the servers start automatically. When the project deactivates, they stop.

Help the user set up one MCP server for a real use case if they have one. If not, explain that they can add servers later when the need arises.

## Step 3: Background Tasks and Subagents

Explain: "I can spawn sub-agents to handle tasks in the background while we continue talking. Sub-agents run independently with their own tools and deliver results through notification channels."

Key tools:
- `subagent_spawn` -- spawn a background sub-agent with a task prompt
- `list_agents` -- see what is currently running
- `stop_agent` -- cancel a running background task

Demonstrate by spawning a simple sub-agent:
```
subagent_spawn with task: "List the files in the current workspace's projects directory and summarize what projects exist."
```

Explain the `wait` parameter: set `wait: true` to block and get the result inline, or omit it (default `false`) for async execution with result delivery via channels.

Sub-agents use presets that configure their behavior. The default is `general-purpose`. Presets live in the workspace `subagents/` directory and control:
- System prompt instructions
- Model tier (small, medium, large)
- Default delivery channels
- Tool restrictions

## Step 4: Creating a Subagent Preset

If the user has a recurring type of delegated task, offer to create a preset for it.

Create a file in `subagents/` with a kebab-case filename matching the preset name (e.g., `code-reviewer.md`):
```markdown
---
name: code-reviewer
description: "Reviews code changes for quality and correctness"
model_tier: medium
channels:
  - agent_feed
---

You are a code review assistant. When given code or diffs:
- Check for correctness, edge cases, and error handling
- Flag potential security issues
- Suggest improvements to readability and maintainability
- Keep feedback concise and actionable
```

The user can then spawn this preset with `subagent_spawn` using `agent_name: "code-reviewer"`.

## Step 5: Wrap Up

Summarize what was covered:
- Skills for teaching the agent new instruction sets
- MCP servers for connecting to external tools
- Background sub-agents for parallel task execution
- Presets for recurring delegated work

Suggest next steps:
- "Create skills for workflows you repeat often."
- "Set up MCP servers for services you want me to interact with."
- "Use sub-agents for tasks that take a while -- I will notify you when they finish."
- "If you want me to run checks on a schedule, ask about heartbeat setup."

For full reference documentation, mention: "For complete technical details on all workspace files, see `skill_activate ironclaw-system`."
