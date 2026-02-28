# Workflow: Always-On Assistant (Jarvis Mode)

This is the power-user path. Walk the user through a full setup that turns the agent into an always-on personal assistant. Each step builds on the previous one. Take it at the user's pace -- this can span multiple conversations.

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

Then activate it with `project_activate` and configure MCP servers in its `PROJECT.md`:
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

Now that external services are connected, set up recurring checks. For each MCP server the user configured, suggest a corresponding heartbeat pulse.

Write `HEARTBEAT.yml` with pulses tailored to their integrations:
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

Configure `NOTIFY.yml` so results reach the user appropriately. Tier the notifications by urgency:

```yaml
agent_wake:
  - email_check

agent_feed:
  - github_activity
  - calendar_review

inbox:
  - email_check
  - github_activity
  - calendar_review
```

Explain the routing strategy:
- Email goes to `agent_wake` because new emails might need immediate response. Also goes to `inbox` as a backup record.
- GitHub and calendar go to `agent_feed` -- the agent sees them at the next interaction but does not interrupt.
- Everything also goes to `inbox` so nothing is lost.

If the user wants phone notifications, help set up an ntfy channel in `config.toml`:
```toml
[notifications.channels.ntfy]
type = "ntfy"
url = "https://ntfy.sh"
topic = "my-assistant"
```

Then add it to NOTIFY.yml for the most important pulses:
```yaml
ntfy:
  - email_check
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

Show them how to manage actions:
- `list_actions` -- see all pending scheduled actions
- `cancel_action` -- cancel one by ID

Mention that actions fire once and are removed. For recurring tasks, heartbeats are the right tool.

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

Explain that when they mention "homelab" in conversation, the agent will recognize it and activate the project context, loading the overview, file manifest, and scoped tools automatically.

## Wrap Up

Summarize the complete setup:
- MCP servers connecting to their external services
- Heartbeat pulses monitoring those services on a schedule
- Notification routing delivering results through appropriate channels
- Scheduled actions for time-based tasks
- Projects organizing ongoing work areas

This is a living system. Explain that:
- "I will evolve HEARTBEAT.yml and NOTIFY.yml based on what works. If you consistently ignore a notification, I will suggest moving it to inbox or removing it."
- "You can add new pulses, adjust schedules, and change routing at any time. Both files are hot-reloaded."
- "As you use the system, I will learn your patterns and suggest improvements."

For the complete technical reference on all configuration files, mention: "For detailed format specifications, see `skill_activate ironclaw-system`."
