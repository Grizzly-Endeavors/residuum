# Routing Policy

Route background task results based on content and urgency.

## Rules
- Security alerts, errors, and failures → notify channels (ntfy, etc.) + inbox
- Routine findings and informational results → inbox only
- Webhook-triggered results → inbox (unless content indicates urgency)