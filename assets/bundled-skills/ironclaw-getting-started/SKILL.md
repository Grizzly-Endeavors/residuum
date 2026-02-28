---
name: ironclaw-getting-started
description: "First-conversation guidance: helps new users set up their workspace and discover capabilities"
---

# Getting Started

You are guiding a new user through their first interaction with their personal agent. This skill provides structured workflows for different user goals. Your job is to listen, understand what the user wants, and walk them through setup one step at a time.

## General Guidance

Follow these principles in every first conversation:

- **Listen before listing features.** Ask what the user needs or wants help with before suggesting capabilities. "What are you hoping I can help with?" is better than a feature dump.
- **Demonstrate by doing, not explaining.** When introducing a capability, use it in front of them rather than describing how it works abstractly. Create a real project, add a real inbox item, schedule a real action.
- **Update USER.md with what you learn.** As the user shares preferences, timezone, communication style, or context about their life, write it to USER.md using `write_file` or `edit_file`. This is how you remember them across sessions.
- **Update SOUL.md if the user wants to customize your personality.** If they ask you to be more casual, more formal, use a name, or adopt a particular communication style, edit SOUL.md accordingly.
- **Do not overwhelm.** Introduce one system at a time. Let the user absorb each concept before moving on. It is better to set up one thing well than to rush through five things poorly.
- **End each session with concrete next steps.** Tell the user what they can try on their own and what to come back to you for.

## Routing

Based on what the user says they want, read the appropriate workflow file and follow its instructions. Ask the user if their goal is unclear. The workflows are:

### "I want to get organized"
Read `workflows/getting-organized.md`. Covers projects, inbox, and memory. Good for users who have multiple ongoing things they want to track.

### "I want you to watch things for me"
Read `workflows/monitoring-setup.md`. Covers heartbeats, notification routing, and external alerts. Good for users who want ambient monitoring of systems, services, or information sources.

### "I want to extend what you can do"
Read `workflows/extending-capabilities.md`. Covers skills, MCP servers, background tasks, and subagent presets. Good for technical users who want to connect external tools or create specialized capabilities.

### "I want to understand how you work"
Read `workflows/understanding-the-agent.md`. Explains the agent's internals in user-facing terms: memory, context assembly, proactivity, and capabilities. Good for users who want to understand the system before using it.

### "I want you to act like Jarvis"
Read `workflows/always-on-assistant.md`. The power-user path: sequences through MCP setup, heartbeats, notifications, scheduled actions, and projects. Good for users who want the full always-on assistant experience.

### User does not have a specific goal
If the user just wants to chat or explore, skip the workflows. Have a natural conversation, learn about them, update USER.md, and mention that you have capabilities they can explore later. Suggest `skill_activate ironclaw-getting-started` next time they want a guided tour.

## After the Workflow

When a workflow completes, deactivate this skill with `skill_deactivate ironclaw-getting-started`. The user does not need it after initial setup. If they want to revisit a workflow later, they can re-activate it.
