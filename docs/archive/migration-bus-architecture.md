# Migration Guide: Bus Architecture & Endpoint Routing

This release replaces the channel-based routing system with a pub/sub bus architecture. Background task results are now routed by an LLM notification router using a customizable policy (`ALERTS.md`), and webhooks are individually named endpoints.

**This is a breaking change** — several config files and API endpoints must be updated.

## 1. config.toml — Named Webhooks

**Old**: Single `[webhook]` section.

**New**: One `[webhooks.<name>]` section per webhook endpoint.

```toml
# Before
[webhook]
secret = "my-token"

# After
[webhooks.github-issues]
secret = "secret:github_webhook"
routing = "agent:code_reviewer"
content_fields = ["issue.title", "issue.body", "issue.labels"]

[webhooks.deploy-hook]
secret = "${DEPLOY_SECRET}"
routing = "inbox"

[webhooks.raw-events]
routing = "inbox"
format = "raw"
```

| Field | Default | Description |
|-------|---------|-------------|
| `secret` | *(none)* | Bearer token for auth. Supports `${ENV_VAR}` and `secret:name` syntax. |
| `routing` | `"inbox"` | Where payloads go: `"inbox"` or `"agent:<preset_name>"`. |
| `format` | `"parsed"` | `"parsed"` extracts JSON fields; `"raw"` passes the full body. |
| `content_fields` | *(none)* | JSON dot-paths to extract (parsed format only), e.g. `["issue.title", "labels[0]"]`. |

## 2. scheduled_actions.json — Remove `channels`

Serde will reject the old format. Remove the `channels` field from every entry.

```jsonc
// Before
{ "name": "Daily report", "prompt": "...", "run_at": "...", "channels": ["telegram"] }

// After
{ "name": "Daily report", "prompt": "...", "run_at": "...", "agent_name": "main" }
```

- `channels` is **removed**. Results route via ALERTS.md policy.
- `agent_name` (optional): `"main"` for a full wake turn, a preset name for a sub-agent, or omit for default.
- `model_tier` (optional): `"small"` / `"medium"` / `"large"`.

## 3. Subagent Presets — Remove `channels:` from Frontmatter

Remove the `channels:` key from YAML frontmatter in `~/.residuum/workspace/subagents/*.md`.

```yaml
# Before
---
name: code_reviewer
description: "Code review specialist"
channels:
  - telegram
---

# After
---
name: code_reviewer
description: "Code review specialist"
model_tier: medium
denied_tools:
  - exec
  - write_file
---
```

New optional fields: `model_tier`, `denied_tools`, `allowed_tools` (mutually exclusive).

## 4. Pulse Configs (HEARTBEAT.yml) — Remove `channels:`

Remove `channels:` from pulse definitions. Results now route through ALERTS.md.

```yaml
# Before
pulses:
  - name: inbox_check
    enabled: true
    schedule: "3h"
    channels: [inbox, telegram]
    tasks:
      - name: check_inbox
        prompt: "Check inbox..."

# After
pulses:
  - name: inbox_check
    enabled: true
    schedule: "3h"
    tasks:
      - name: check_inbox
        prompt: "Check inbox..."
```

## 5. WebSocket Clients — `MessageOrigin.interface` → `.endpoint`

If your WebSocket client reads `MessageOrigin` from the server, the `interface` field has been renamed to `endpoint`.

```jsonc
// Before
{ "origin": { "interface": "websocket" } }

// After — the field is now "endpoint"
{ "origin": { "endpoint": "websocket" } }
```

## 6. Webhook URLs — Named Routes

**Old**: `POST /webhook`
**New**: `POST /webhook/<name>`

```
POST /webhook/github-issues    → 202 Accepted
POST /webhook/nonexistent      → 404 Not Found
```

Update any external services (GitHub webhooks, CI/CD hooks, etc.) to include the webhook name in the URL path.

## 7. ALERTS.md — Notification Routing Policy

A new file `~/.residuum/workspace/ALERTS.md` controls how background task results are routed. It is created automatically at bootstrap with this default:

```markdown
# Routing Policy

Route background task results based on content and urgency.

## Rules
- Security alerts, errors, and failures → notify channels (ntfy, etc.) + inbox
- Routine findings and informational results → inbox only
- Webhook-triggered results → inbox (unless content indicates urgency)
```

**How it works**: A small-tier LLM reads each background result and applies the rules in ALERTS.md to decide where it goes (inbox, notification channels, or the main agent). Edit the file to customize — changes take effect immediately.

Healthy heartbeat results (`HEARTBEAT_OK`) are silently discarded before reaching the router.

## Quick Checklist

- [ ] Rewrite `[webhook]` → `[webhooks.<name>]` sections in `config.toml`
- [ ] Remove `channels` from `scheduled_actions.json` entries
- [ ] Remove `channels:` from subagent preset frontmatter
- [ ] Remove `channels:` from HEARTBEAT.yml pulse definitions
- [ ] Update WebSocket clients: `MessageOrigin.interface` → `.endpoint`
- [ ] Update external webhook URLs: `/webhook` → `/webhook/<name>`
- [ ] Review/customize `ALERTS.md` after first boot (created automatically)
