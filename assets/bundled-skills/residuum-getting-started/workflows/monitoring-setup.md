# Workflow: Monitoring Setup

Walk the user through heartbeats and notification routing. Build on whatever was already configured during Quick Setup — the built-in `reflection` and `memory_tending` pulses run by default, and the user may also have starter pulses (inbox_check, morning_briefing, nightly_review) enabled if they opted into more proactivity. Don't re-explain what's already running; acknowledge it and expand from there.

**Remember**: Write to `USER.md` and `MEMORY.md` as you learn things throughout this workflow — don't save it all for the end.

## Step 1: Review What's Already Running and Ask What Else to Monitor

Start by checking `HEARTBEAT.yml` to see what's already enabled. Briefly acknowledge it: "You've already got [X] running — [built-in self-maintenance, plus whatever starter pulses from our initial setup]. Let's talk about what else you want me to keep an eye on."

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

## Step 3: Explain How Results Reach the User

Explain: "When a heartbeat check finds something worth reporting, the result is routed automatically. A small model reads the result and decides where it goes, following the policy in `ALERTS.md`."

There is no per-pulse routing field. Do not add a `channels:` key to a pulse — it is not a real field and will be silently ignored.

The destinations a result can reach:
- `inbox` -- stores the result silently for the user to review later. This is the default and the fallback.
- Any notification channel defined in `config/channels.toml` (for example an ntfy push to the user's phone).

If the user wants a pulse to talk to them directly rather than filing to the inbox, set `agent: main` on the pulse. That runs the pulse as a wake turn instead of a background sub-agent, and its prompt is injected straight into the conversation:

```yaml
pulses:
  - name: server_health
    schedule: "30m"
    active_hours: "08:00-22:00"
    agent: main
    tasks:
      - name: check_server
        prompt: "Run 'curl -s -o /dev/null -w \"%{http_code}\" https://example.com' and report if the status code is not 200."
```

Use `agent: main` sparingly -- it interrupts. Most monitoring should stay a background sub-agent and land in the inbox.

To change routing behavior, edit `ALERTS.md` in the workspace root. It is plain prose read on every routing decision, and edits take effect immediately:

```markdown
## Rules
- Security alerts, errors, and failures -> notify channels (ntfy, etc.) + inbox
- Routine findings and informational results -> inbox only
```

## Step 4: External Notifications (Optional)

Ask if the user wants results delivered outside the agent -- for example, push notifications to their phone.

If yes, explain that external channels are defined in `config/channels.toml` in the workspace. Walk them through what to add:
```toml
[channels.phone]
type = "ntfy"
url = "https://ntfy.sh"
topic = "my-residuum"
```

Once the channel exists, it becomes an available target for the router automatically. If the user wants it used for a specific class of result, add a line saying so to `ALERTS.md`.

If the user is not ready for external notifications, skip this step. They can ask you to set it up later.

## Step 5: Verify and Wrap Up

Tell the user that `HEARTBEAT.yml` and `ALERTS.md` are both re-read on every use -- changes take effect without restarting the gateway.

Summarize what was configured:
- Which pulse is running and how often
- Where results are delivered
- How to check results (the inbox, or directly in conversation if the pulse uses `agent: main`)

Suggest next steps:
- "If you think of more things to monitor, just tell me and I will set up new pulses."
- "I will evolve the monitoring over time based on what you pay attention to and what you ignore."
- "If you want to connect to external services like email or calendars, ask about MCP server setup."
- "For scheduled one-off tasks instead of recurring checks, I can use `schedule_action`."

For full heartbeat and notification reference, activate `residuum-system`.
