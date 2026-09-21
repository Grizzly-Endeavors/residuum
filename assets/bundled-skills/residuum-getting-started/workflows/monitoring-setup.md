# Workflow: Monitoring Setup

Walk the user through heartbeats and notification routing. Build on whatever was already configured during Quick Setup — the built-in `reflection`, `memory_tending`, and `wiki_lint` pulses run by default, and the user may also have starter pulses (inbox_check, morning_briefing, nightly_review) enabled if they opted into more proactivity. Don't re-explain what's already running; acknowledge it and expand from there.

**Remember**: Write core facts to `USER.md` and everything else to wiki pages (activate the `wiki` skill) as you learn things throughout this workflow — don't save it all for the end.

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

Explain: "When a heartbeat check finds something worth reporting, the result is filed to your inbox. If the check decides it cannot wait, it also pushes to whatever notification channels you have set up."

There is no per-pulse routing field. Do not add a `channels:` key to a pulse — it is not a real field and will be silently ignored.

How a result is handled depends on what the sub-agent reports:
- Nothing noteworthy -> it replies `HEARTBEAT_OK` and the result is discarded entirely. This is what keeps routine checks from becoming noise.
- Something worth knowing -> filed to the inbox for the user to review.
- Something that cannot wait -> the sub-agent ends its report with `HEARTBEAT_URGENT`, which also pushes to every configured notification channel.

The sub-agent makes that urgency call itself, so **the way you word the pulse prompt is how you steer it**. Be concrete about what counts as urgent for this particular check. Compare:

```yaml
    tasks:
      - name: check_server
        prompt: "Run 'curl -s -o /dev/null -w \"%{http_code}\" https://example.com'. If the status code is not 200, that is urgent — the site is down. If it is 200, report HEARTBEAT_OK."
```

That tells the sub-agent exactly which outcome deserves to interrupt. A vague prompt gets vague judgment.

If the user wants a pulse to talk to them directly in conversation rather than filing to the inbox, set `agent: main` on the pulse. That runs it as a wake turn and injects the prompt straight into the conversation:

```yaml
pulses:
  - name: server_health
    schedule: "30m"
    active_hours: "08:00-22:00"
    agent: main
    tasks:
      - name: check_server
        prompt: "Check whether https://example.com returns 200."
```

Use `agent: main` sparingly -- it interrupts every time it fires, whether or not there is anything to say. Most monitoring should stay a background sub-agent.

## Step 4: External Notifications (Optional)

Ask if the user wants urgent results delivered outside the agent -- for example, push notifications to their phone.

If yes, explain that external channels are defined in `config/channels.toml` in the workspace. Walk them through what to add:
```toml
[channels.phone]
type = "ntfy"
url = "https://ntfy.sh"
topic = "my-residuum"
```

Once the channel exists it receives every urgent result automatically -- there is nothing further to wire up. Non-urgent results stay in the inbox.

If the user is not ready for external notifications, skip this step. Urgent results still reach the inbox, and they can ask you to set up push later.

## Step 5: Verify and Wrap Up

Tell the user that `HEARTBEAT.yml` is re-read on every scheduler tick -- changes take effect without restarting the gateway.

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
