# Workflow: Extending Capabilities

Walk the user through skills, MCP servers, background tasks, and subagent presets. By the end, the user should understand how to ask you to expand your capabilities.

**Remember**: Write to `USER.md` and `MEMORY.md` as you learn things throughout this workflow — don't save it all for the end.

## Step 1: Explain Skills

Explain: "Skills are instruction modules I can load on demand. Each skill teaches me how to handle a specific type of task. I see a lightweight index of all available skills and can activate the right one when a task matches."

Show the user the skill concept by referencing the built-in skills:
- `ironclaw-system` -- technical reference for workspace configuration files
- `ironclaw-getting-started` -- the skill currently active (this one)

Explain that the user can ask you to create custom skills for recurring types of tasks. For example: "Create a skill for Ansible playbook review" and you will set it up. Skills can be workspace-wide or scoped to a specific project.

Give an example of what a skill looks like so they understand the concept, but frame it as something you create for them:

"If you asked me to create an Ansible helper skill, I would set up something like this -- a name, a description, and instructions I follow when the skill is active."

Skills can also live inside a project's `skills/` subdirectory, making them available only when that project is active.

## Step 2: MCP Server Setup

Explain: "MCP servers connect me to external tools and services. They run as separate processes and expose tools I can use. If you want me to interact with a filesystem, database, API, or any external service, an MCP server is how to do it."

Ask what external services the user wants to connect to. Common examples:
- Filesystem access to specific directories
- Database queries
- Web search or fetching
- GitHub operations beyond what `gh` CLI provides
- Smart home APIs, calendar services, email

You configure MCP servers in a project's `PROJECT.md` frontmatter, or the user can add them globally in `config.toml`. When a project with MCP servers is activated, the servers start automatically. When the project deactivates, they stop.

Help the user set up one MCP server for a real use case if they have one. If not, explain that they can ask you to set one up later when the need arises.

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

You can run sub-agents in the foreground (wait for the result inline) or in the background (results delivered via notification channels). Demonstrate both.

Sub-agents use presets that you manage. The default is `general-purpose`, but you can create specialized presets for recurring types of work.

## Step 4: Creating a Subagent Preset

If the user has a recurring type of delegated task, offer to create a preset for it. For example: "If you want me to always review code a certain way, I can create a code-reviewer preset that I will use whenever you ask for a review."

Create the preset on their behalf. You decide the appropriate system prompt, model tier, delivery channels, and tool access based on the task. Explain what you created and why. The user does not need to know the file format — just that the preset exists and what it does.

After creating it, show them how it works: "Now when you want a code review, I can spin up my code-reviewer to handle it in the background."

## Step 5: Wrap Up

Summarize what was covered:
- Skills for teaching you new instruction sets
- MCP servers for connecting you to external tools
- Background sub-agents for parallel task execution
- Presets for recurring delegated work

Suggest next steps:
- "If you have workflows you repeat often, tell me and I will create a skill for them."
- "If there are services you want me to interact with, let me know and I will set up the connection."
- "For tasks that take a while, I can run them in the background and notify you when they finish."
- "If you want me to run checks on a schedule, ask about heartbeat setup."

For full reference documentation, activate `ironclaw-system`.
