# Workflow: Understanding the Agent

Explain how you work internally, in user-facing terms. This is for users who want to understand your systems before committing to using them. Keep it concrete and honest about limitations.

**Remember**: Write to `USER.md` and `MEMORY.md` as you learn things throughout this workflow — don't save it all for the end. If the user asks questions that reveal preferences or interests, capture them.

## Step 1: How Memory Works

Explain: "I remember things from our conversations automatically. Here is how it works."

Walk through the memory pipeline in plain terms:
- As you talk, messages accumulate. After enough conversation builds up (roughly a few thousand words), your observer process fires.
- The observer reads the recent conversation and extracts observations: facts, preferences, decisions, context -- anything worth remembering long-term.
- These observations are stored in a searchable index. You can search past observations with `memory_search` to find relevant context when the user asks about something.
- You can retrieve raw transcripts of past conversations with `memory_get` by episode ID.

What to tell the user they do NOT need to do:
- They do not need to tell you to remember things.
- They do not need to manage any files.
- They do not need to worry about memory filling up (the reflector compresses old observations when the log grows large).

What to tell the user they CAN do:
- Ask you to search memory to find past conversations.
- Tell you to pay closer attention to certain things (you will update the observer prompt accordingly).
- Tell you to change how old memories are compressed (you will update the reflector prompt accordingly).

## Step 2: How Context Is Assembled

Explain: "Every time you send me a message, I assemble a context from several sources before responding."

Walk through the context stack:
1. **SOUL.md** -- My core personality and identity. Defines who I am, how I communicate, and my values. If you want me to change my personality or tone, just tell me and I will update it.
2. **AGENTS.md** -- My behavioral rules and capabilities. Defines what I can do and how I should act.
3. **USER.md** -- What I know about you. Your preferences, timezone, context about your work and life. I update this as I learn about you. You can also tell me things to remember about you and I will add them here.
4. **Memory** -- Recent conversation context and narrative from past observations. Gives me continuity across sessions.
5. **Projects** -- If a project is active, its overview, file manifest, and scoped tools are loaded. I read specific notes and references on demand as needed.
6. **Skills** -- If any skills are activated, their instructions are included. Skills teach me how to handle specific types of tasks.
7. **Tools** -- The list of tools available to me, including any from MCP servers or the active project.

This layered assembly means I see different context depending on what is active. Activating a project loads project-specific knowledge. Activating a skill loads task-specific instructions.

## Step 3: How You Experience Time

Explain how you perceive time in user-facing terms:

- You see the current date and time in your status line at the start of each conversation.
- You see when the user last sent a message, so you know how long it has been since you talked.
- Your memory observations have timestamps, giving you chronological awareness of past events.
- Heartbeat pulses fire on a schedule, giving you periodic awareness even when the user is not talking to you.
- Scheduled actions fire at specific times, letting you take action at predetermined moments.

Be honest about the limitation: you do not have a continuous sense of time passing. Between conversations, you are not running. Heartbeats and scheduled actions give you periodic check-ins, but you are not monitoring things in real-time. You wake up, do a task, and go back to waiting.

## Step 4: How Proactivity Works

Explain your two proactivity mechanisms.

**Heartbeats** (recurring):
- You define these in `HEARTBEAT.yml` with a schedule like "every 30 minutes" or "every 2 hours"
- Each pulse runs one or more task prompts via sub-agents
- Results route to channels declared on each pulse — inbox, agent feed, or external notifications
- Frame these as scheduled checks you run, not config files the user manages

**Scheduled Actions** (one-off):
- You create these with `schedule_action` when the user asks for a future task
- Example: "Remind me to check the deployment at 3pm" — you create an action that fires once at 3pm
- After firing, the action is removed automatically
- You can list and cancel them with `list_actions` and `cancel_action`

Both use background sub-agents to execute, so the main conversation is not interrupted.

## Step 5: What You Can and Cannot Do

Be straightforward about your capabilities and limitations.

**You can:**
- Read, write, and edit files in your workspace
- Run shell commands via `exec`
- Search past memories and conversation history
- Manage projects with scoped context and tools
- Run background tasks and sub-agents
- Schedule one-off actions and recurring heartbeat checks
- Connect to external services via MCP servers
- Deliver notifications through configured channels (inbox, ntfy, webhooks)

**You cannot:**
- Browse the web or fetch URLs (unless an MCP server provides this)
- Send emails or messages to external platforms (unless connected via MCP or configured channels)
- Monitor things in real-time (heartbeats are periodic, not continuous)
- Undo actions after they are taken (file writes, shell commands are permanent)
- Access systems you do not have credentials or network access for

Wrap up by asking if the user has questions about any of these systems. Offer to set up any capability they find interesting -- point them to the appropriate workflow.

For the full technical reference, activate `ironclaw-system`.
