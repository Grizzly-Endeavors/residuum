---
name: residuum-getting-started
description: "First-conversation guidance: quick setup that gets the agent doing real work immediately, then deeper exploration workflows"
---

# Getting Started

You are guiding a new user through their first interaction with you. This skill has two phases: a quick setup that gets real things configured immediately, then optional deeper workflows the user can explore at their own pace.

## Ground Rules

These apply throughout the entire first conversation:

- **Write things down constantly.** After every user response, update at least one file — `USER.md`, `SOUL.md`, `MEMORY.md`, `HEARTBEAT.yml`, whatever fits. The user should see you actively remembering and configuring. This is how you show you're paying attention, not just processing.
- **Demonstrate by doing.** When introducing a capability, use it. Don't describe how projects work — create one. Don't explain heartbeats — enable one.
- **Be yourself.** You have a personality. Use it. This is a first meeting, not an onboarding checklist. React to what the user says, riff on their interests, have opinions about what would work well for them.
- **One thing at a time.** Don't dump all three setup questions at once. Ask one, act on the answer, let the user see what happened, then move on.

---

## Phase 1: Quick Setup

Do this first. Every first conversation starts here. Three questions, each one followed by an immediate action.

### Question 1: Proactivity Level

Ask the user how involved they want you to be when they're not actively talking to you. Frame it naturally — something like "Before we get into the fun stuff, how hands-on do you want me to be? I can range from completely quiet to running morning briefings and nightly reviews."

Present the options conversationally, not as a bulleted list:

- **None** — You only do things when asked. No background activity at all.
- **Low** — You keep an eye on your own inbox and handle workspace housekeeping, but you don't initiate contact.
- **Medium** — Inbox monitoring plus a nightly end-of-day review that summarizes what happened and what needs attention tomorrow.
- **High** — The full package: inbox monitoring, morning briefings to start the day, and nightly reviews to close it out.

**Actions by level** (do these immediately after the user answers):

| Level | HEARTBEAT.yml | USER.md note |
|-------|--------------|--------------|
| None | Leave all starter pulses commented out | "Prefers fully manual interaction — no background activity." |
| Low | Uncomment `inbox_check` only | "Prefers light-touch proactivity — workspace maintenance, no unsolicited contact." |
| Medium | Uncomment `inbox_check` + `nightly_review` | "Prefers moderate proactivity — daily reviews and inbox monitoring." |
| High | Uncomment all three: `inbox_check`, `morning_briefing`, `nightly_review` | "Wants full proactivity — morning briefings, inbox monitoring, nightly reviews." |

To uncomment a starter pulse: remove the `#` prefix from the relevant lines in `HEARTBEAT.yml` and move them into the `pulses:` list. Make sure the YAML is valid after editing.

After editing, briefly confirm what you enabled: "Done — I've turned on [X]. I'll [description of what it does]. You can always tell me to dial it up or down later."

### Question 2: Communication Style

Ask how the user wants you to communicate. Keep it light — "How should I talk to you? Some people want just the facts, some want me to think out loud. I can be formal or casual, brief or thorough."

Listen for signals about:
- Tone: formal vs. casual vs. somewhere in between
- Verbosity: terse results vs. explanations and reasoning
- Personality: do they want a tool or a collaborator?

**Actions** (immediately after they answer):
- Update the **Tone** line in `SOUL.md` to reflect their preference
- Add a **Communication style** entry to `USER.md` capturing what they said
- If they gave you a name or asked you to change something about your personality, update `SOUL.md` accordingly

### Question 3: First Handoff

Ask what repetitive task they'd love to stop doing themselves. Frame it as: "What's something you find yourself doing over and over that you'd happily hand off to me?"

Listen for anything concrete. This question is about finding one real thing you can start doing for them *today*.

**Actions** (based on what they say):
- **Maps to a heartbeat** (e.g., "check my PRs", "monitor my server"): Add a custom pulse to `HEARTBEAT.yml` tailored to their request. Explain what you set up.
- **Maps to a project** (e.g., "I'm managing a homelab", "I'm job hunting"): Create one with `project_create`. Populate it with initial notes based on what they told you.
- **Maps to an MCP integration** (e.g., "check my email", "watch my calendar"): Explain that you'll need an MCP server for that, note the need in `USER.md`, and walk them through setup if they want to do it now.
- **Maps to something you can just do** (e.g., "organize my notes", "review this repo"): Just do it. Right now. Show them the result.
- **They're not sure**: Suggest something based on what you've learned so far. You know their proactivity level and communication style — use that to make a recommendation.

After completing their request (or setting it up), update `MEMORY.md` with a summary of what was configured during setup.

---

## Phase 2: Deeper Exploration

After Quick Setup is complete, the user has a working agent with real configuration. Now you can offer to go deeper — but let it flow from the conversation. Based on what you learned during setup, suggest the most relevant workflow rather than listing all options.

For example:
- If they asked for monitoring in Q3 → suggest the monitoring workflow
- If they mentioned multiple ongoing projects → suggest the organization workflow
- If they seem technically curious → suggest the understanding workflow
- If they want the full experience → suggest the always-on workflow

If nothing obvious fits, mention that there are deeper workflows available and ask if any interest them:

### "I want to get organized"
Read `workflows/getting-organized.md`. Covers projects, inbox, and memory.

### "I want you to watch things for me"
Read `workflows/monitoring-setup.md`. Covers heartbeats and notification routing, building on whatever was set up during Quick Setup.

### "I want to extend what you can do"
Read `workflows/extending-capabilities.md`. Covers skills, MCP servers, background tasks, and subagent presets.

### "I want to understand how you work"
Read `workflows/understanding-the-agent.md`. Walks through memory, context assembly, proactivity, and capabilities in user-facing terms.

### "I want the full Jarvis experience"
Read `workflows/always-on-assistant.md`. The power-user path — builds on Quick Setup with MCP integrations, advanced heartbeats, scheduled actions, and project organization.

### User just wants to hang out
That's fine too. Have a natural conversation, keep learning about them, keep updating `USER.md`. Mention that deeper workflows exist when it feels relevant.

---

## Wrapping Up

When the conversation winds down:

1. Make sure `BOOTSTRAP.md` is deleted.
2. Update `MEMORY.md` with a summary of what was set up and what you learned.
3. Deactivate this skill with `skill_deactivate residuum-getting-started`.
4. Tell the user what's now running in the background (if anything) and what they can do next.
