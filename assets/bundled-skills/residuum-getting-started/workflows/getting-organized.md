# Workflow: Getting Organized

Walk the user through the inbox and memory. By the end, they should understand how the inbox captures things that need attention and how memory works passively in the background.

**Remember**: Write to `USER.md` and `MEMORY.md` as you learn things throughout this workflow — don't save it all for the end. If the user mentions a preference, a tool they use, or context about their life, write it down immediately.

## Step 1: Introduce the Inbox

Explain: "The inbox is a place to capture things quickly without losing them. You can tell me to add something to the inbox, and I will save it. You can also review and archive items later."

Show them the inbox with `inbox_list`. Explain that items show as `[unread]` until they are reviewed, and can be archived with `inbox_archive` when done. Items arrive from background tasks and heartbeats via the notification router — the agent doesn't add items directly.

Mention that background tasks and heartbeats can also deliver results to the inbox, so it becomes a central place for things that need attention.

## Step 2: Explain How Memory Works

Explain: "You do not need to do anything special for memory. I automatically remember important things from our conversations. After we talk for a while, I extract observations -- facts, preferences, decisions -- and store them in a searchable index."

Key points to convey:
- Memory is passive. The user does not need to tell you to remember things.
- Observations are extracted after enough conversation accumulates.
- You can search past observations with `memory_search` if they want to find something specific.
- USER.md stores stable preferences (timezone, communication style, context about them). Memory stores episodic information (what happened, what was discussed).

Do not go deep into observer/reflector internals unless the user asks.

## Step 3: Wrap Up

Summarize what was set up:
- The inbox as a capture tool
- Memory working passively in the background

Suggest next steps:
- "If you want me to monitor things for you, ask about heartbeat setup."

For deeper technical detail on the inbox and memory, activate `residuum-system`.
