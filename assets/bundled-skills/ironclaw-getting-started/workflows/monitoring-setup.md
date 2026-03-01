# Workflow: Monitoring Setup

Walk the user through heartbeats and notification routing. Build on whatever was already configured during Quick Setup — the user may already have starter pulses (inbox_check, morning_briefing, nightly_review) enabled. Don't re-explain what's already running; acknowledge it and expand from there.

**Remember**: Write to `USER.md` and `MEMORY.md` as you learn things throughout this workflow — don't save it all for the end.

## Step 1: Review What's Already Running and Ask What Else to Monitor

Start by checking `HEARTBEAT.yml` to see what's already enabled. Briefly acknowledge it: "You've already got [X] running from our initial setup. Let's talk about what else you want me to keep an eye on."

Ask the user what they would like monitored. Listen for:
- Server or service health checks
- Git repository activity (PRs, issues)
- File or directory changes
- Email or message checking (requires MCP server)
- Anything they currently check manually on a regular basis

Pick one concrete example to start with.

## Step 2: Set Up a Heartbeat

Configure a heartbeat pulse based on the user's chosen monitoring target. Write it to `HEARTBEAT.yml` using `edit_file` or `write_file`. Do not tell the user to edit the file — you do it for them.

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

Explain what you configured in plain terms — what it checks, how often, and during what hours. The user does not need to know the file format or field names.

## Step 3: Set Up Notification Routing

Explain: "When a heartbeat check finds something worth reporting, the result needs to go somewhere. I route results to different channels depending on urgency."

Add the `channels` field to the pulse you just created. Use `edit_file` or `write_file`.

Example — add `channels` to the pulse:
```yaml
pulses:
  - name: server_health
    schedule: "30m"
    active_hours: "08:00-22:00"
    channels: [agent_feed, inbox]
    tasks:
      - name: check_server
        prompt: "Run 'curl -s -o /dev/null -w \"%{http_code}\" https://example.com' and report if the status code is not 200."
```

Explain the built-in channels:
- `agent_wake` -- injects the result and starts a conversation turn immediately. Use for urgent things.
- `agent_feed` -- injects the result passively, shown at the next natural interaction. Use for things you should know about.
- `inbox` -- stores the result silently. The user sees an unread count. Use for things to review later.

Help the user decide which channel fits their monitoring target. Urgent issues (server down) should go to `agent_wake`. Informational checks (new PRs) fit `agent_feed` or `inbox`.

## Step 4: External Notifications (Optional)

Ask if the user wants results delivered outside the agent -- for example, push notifications to their phone.

If yes, explain that external channels like `ntfy` need to be added to `config.toml` (the user's config file). Walk them through what to add:
```toml
[notifications.channels.ntfy]
type = "ntfy"
url = "https://ntfy.sh"
topic = "my-ironclaw"
```

After the user adds the channel to config.toml, update the pulse's routing to include it.

If the user is not ready for external notifications, skip this step. They can ask you to set it up later.

## Step 5: Verify and Wrap Up

Tell the user that heartbeat and channel configuration is hot-reloaded -- changes take effect without restarting the gateway.

Summarize what was configured:
- Which pulse is running and how often
- Where results are delivered
- How to check results (inbox, or waiting for agent_feed/agent_wake)

Suggest next steps:
- "If you think of more things to monitor, just tell me and I will set up new pulses."
- "I will evolve the monitoring over time based on what you pay attention to and what you ignore."
- "If you want to connect to external services like email or calendars, ask about MCP server setup."
- "For scheduled one-off tasks instead of recurring checks, I can use `schedule_action`."

For full heartbeat and notification reference, activate `ironclaw-system`.
