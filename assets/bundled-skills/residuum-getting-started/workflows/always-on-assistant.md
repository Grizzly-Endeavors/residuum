# Workflow: Always-On Assistant (Jarvis Mode)

This is the power-user path. Walk the user through a full setup that turns you into an always-on personal assistant. Each step builds on the previous one. Take it at the user's pace — this can span multiple conversations.

**Remember**: Write to `USER.md` and `MEMORY.md` as you learn things throughout this workflow — don't save it all for the end.

**Build on Quick Setup**: The built-in `reflection` and `memory_tending` pulses are already running, the user may have starter pulses configured on top depending on the proactivity level they chose, and you know their communication preferences. Don't re-cover ground — acknowledge what's running and expand from there.

## Step 1: MCP Server Setup for Integrations

Ask the user what external services they interact with daily. Common categories:
- **Email** -- checking for new messages, drafting replies
- **Calendar** -- viewing schedule, creating events
- **Smart home** -- controlling lights, thermostats, security
- **Code repositories** -- GitHub, GitLab activity
- **Communication** -- Slack, Discord, messaging platforms
- **Cloud infrastructure** -- AWS, GCP, monitoring dashboards
- **Files and documents** -- local or cloud file systems

For each service the user wants, help them find or configure an MCP server. Set these up in a project so they are organized:

Create a project for the integration work:
```
project_create with name: "Personal Integrations" and description: "MCP servers and automation for daily services"
```

Then activate it with `project_activate` and configure MCP servers in the project's `PROJECT.md`:
```yaml
mcp_servers:
  - name: filesystem
    command: "mcp-server-filesystem"
    args: ["/home/user/documents"]
  - name: github
    command: "mcp-server-github"
    env:
      GITHUB_TOKEN: "${GITHUB_TOKEN}"
```

Help the user install any MCP server binaries they need. Common ones are available via npm (`npx @modelcontextprotocol/server-*`) or as standalone binaries.

## Step 2: Heartbeat Configuration

Now that external services are connected, set up recurring checks for them. Besides the built-in `reflection` and `memory_tending` pulses, the user may already have starter pulses running from Quick Setup (inbox_check, morning_briefing, nightly_review). Build on those — don't overwrite them.

For each MCP server the user configured, suggest a corresponding heartbeat pulse. Add new pulses to `HEARTBEAT.yml` alongside any existing ones:
```yaml
pulses:
  - name: email_check
    schedule: "30m"
    active_hours: "08:00-22:00"
    tasks:
      - name: check_inbox
        prompt: "Check for unread emails. Summarize any that need attention. Report HEARTBEAT_OK if nothing new."

  - name: calendar_review
    schedule: "4h"
    active_hours: "07:00-20:00"
    tasks:
      - name: upcoming_events
        prompt: "Review my calendar for the next 4 hours. Highlight any upcoming meetings or deadlines."

  - name: github_activity
    schedule: "2h"
    active_hours: "09:00-18:00"
    tasks:
      - name: check_prs
        prompt: "Check for open PRs that need my review or PRs I authored that have new comments. Report HEARTBEAT_OK if nothing needs attention."
```

Explain the `HEARTBEAT_OK` convention: when a sub-agent returns this exact string, it means "nothing to report" and the result is silently discarded (not routed). This prevents notification spam for routine checks that find nothing.

## Step 3: Notification Routing

Results are filed to the inbox automatically. A check that finds nothing replies `HEARTBEAT_OK` and is discarded; a check that finds something that cannot wait ends its report with `HEARTBEAT_URGENT` and is also pushed to every configured notification channel.

There is no per-pulse routing field -- do not add a `channels:` key to a pulse, it is not real and will be silently ignored.

The sub-agent decides urgency, so steer it in the prompt. Revise the pulses from Step 2 to say what counts as urgent for each:

```yaml
pulses:
  - name: email_check
    schedule: "30m"
    active_hours: "08:00-22:00"
    tasks:
      - name: check_inbox
        prompt: "Check for unread emails. Summarize any that need attention. Anything genuinely time-sensitive -- a same-day deadline, something from my manager, an account security notice -- is urgent. Report HEARTBEAT_OK if nothing new."

  - name: github_activity
    schedule: "2h"
    active_hours: "09:00-18:00"
    tasks:
      - name: check_prs
        prompt: "Check for open PRs that need my review or PRs I authored that have new comments. This is informational -- it is never urgent. Report HEARTBEAT_OK if nothing needs attention."
```

Note the second one explicitly rules urgency out. Telling a check what is *not* urgent is as useful as telling it what is.

If the user wants phone notifications for the urgent cases, help set up an ntfy channel in `config/channels.toml`:
```toml
[channels.phone]
type = "ntfy"
url = "https://ntfy.sh"
topic = "my-residuum"
```

## Rules
- Anything from email_check that looks time-sensitive -> ntfy + inbox
- Calendar and GitHub findings -> inbox only
- Errors and failures from any pulse -> ntfy + inbox
```

If the user wants phone notifications, help set up an ntfy channel in `config/channels.toml`:
```toml
[channels.phone]
type = "ntfy"
url = "https://ntfy.sh"
topic = "my-residuum"
```

## Step 4: Scheduled Actions for Time-Based Tasks

Explain: "Beyond recurring heartbeats, I can schedule one-off actions for specific times. Think of these as reminders with intelligence -- instead of just buzzing at you, I can actually do something when the time comes."

Demonstrate with a real example based on the user's context:
```
schedule_action with:
  name: "morning_briefing"
  prompt: "Compile a morning briefing: check email, review today's calendar, list open PRs, and summarize anything from overnight heartbeats that needs attention."
  run_at: "2026-02-28T08:00:00"
  agent_name: "main"
```

Using `agent_name: "main"` makes the action run as a full agent turn with conversation context, which is appropriate for briefings.

Tell the user they can ask you to list or cancel scheduled actions at any time. Mention that actions fire once and are removed. For recurring tasks, heartbeats are the right tool.

## Step 5: Projects for Ongoing Automation

Help the user create projects for their major ongoing areas. Each project scopes the agent's knowledge and tools to what is relevant.

Suggest project structure based on what was set up:
- A project for each major area of their life (work, personal, homelab, etc.)
- The "Personal Integrations" project already created holds cross-cutting automation config
- Topic-specific projects hold domain knowledge (notes, references, decisions)

Example for someone with a homelab:
```
project_create with:
  name: "Homelab"
  description: "Home server infrastructure, Docker services, networking"
  tools: ["exec", "read", "write"]
```

Explain that when they mention "homelab" in conversation, you will recognize it and activate the project context, loading the relevant knowledge and tools automatically.

## Wrap Up

Summarize the complete setup:
- MCP servers connecting to their external services
- Heartbeat pulses monitoring those services on a schedule
- Notification routing delivering results through appropriate channels
- Scheduled actions for time-based tasks
- Projects organizing ongoing work areas

This is a living system. Explain that:
- "I will evolve monitoring based on what works. If you consistently ignore a notification, I will suggest moving it to inbox or removing it."
- "If you want to add new checks, adjust schedules, or change where notifications go, just tell me and I will update things."
- "As you use the system, I will learn your patterns and suggest improvements."

For the complete technical reference, activate `residuum-system`.
