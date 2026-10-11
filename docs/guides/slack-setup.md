# Set up Residuum in Slack

This walks through putting your agent in Slack as an app you can DM. It connects out to Slack over a websocket (Socket Mode), so unlike Teams it needs no public address, no tunnel, and no inbound port — it works from a machine behind a firewall.

One Slack app serves one agent. DMs are the whole surface in this version: the app doesn't join channels or group chats.

You will need a Slack workspace where you can create apps — most workspaces, including free ones, allow this under [api.slack.com/apps](https://api.slack.com/apps).

## 1. What the manifest is

`assets/slack-app-manifest.json` in the Residuum checkout is a filled-in Slack app definition: Socket Mode on, the `message.im` event subscribed (the app sees DMs), and the two bot scopes the interface needs — `chat:write` to post replies and `im:history` to read DM history. Nothing more; the scope list is deliberately minimal. The verification notes beside it (`assets/slack-app-manifest.README.md`) record what was checked against Slack's current manifest documentation and why `connections:write` is not in it (that one belongs to the app-level token you'll create in step 3).

## 2. Create the app from the manifest

1. Open [api.slack.com/apps](https://api.slack.com/apps) and choose **Create New App → From manifest**.
2. Pick your workspace, choose the **JSON** tab, and paste the contents of `assets/slack-app-manifest.json`.
3. Before confirming, rename the app to your agent: change **Residuum Agent** (both the app name and, on the review screen, the bot display name) to the agent's name, and make the description one line about what this agent does. This is the name people see when they DM it.
4. Confirm with **Create**.

## 3. Install it and collect the two tokens

1. From the app's page, choose **Install to Workspace** and approve. (If your workspace requires admin approval, that request goes out here.)
2. **Bot token** — under **Features → OAuth & Permissions**, copy the **Bot User OAuth Token**. It starts with `xoxb-`. This is the agent's identity for posting replies.
3. **App token** — under **Basic Information → App-Level Tokens** (scroll to that section), choose **Generate Token and Scopes**, give it a name (anything, e.g. `socket`), add the **`connections:write`** scope, and copy the token. It starts with `xapp-`. This is what opens the Socket Mode connection. If the generator asks nothing about scopes, add `connections:write` before generating — the connection refuses to open without it.

## 4. Configure Residuum

Slack is configured in `config.toml`, in the agent's config directory. Store the two tokens in the secret store first, so they never sit in the file:

```sh
residuum secret set slack_bot_token   # paste the xoxb- token
residuum secret set slack_app_token   # paste the xapp- token
```

Then:

```toml
[slack]
bot_token = "secret:slack_bot_token"   # xoxb-… — posts messages as the bot user
app_token = "secret:slack_app_token"   # xapp-… — opens the Socket Mode connection
respond_to_others = false   # true: anyone who can DM the app can use the agent
```

Each token also accepts a `"${ENV_VAR}"` reference or a literal, if you'd rather pull them from the environment:

```toml
[slack]
bot_token = "${SLACK_BOT_TOKEN}"
app_token = "${SLACK_APP_TOKEN}"
```

Saving the file is all it takes: Residuum watches `config.toml` and reloads it in place. The log shows the slack adapter restarting (`adapter restarted with new config`) and then `slack socket connected` — no agent restart needed. The same is true later: editing the `[slack]` section (say, rotating a token) drains and closes the old socket and boots a new one without an agent restart, and removing the section stops the adapter. If you start the agent fresh instead, the boot-time line is `slack interface started`.

## 5. Say hello — and become the owner

Open Slack, find the app (search its name, or under the workspace's apps), and send it a direct message. **The first person to DM the app becomes its owner** — that's how it tells you apart from anyone else, and where it sends scheduled results and other proactive messages. You'll see `slack owner set from first direct message` in the log, and it's remembered in `slack_state.json` in the agent's workspace.

The round trip is your verification: you DM the app, the agent answers in the same DM. If the answer comes back, every piece — socket, event subscription, tokens, reply posting — works.

By default the agent answers only its owner; anyone else who DMs it is told so. To open it to others, set `respond_to_others = true` in the `[slack]` section — the agent still knows who is asking and keeps your private information out of its replies to them.

## 6. First deployment: probe the wire format

The Slack interface ships with envelope fixtures (`src/interfaces/slack/fixtures/*.json`) written from Slack's published docs and marked **assumed-until-probed** — see `fixtures/README.md` in that directory. At the first live deployment, capture real envelopes and events (from the socket traffic or Slack's app logs) and **replace those fixture files byte-for-byte**, then re-run the test suite (`cargo test`). Until that probe runs, treat any wire-format oddity as possibly ours, not Slack's.

## Troubleshooting

Messages you send never reach the agent:

- No `slack socket connected` in the log: the app token can't open the connection. Check it starts with `xapp-`, was generated with the `connections:write` scope (step 3), and matches the installed app.
- `slack interface started` never appears: the `[slack]` section didn't load — the log names which token is missing or unreadable (check your `secret:` references with `residuum secret list`).

The agent gets messages but replies never show up in Slack:

- The bot token is wrong or lacks `chat:write`: check it starts with `xoxb-` and comes from **Features → OAuth & Permissions** of the same app. The log records post failures.

The app says it only takes requests from someone else: someone else DMed it first. The owner is the `owner` entry in `slack_state.json` in the agent's workspace — remove it (or the file) and reload; the next DM re-claims ownership.
