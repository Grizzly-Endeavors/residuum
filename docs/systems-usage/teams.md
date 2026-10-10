# Microsoft Teams

The Teams interface lets the agent chat in Microsoft Teams: in a direct message with its owner, and — when added to them — in group chats and team channels. For step-by-step setup see the [Teams setup guide](../guides/teams-setup.md).

## How messages reach Residuum

Teams has no outbound connection mode like Discord's gateway or Telegram's long polling: Microsoft's Bot Connector delivers every message as an HTTPS `POST` of a Bot Framework activity to the bot's public messaging endpoint. Residuum runs a small dedicated listener for Teams on `[teams] port` (default `7701`, bound on the gateway's `bind` address) serving a single route, `POST /api/teams/messages`.

With Residuum Cloud connected, that public address is this instance's own host: `https://{slug}.{user}.agent-residuum.com/teams/{agent}`. Settings → Connections → Microsoft Teams shows it while the tunnel is up. Residuum terminates TLS for that host itself, accepts only a `POST` there and forwards it, bearer token included, to this agent's listener; the relay only carries encrypted bytes. Replies still go from the hub straight to Microsoft. While the tunnel is down nothing answers, and while the agent isn't listening for Teams (not configured, or stopped) Residuum answers `503` so Microsoft retries.

The listener stays separate from the gateway port, and a tunnel of your own pointed at the Teams port still works. The gateway serves the config and secrets API without authentication and is meant to stay on loopback; a tunnel pointed at the Teams port exposes only the Teams route. Microsoft requires HTTPS on 443 with a publicly trusted certificate, which Residuum Cloud's instance address already has. Tailscale Funnel, a Cloudflare named tunnel, or any reverse proxy with a trusted certificate are the other way to reach the port.

Every request is authenticated before anything in it is trusted:

- The bearer JWT must verify (RS256) against Microsoft's published Bot Framework signing keys (`login.botframework.com` OpenID metadata, cached for a day and refetched when an unknown key ID appears), from a key endorsed for the `msteams` channel.
- Issuer must be `https://api.botframework.com`, audience must be `[teams] app_id`, the token must be within its validity window (5 minutes of clock skew tolerated), and its `serviceurl` claim must match the activity's `serviceUrl`, so a captured token cannot redirect replies elsewhere.
- The activity must come from the `msteams` channel and from `[teams] tenant_id`.

Failures return `401` (bad token), `403` (other tenant), or `503` when Residuum cannot fetch Microsoft's signing keys — the connector retries a `503` later. Each rejection is logged with the reason. Authenticated activities are acknowledged immediately and handled in order by a single worker, since the connector retries anything not acknowledged within about 15 seconds.

## Who the agent answers

The **owner** is whoever first sends the bot a direct message. With a custom-uploaded app that can only be the person who installed it. The owner's Entra object ID and DM conversation are saved in `teams_state.json` in the workspace, so ownership survives restarts. To hand the bot to someone else, stop Residuum, remove the `owner` entry from that file, and have the new owner DM the bot.

`[teams] respond_to_others` controls everyone else:

- `false` (default): only the owner's messages reach the agent. Anyone else who messages or @mentions the bot gets a short reply saying it only takes requests from the owner. Before an owner exists, the reply asks the owner to DM the bot first.
- `true`: anyone in the tenant who can reach the bot can talk to the agent.

Slash commands (`/stop`, `/reload`, `/inbox`, …) run only for the owner in either mode.

Every message the agent sees records who sent it and where, e.g. `[From: Jane Doe via teams (#builds (Eng Team))]` — see [Message Senders](memory.md#message-senders). The agent's standing orientation tells it to keep the owner's private information out of replies to other people.

## Conversation routing

Only the owner's own direct message reaches the main agent. Every other admitted conversation — a group chat, a standard or private channel, and a non-owner's DM when `respond_to_others` is on — is handled by an [agent session](background-tasks.md) of its own instead: a temporary fork of the main agent, addressed deterministically by that conversation, that keeps its own memory and idle timeout rather than sharing the owner's private conversation. This holds even when the owner is the one talking in a group chat or channel — those are still shared spaces, so they get a session, not main. The session sees the same sender attribution and buffered chatter described below, and replies into that conversation — see [Where replies go](#where-replies-go).

`/stop` follows the same routing: typed in the owner's own DM it stops main's current turn; typed in a group chat or channel it stops that conversation's session instead, never main — see [Turn Control](turn-control.md). `/stop <name>` names any live session explicitly instead of relying on that routing — useful for stopping a session from somewhere other than its own conversation, e.g. the owner's own DM. `/sessions` lists every live session (address, purpose, state, elapsed time). `/multitask <task>`, in the owner's own DM only, forks the main conversation into a session that works on the task and posts its replies back into that DM — see [Owner Sessions](background-tasks.md#owner-sessions).

## Direct messages, group chats, and channels

In a direct message every message goes to the agent.

In group chats and standard channels the agent acts only on messages that @mention it. The bot's own mention is stripped from the text; mentions of other people are kept as `@Name`.

When the app manifest grants the resource-specific consent permissions `ChatMessage.Read.Chat` and `ChannelMessage.Read.Group`, Teams also delivers the messages that don't mention the bot. Those are held in memory (the same buffer Discord and Telegram use for their own unmentioned messages) — never written to disk, and lost on restart — up to `[teams] context_messages` per conversation (default 20; `0` disables), across at most 256 conversations at once; past that, the conversation that went longest without new chatter is dropped. When the bot is next @mentioned there, the held messages are handed to the agent as a single background message placed just before the mention:

```
Previous conversation in group chat "Launch prep" since you were last mentioned there. This is background only; the message that mentions you follows.
[14:05] Sam Lee: build is red again
[14:06] Priya: looks like the migration step
```

The buffer for that conversation is emptied when it is delivered, so each message reaches the agent at most once. Files posted without a mention appear in the buffer as `[shared file: name]` and are not downloaded. Private and shared channels never deliver unmentioned messages (a Teams restriction), so mentions there arrive without context. The buffer only covers messages since the bot joined and since Residuum last started; fetching history from Microsoft Graph instead is tracked in issue #150.

## Where replies go

- A DM reply goes to the owner from the main agent's own conversation.
- A group chat or channel's reply comes from that conversation's own session and always goes back to that same conversation (a channel reply lands in the same thread). It never falls back to the owner's DM: if Teams refuses the post, the output is dropped, the error is logged naming the session and conversation, and main gets a notice so the owner can be told if it matters.
- `send_message` with a `conversation` from `list_conversations` posts into that DM, group chat, or channel (a channel post starts a new thread), from whichever agent (main or a session) calls it. If Teams refuses main's own send (or the conversation is unknown), the owner sees a notice in the web UI and main gets a `[Delivery Failed]` message; a session's send is subject to the same never-falls-back rule as its own replies above.
- Other proactive output from main with no originating Teams message — `send_message` without a conversation, results routed through `idle_channel = "teams"`, background turns whose last user message came from Teams — goes to the owner's DM.
- When a main-agent turn started from Teams fails, the plain-language error goes back where its reply would have. System notices and other errors are never sent to Teams; they appear in the web UI.
- The web UI follows the main agent's turns from Teams as they happen: the owner's message, the agent's text and tool calls in order, the reply and the end of the turn, each marked as having come from Teams. A web message sent while such a turn runs joins it, the same as a Teams message would. See [The Main Conversation Stream](turn-control.md#the-main-conversation-stream).
- If a turn is already running when another Teams message arrives, the new message joins that turn (main's own turn for the owner's DM, or a session's turn for its own conversation) and the answer goes to the conversation that started it.

Long replies are split into chunks of about 20 KB (Teams rejects larger activities). A typing indicator shows in the target conversation while a turn runs — for the main agent's own turn, and equally for a group chat or non-owner DM's own conversation session turn (see [Conversation Routing](background-tasks.md#conversation-routing)), each driven by its own lifecycle signal so one doesn't depend on the other.

## Attachments

Files shared in a DM or in a message that @mentions the bot are downloaded into the agent inbox (`inbox/agent/`) and noted in the message, with an inbox item alongside; supported images under the inline limit are also shown to the model. Inline images pasted into a chat are fetched with the bot's token.

The agent cannot send files over Teams: bots can only deliver files through a consent-card upload flow, which this interface does not implement. When the agent sends a file to Teams, the text goes through with a note giving the file's path, and the file stays available in the web UI.

## Configuration

```toml
[teams]
app_id = "11111111-2222-3333-4444-555555555555"   # bot ID from the Teams Developer Portal
tenant_id = "your-directory-tenant-id"
app_password = "secret:teams"                     # client secret; or a "${ENV_VAR}" reference, or a literal
respond_to_others = false
context_messages = 20
port = 7701
```

`app_id`, `tenant_id`, and `app_password` are required whenever the section is present; a missing one disables Teams — with a notice naming the missing field — rather than leaving a bot that silently never answers, or failing the rest of `config.toml`. Changing any `[teams]` value, or the gateway `bind` it shares, restarts the Teams listener on reload. Replies are sent with a client-credentials token from `login.microsoftonline.com/{tenant_id}`, cached until shortly before it expires.

## State

`teams_state.json` in the workspace root holds the owner and a reference (conversation ID, service URL, kind, label) for every conversation the bot has seen or been added to. References are what make proactive messages possible — Teams gives a bot no way to open or list a conversation it has never heard from. Removing the bot from a conversation, or uninstalling the app there, drops its reference. These references are what `list_conversations` shows for Teams; DMs are labelled with the person (`direct message with Jane Doe`).

A corrupt `teams_state.json` is moved aside (to `teams_state.json.corrupt`) and the interface starts fresh with a notice, rather than staying down until someone fixes the file by hand — the owner and every conversation reference are lost, so the owner will need to message the bot again to be recognized.

## Automated Setup & Toolkit Runner

Residuum integrates the Microsoft 365 Agents Toolkit (`@microsoft/m365agentstoolkit-cli`) to automate bot registration, provisioning, app packaging, and sideloading without manual portal clicks.

### Job Lifecycle and Phases

Automated setup runs as an in-memory, agent-scoped job managed by `TeamsSetupJobManager`. Only one job can run per agent at a time (concurrent attempts return HTTP `409 Conflict`). The job executes through sequential phases:

1. **Check Prerequisites (`check_prereqs`)**: Verifies that Node.js (v18+ LTS recommended) and npm are installed in `PATH`.
2. **Install CLI (`install_cli`)**: Installs the pinned Microsoft 365 Agents Toolkit CLI (`@microsoft/m365agentstoolkit-cli@1.1.17`) into `<hub>/tools/m365agentstoolkit`. Skipped if already installed.
3. **Sign In (`sign_in`)**: Checks whether the user is already authenticated via `atk auth list m365`. If not, runs `atk auth login m365`, extracts the login URL and local redirect port, and transitions the job to `waiting_for_user`. Once the user authenticates (or forwards the OAuth callback URL via `forward_redirect`), execution resumes.
4. **Scaffold (`scaffold`)**: Generates the ATK project files (`m365agents.yml`, `appPackage/manifest.json`, icons) in `<agent>/teams-app/`.
5. **Provision (`provision`)**: Runs `atk provision --env residuum --interactive false` to create the Entra app for the bot (with a client secret and a service principal in the tenant, which the bot needs to get Bot Connector tokens from the tenant's token endpoint), register it with the Bot Framework for the Teams channel, and write the credentials to `env/.env.residuum` and `env/.env.residuum.user`. If provisioning fails mid-way, partial resources (e.g., `BOT_ID`) are captured and preserved so they can be cleaned up or retried.
6. **Import (`import`)**: Decrypts the bot password, stores it in Residuum's `SecretStore` under secret `teams`, updates `[teams]` in the agent's `config.toml`, and creates checkpoints.
7. **Install App (`install_app`)**: Optional post-setup step that sideloads the generated `appPackage.residuum.zip` into the user's Teams tenant via `atk install`.

### File Layout

- `<hub>/tools/m365agentstoolkit/`: Hub-level tool directory hosting the isolated npm installation and binary wrapper for the Agents Toolkit CLI.
- `<agent>/teams-app/`: Agent-specific project workspace holding:
  - `m365agents.yml`: Project manifest configuring ATK actions.
  - `appPackage/`: App manifest template (`manifest.json`), icons (`color.png`, `outline.png`), and built deployment package (`build/appPackage.residuum.zip`).
  - `env/.env.residuum` & `env/.env.residuum.user`: Environment files containing `BOT_ID`, `TEAMS_APP_TENANT_ID`, `TEAMS_APP_ID`, and encrypted bot credentials.

### Authentication and Token Cache

The ATK CLI stores Microsoft 365 account credentials in `~/.fx/account/`. When a user signs in, tokens are cached there and reused across setup runs.
To sign out or switch accounts:
- Call the cleanup endpoint with `sign_out: true` or run `atk auth logout m365`.

### Multi-Tenancy Considerations

The Microsoft 365 Agents Toolkit authentication cache (`~/.fx/account/`) is user-global. All agents set up on the same machine share the signed-in tenant unless explicitly logged out and switched. Each agent requires its own bot registration and unique listener port (e.g., `7701`, `7702`), but can share the same tenant.

### Daemon-Restart and Recovery

Setup jobs are held in daemon memory. If the Residuum daemon restarts while a setup job is running or waiting:
- The in-memory job state is lost, but files created on disk (`<agent>/teams-app/`) remain intact.
- Calling the setup API or asking the agent to retry resumes the workflow: existing tools and signed-in status are detected immediately, and already-scaffolded files are preserved unless `replace_existing` is specified.

### Cleanup Endpoint

The cleanup endpoint (`POST /api/teams-setup/cleanup` or `residuum teams cleanup`) allows selective or complete cleanup of local assets:
- `project_files: true` / `--project-files`: Deletes `<agent>/teams-app/`.
- `cli: true` / `--cli`: Removes `<hub>/tools/m365agentstoolkit/`.
- `sign_out: true` / `--sign-out`: Runs `atk auth logout m365` to clear credentials.

### CLI Commands

The `residuum teams` subcommands provide terminal access to toolkit management and automation:

- `residuum teams import-atk --agent <agent> [--dir <dir>]`: Securely decrypts bot credentials from `<dir>/env/.env.residuum.user`, stores the password in the encrypted secret store under secret `teams`, and updates `[teams]` in the agent's `config.toml`.
- `residuum teams forward-redirect "<url>"`: Forwards an OAuth callback redirect URL (`http://localhost:<port>/?code=...`) to the local listener during remote sign-in.
- `residuum teams atk-login --agent <agent> [--status] [--cancel]`: Manages detached Microsoft 365 sign-in. Starts the login process in the background and prints the browser authorization URL, inspects running sign-in status, or cancels an active login.
- `residuum teams atk-scaffold --agent <agent> [--endpoint <url>] [--dir <dir>] [--force] [--bot-name <name>] ...`: Scaffolds the ATK project files (`m365agents.yml`, `appPackage/manifest.json`, icons) in `<agent>/teams-app/`, using the derived cloud endpoint or an explicit messaging endpoint.
- `residuum teams atk-paths --agent <agent> [--json]`: Resolves standard project directory, ATK binary, package zip, and environment file paths.
- `residuum teams cleanup --agent <agent> [--project-files] [--cli] [--sign-out] [--json]`: Selectively removes local project files, the hub-level ATK CLI installation, or signs out of Microsoft 365.

## Code

`src/interfaces/teams/`: `mod.rs` (listener, worker, reply routing), `atk_runner.rs` (CLI execution, log streaming, prereqs, sign-in), `setup_job.rs` (job lifecycle manager), `setup_types.rs` (contract types), `auth.rs` (token validation), `handler.rs` (activity handling), `connector.rs` (outbound calls), `store.rs`, `subscriber.rs`, `activity.rs` (wire types). The unmentioned-message buffer is `src/interfaces/context_buffer.rs`, shared with Discord and Telegram.
