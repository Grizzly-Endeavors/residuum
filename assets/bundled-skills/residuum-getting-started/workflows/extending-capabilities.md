# Workflow: Extending Capabilities

Walk the user through skills, MCP servers, and background tasks. By the end, the user should understand how to ask you to expand your capabilities.

**Remember**: Write core facts to `USER.md` and everything else to wiki pages (activate the `wiki` skill) as you learn things throughout this workflow — don't save it all for the end.

## Step 1: Explain Skills

Explain: "Skills are instruction modules I can load on demand. Each skill teaches me how to handle a specific type of task. I see a lightweight index of all available skills and can activate the right one when a task matches."

Show the user the skill concept by referencing the built-in skills:
- `residuum-system` -- technical reference for workspace configuration files
- `residuum-getting-started` -- the skill currently active (this one)

Explain that the user can ask you to create custom skills for recurring types of tasks. For example: "Create a skill for Ansible playbook review" and you will set it up.

Give an example of what a skill looks like so they understand the concept, but frame it as something you create for them:

"If you asked me to create an Ansible helper skill, I would set up something like this -- a name, a description, and instructions I follow when the skill is active."

## Step 2: MCP Server Setup

Explain: "MCP servers connect me to external tools and services. They run as separate processes and expose tools I can use. If you want me to interact with a filesystem, database, API, or any external service, an MCP server is how to do it."

Ask what external services the user wants to connect to. Common examples:
- Filesystem access to specific directories
- Database queries
- Web search or fetching
- GitHub operations beyond what `gh` CLI provides
- Smart home APIs, calendar services, email

You configure MCP servers in `config/mcp.json`, using the same `mcpServers` map format Claude Code and Claude Desktop use. Servers listed there start automatically and stay running; editing the file and saving is enough for the change to take effect, no restart needed.

Help the user set up one MCP server for a real use case if they have one. If not, explain that they can ask you to set one up later when the need arises.

## Step 3: Background Tasks and Subagents

Explain: "I can spawn sub-agents to handle tasks in the background while we continue talking. Sub-agents run independently with their own tools and deliver results through notification channels."

Key tools:
- `subagent_spawn` -- spawn a background sub-agent with a task prompt
- `list_agents` -- see what is currently running
- `stop_agent` -- cancel a running background task

Demonstrate by spawning a simple sub-agent:
```
subagent_spawn with task: "List the files in the current workspace and summarize what's there."
```

You can run sub-agents in the foreground (wait for the result inline) or in the background (results delivered via notification channels). Demonstrate both.

A sub-agent can take a skill as its role. By default it runs on the task prompt alone, but you can write a skill for a recurring type of work and hand it to the sub-agent.

## Step 4: Creating a Subagent Preset

If the user has a recurring type of delegated task, offer to write a skill for it. For example: "If you want me to always review code a certain way, I can write a code-reviewer skill and run a sub-agent with it whenever you ask for a review."

Write the skill on their behalf, and pick the model tier when you spawn it. Explain what you created and why. The user does not need to know the file format — just that the skill exists and what it does.

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

For full reference documentation, activate `residuum-system`.
