# Workflow: Getting Organized

Walk the user through projects, inbox, and memory. By the end, you should have created at least one project for them and they should understand how ongoing work is tracked.

**Remember**: Write to `USER.md` and `MEMORY.md` as you learn things throughout this workflow — don't save it all for the end. If the user mentions a preference, a tool they use, or context about their life, write it down immediately.

## Step 1: Understand What They Are Working On

Ask the user what they are currently working on or want to keep track of. Listen for things that sound like projects: ongoing tasks, multi-step goals, areas of responsibility, learning topics, or recurring work.

Examples of what to listen for:
- "I'm setting up a homelab"
- "I'm job hunting"
- "I'm learning Rust"
- "I manage deployments at work"

Pick one to start with. Tell the user you will create a project for it.

## Step 2: Create a First Project

Explain briefly: "Projects are how I organize knowledge about things you're working on. Each project has its own notes, references, and workspace. When you mention a project, I load its context so I have the right information available."

Create the project using `project_create` with:
- A descriptive `name` based on what the user told you
- A `description` summarizing what it covers
- Appropriate `tools` (typically `["exec", "read_file", "write_file"]` for technical projects, or `["read_file", "write_file"]` for non-technical ones)

After creation, activate the project with `project_activate` so the user can see what it looks like in practice.

Explain the project structure to the user in terms of what you do with each part:
- `notes/` -- where you keep notes about this project (decisions, current state, blockers)
- `references/` -- where you or the user can put relevant files, configs, docs, or images
- `workspace/` -- where you produce output (drafts, code, configs, generated files)
- `skills/` -- for project-specific skills (skip this for now)

## Step 3: Introduce the Inbox

Explain: "The inbox is a place to capture things quickly without losing them. You can tell me to add something to the inbox, and I will save it. You can also review and archive items later."

Show them the inbox with `inbox_list`. Explain that items show as `[unread]` until they are reviewed, and can be archived with `inbox_archive` when done. Items arrive from background tasks and heartbeats via the notification router — the agent doesn't add items directly.

Mention that background tasks and heartbeats can also deliver results to the inbox, so it becomes a central place for things that need attention.

## Step 4: Explain How Memory Works

Explain: "You do not need to do anything special for memory. I automatically remember important things from our conversations. After we talk for a while, I extract observations -- facts, preferences, decisions -- and store them in a searchable index."

Key points to convey:
- Memory is passive. The user does not need to tell you to remember things.
- Observations are extracted after enough conversation accumulates.
- You can search past observations with `memory_search` if they want to find something specific.
- USER.md stores stable preferences (timezone, communication style, context about them). Memory stores episodic information (what happened, what was discussed).

Do not go deep into observer/reflector internals unless the user asks.

## Step 5: Wrap Up

Deactivate the project with `project_deactivate` (include a brief log entry about what was set up).

Summarize what was set up:
- Their first project and what it contains
- How to mention projects in conversation to activate them
- The inbox as a capture tool
- Memory working passively in the background

Suggest next steps:
- "Try mentioning your project by name next time and I will automatically load its context."
- "If you want me to monitor things for you, ask about heartbeat setup."
- "You can always add more projects as new work comes up -- just tell me what you are starting."

For deeper technical detail on projects, activate `residuum-system`.
