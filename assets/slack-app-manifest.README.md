# Slack app manifest — verification notes

`slack-app-manifest.json` is the starting point for the per-agent Slack app
described in `docs/guides/slack-setup.md`. JSON has no comments, so the
verification record lives here.

## What was verified, and against what

Field shapes and scopes were checked against Slack's current documentation on
**2026-10-10** (Task 8 of the Slack interface work, issue #490):

- Manifest structure (`_metadata` v2.1, `display_information`, `features`,
  `oauth_config.scopes.bot`, `settings.*` including `socket_mode_enabled`):
  <https://docs.slack.dev/reference/app-manifest/>
- Socket Mode setup, including a manifest that enables it and where the
  app-level token is generated:
  <https://docs.slack.dev/apis/events-api/using-socket-mode/>
- `chat:write` (post as the bot user) and `im:history` (read DM history) are
  bot-token scopes that belong in `oauth_config.scopes.bot`; both appear in
  the current manifest reference example:
  <https://docs.slack.dev/reference/app-manifest/>
- `message.im` is a valid `bot_events` subscription (DM message events) and
  appears in the current manifest reference example:
  <https://docs.slack.dev/reference/app-manifest/>

## The deliberate omission: `connections:write` is NOT in `oauth_config.scopes.bot`

Slack's scope reference states `connections:write` is supported by
**app-level tokens only** (<https://docs.slack.dev/reference/scopes/connections.write/>):
it grants `apps.connections.open`, the call that opens the Socket Mode
websocket. App-level tokens (and their scopes) are generated in the app's
admin UI under **Basic Information → App-Level Tokens**, not requested through
the manifest's bot scopes — the current manifest reference has no field for
app-level token scopes. So the manifest carries only `chat:write` and
`im:history`; the guide walks through granting `connections:write` when the
`xapp-` token is created.

## Placeholder values the operator renames per agent

- `display_information.name` and `features.bot_user.display_name`
  (`"Residuum Agent"`) — one app per agent; name it after the agent.
- `display_information.description` — one line about what this agent does.

## What stays assumed until first live deployment

That Slack's **Create from manifest** flow accepts this file verbatim is
assumed-until-probed, per the same discipline as
`src/interfaces/slack/fixtures/README.md`: the first live deployment (or the
Task 8 reviewer) confirms it, and any drift is fixed here and in
`docs/guides/slack-setup.md`. `features.bot_user.always_online = true` is a
choice, not a Slack requirement: the adapter holds a resident Socket Mode
connection, so the bot is shown online.
