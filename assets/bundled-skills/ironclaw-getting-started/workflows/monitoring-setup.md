# Workflow: Monitoring Setup

Walk the user through heartbeats and notification routing. By the end, they should have at least one heartbeat pulse running and understand how results reach them.

## Step 1: Explain What Heartbeats Do

Explain: "Heartbeats are periodic checks I run in the background. I can monitor things like whether a service is up, check for new emails, review pull requests, or anything else you want me to keep an eye on. Each check runs on a schedule you define."

Ask the user what they would like monitored. Listen for:
- Server or service health checks
- Git repository activity (PRs, issues)
- File or directory changes
- Email or message checking (requires MCP server)
- Anything they currently check manually on a regular basis

Pick one concrete example to start with.

## Step 2: Configure HEARTBEAT.yml

Explain that heartbeats are defined in `HEARTBEAT.yml`. Write a real example using `edit_file` or `write_file` on the HEARTBEAT.yml file. Use the user's chosen monitoring target.

Example for a service health check:
```yaml
pulses:
  - name: server_health
    schedule: "30m"
    active_hours: "08:00-22:00"
    tasks:
      - name: check_server
        prompt: "Run 'curl -s -o /dev/null -w \"%{http_code}\" https://example.com' and report if the status code is not 200."
```

Example for git repository monitoring:
```yaml
pulses:
  - name: pr_review
    schedule: "2h"
    active_hours: "09:00-18:00"
    tasks:
      - name: check_prs
        prompt: "Check for open pull requests on the main repository using gh pr list. Report any that need attention."
```

Explain the key fields:
- `name` -- identifies this pulse (used in NOTIFY.yml for routing)
- `schedule` -- how often to run (e.g., `"30m"`, `"2h"`, `"1d"`)
- `active_hours` -- optional window to restrict when the pulse fires (respects configured timezone)
- `tasks` -- one or more prompts that are executed when the pulse fires
- `enabled` -- set to `false` to pause a pulse without deleting it

## Step 3: Set Up Notification Routing

Explain: "When a heartbeat check finds something worth reporting, the result needs to go somewhere. NOTIFY.yml controls where results are delivered."

Edit `NOTIFY.yml` to route the pulse the user just created. Use `edit_file` or `write_file`.

Example configuration:
```yaml
agent_feed:
  - server_health

inbox:
  - server_health
```

Explain the built-in channels:
- `agent_wake` -- injects the result and starts a conversation turn immediately. Use for urgent things.
- `agent_feed` -- injects the result passively, shown at the next natural interaction. Use for things the agent should know about.
- `inbox` -- stores the result silently. The user sees an unread count. Use for things to review later.

Help the user decide which channel fits their monitoring target. Urgent issues (server down) should go to `agent_wake`. Informational checks (new PRs) fit `agent_feed` or `inbox`.

## Step 4: External Notifications (Optional)

Ask if the user wants results delivered outside the agent -- for example, push notifications to their phone.

If yes, explain that external channels like `ntfy` are configured in `config.toml`:
```toml
[notifications.channels.ntfy]
type = "ntfy"
url = "https://ntfy.sh"
topic = "my-ironclaw"
```

After adding the channel to config.toml, reference it by name in NOTIFY.yml:
```yaml
ntfy:
  - server_health
```

If the user is not ready for external notifications, skip this step. They can add it later.

## Step 5: Verify and Wrap Up

Tell the user that HEARTBEAT.yml and NOTIFY.yml are hot-reloaded -- changes take effect without restarting the gateway.

Summarize what was configured:
- Which pulse is running and how often
- Where results are delivered
- How to check results (inbox, or waiting for agent_feed/agent_wake)

Suggest next steps:
- "You can add more pulses to HEARTBEAT.yml as you think of things to monitor."
- "I will evolve NOTIFY.yml over time based on what you pay attention to and what you ignore."
- "If you want to connect to external services like email or calendars, ask about MCP server setup."
- "For scheduled one-off tasks instead of recurring checks, I can use `schedule_action`."

For the full heartbeat and notification format details, mention: "For complete reference documentation, see `skill_activate ironclaw-system`."
