# Workflow: Understanding the Agent

Explain how the agent works internally, in user-facing terms. This is for users who want to understand the system before committing to using it. Keep it concrete and honest about limitations.

## Step 1: How Memory Works

Explain: "I remember things from our conversations automatically. Here is how it works."

Walk through the memory pipeline in plain terms:
- As we talk, messages accumulate. After enough conversation builds up (roughly a few thousand words), an observer process fires.
- The observer reads our recent conversation and extracts observations: facts, preferences, decisions, context -- anything worth remembering long-term.
- These observations are stored in a searchable index. When you ask me something later, I can search past observations using `memory_search` to find relevant context.
- If you want to see the raw transcript of a past conversation, `memory_get` retrieves it by episode ID.

What the user does NOT need to do:
- They do not need to tell you to remember things.
- They do not need to manage memory files.
- They do not need to worry about memory filling up (the reflector compresses old observations when the log grows large).

What the user CAN do:
- Search memory with `memory_search` to find past conversations.
- Edit `memory/OBSERVER.md` to customize what the observer pays attention to.
- Edit `memory/REFLECTOR.md` to customize how old memories are compressed.

## Step 2: How Context Is Assembled

Explain: "Every time you send me a message, I assemble a context from several sources before responding."

Walk through the context stack:
1. **SOUL.md** -- My core personality and identity. Defines who I am, how I communicate, and my values. You can edit this to change my personality.
2. **AGENTS.md** -- My behavioral rules and capabilities. Defines what I can do and how I should act. Usually left as-is.
3. **USER.md** -- What I know about you. Your preferences, timezone, context about your work and life. I update this as I learn about you, and you can edit it directly.
4. **Memory** -- Recent conversation context and narrative from past observations. Gives me continuity across sessions.
5. **Projects** -- If a project is active, its overview, notes, and configuration are loaded. This scopes my knowledge to what is relevant.
6. **Skills** -- If any skills are activated, their instructions are included. Skills teach me how to handle specific types of tasks.
7. **Tools** -- The list of tools available to me, including any from MCP servers or the active project.

This layered assembly means I see different context depending on what is active. Activating a project loads project-specific knowledge. Activating a skill loads task-specific instructions.

## Step 3: How the Agent Sees Time Passing

Explain: "I am aware of time in a few ways."

- I see the current date and time in my status line at the start of each conversation.
- I see when you last sent a message, so I know how long it has been since we talked.
- My memory observations have timestamps, so I can understand the chronological order of past events.
- Heartbeat pulses fire on a schedule, giving me periodic awareness even when you are not talking to me.
- Scheduled actions fire at specific times, letting me take action at predetermined moments.

Be honest about the limitation: "I do not have a continuous sense of time passing. Between conversations, I am not running. Heartbeats and scheduled actions give me periodic check-ins, but I am not monitoring things in real-time. I wake up, do a task, and go back to waiting."

## Step 4: How Proactivity Works

Explain: "I can do things without you asking, through two mechanisms."

**Heartbeats** (recurring):
- Defined in `HEARTBEAT.yml` with a schedule like "every 30 minutes" or "every 2 hours"
- Each pulse runs one or more task prompts
- Results are routed through `NOTIFY.yml` to channels like inbox, agent feed, or external notifications
- Think of them as cron jobs, but instead of running scripts, they run agent prompts

**Scheduled Actions** (one-off):
- Created with `schedule_action` to fire at a specific future time
- Example: "Remind me to check the deployment at 3pm" creates an action that fires once at 3pm
- After firing, the action is removed
- Managed with `list_actions` and `cancel_action`

Both use background sub-agents to execute, so the main conversation is not interrupted.

## Step 5: What the Agent Can and Cannot Do

Be straightforward about capabilities and limitations.

**Can do:**
- Read, write, and edit files in the workspace
- Run shell commands via `exec`
- Search past memories and conversation history
- Manage projects with scoped context and tools
- Run background tasks and sub-agents
- Schedule one-off actions and recurring heartbeat checks
- Connect to external services via MCP servers
- Deliver notifications through configured channels (inbox, ntfy, webhooks)

**Cannot do:**
- Browse the web or fetch URLs (unless an MCP server provides this)
- Send emails or messages to external platforms (unless connected via MCP or channel plugins)
- Monitor things in real-time (heartbeats are periodic, not continuous)
- Undo actions after they are taken (file writes, shell commands are permanent)
- Access systems it does not have credentials or network access for

Wrap up by asking if the user has questions about any of these systems. Offer to set up any capability they find interesting -- point them to the appropriate workflow.

For the full technical reference, mention: "For detailed documentation on every workspace file and configuration format, see `skill_activate ironclaw-system`."
