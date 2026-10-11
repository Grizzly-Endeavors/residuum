# Set up Residuum in Microsoft Teams

This walks through putting your agent in Microsoft Teams: a bot you can DM, and that you can add to group chats and channels. It needs no Azure subscription. How the interface behaves once running is described in [Microsoft Teams](../systems-usage/teams.md).

You will need:

- A Microsoft 365 work or school account whose Teams lets you upload custom apps. Check in Teams under **Apps → Manage your apps**: if you see **Upload an app**, you're set. If not, your IT admin has to allow custom app upload for you or approve the app.
- Residuum Cloud connected, which gives the bot its public address. Microsoft delivers every Teams message to your bot over the internet; there is no mode where the bot connects out instead. A tunnel of your own to the Teams port works instead, and step 3 covers both.

## Automated Setup (Recommended)

You can set up Teams automatically without manually creating resources in the Microsoft portal.

### Option 1: Web UI Settings Wizard

1. Open **Settings → Connections → Microsoft Teams** in the Residuum web interface.
2. Click **Set up with Microsoft 365 Agents Toolkit**.
3. Follow the guided wizard:
   - Check system prerequisites (Node.js 18+ and npm).
   - If needed, consent to install the Microsoft 365 Agents Toolkit CLI.
   - Click the Microsoft sign-in link to authenticate in your browser. If working on a remote machine, paste the final redirect URL into the prompt.
   - Enter your bot details and icons.
   - Residuum will scaffold the project, provision the bot resources in Entra, and save your credentials automatically.
   - Optionally choose **Install App** to sideload the package directly into Teams.

### Option 2: Ask Your Agent

If your agent has the `teams-setup` skill enabled, you can simply ask it:
> "Set up a Microsoft Teams bot for this agent"

The agent will check your prerequisites, start the background login flow, give you the sign-in URL, provision the resources, and configure its own connection.

---

## Manual Setup

If you prefer to configure the bot yourself using the Teams Developer Portal, follow the steps below.

### 1. Register the bot

1. Open the Teams Developer Portal at [dev.teams.microsoft.com](https://dev.teams.microsoft.com) and sign in with your work account.
2. Go to **Tools → Bot management** and create a new bot. Give it the name you want to see in Teams.
3. Copy the bot's **ID**. This is your `app_id`.
4. Under **Client secrets**, add a secret and copy it right away — it is shown once. This is your `app_password`.
5. Leave the **Endpoint address** for now; you'll fill it in after step 3.

Find your **tenant ID** (`tenant_id`): it's shown on the overview page of the Microsoft Entra admin center, or you can open `https://login.microsoftonline.com/<your-company-domain>/v2.0/.well-known/openid-configuration` in a browser — the GUID in the `issuer` URL is your tenant ID.

## 2. Configure Residuum

In the web UI, open **Settings** (the gear at the bottom of the left-hand rail, or **Settings** in the bottom bar on a phone), choose the agent that should answer in Teams under **Settings for**, and pick **Connections**. In its **Microsoft Teams** group, enter the **App ID**, **Tenant ID** and **Client secret**, then choose **Save changes**. The secret goes into Residuum's encrypted secret store. Leave **Let others talk to this agent** off for now. Teams is set up per agent, so each agent that should be in Teams needs its own bot and its own **Listener port**.

Or in `config.toml`, after storing the secret with `residuum secret set teams`:

```toml
[teams]
app_id = "11111111-2222-3333-4444-555555555555"
tenant_id = "your-directory-tenant-id"
app_password = "secret:teams"
```

Saving reloads the config. The log shows `teams interface listening` with the address — by default port `7701` on the same address as the gateway — and the **Microsoft Teams** group in **Connections** reads **Connected** while the agent is running.

## 3. Set the messaging endpoint

With Residuum Cloud connected, **Settings → (agent) → Connections → Microsoft Teams** shows a **Messaging endpoint** like `https://<instance>.<you>.agent-residuum.com/teams/<agent>`. Copy it. In the Developer Portal bot page, set **Endpoint address** to that URL and save. Residuum delivers messages to this agent while the cloud connection is up. Teams messages can't arrive through it while Residuum Cloud is disconnected.

To reach the bot without Residuum Cloud, point a tunnel at the **Teams port (7701)** only (the **Listener port** in the same **Microsoft Teams** group). Never expose the gateway port (7700): it serves the configuration and secrets API without a login. Everything that reaches port 7701 has to carry a valid Microsoft-signed token, or it's rejected. The endpoint is then `https://…/api/teams/messages`.

**Tailscale Funnel** (free, stable address, no domain needed):

```sh
tailscale funnel --bg 7701
```

Your messaging endpoint is `https://<machine>.<tailnet>.ts.net/api/teams/messages`.

**Cloudflare named tunnel** (free, needs a domain on Cloudflare): add an ingress rule for a hostname such as `teams.example.com` pointing at `http://localhost:7701`. Your endpoint is `https://teams.example.com/api/teams/messages`.

**Microsoft Dev Tunnels** work for trying things out (`devtunnel host -p 7701 --allow-anonymous`), but Microsoft labels them not for production, and an unused tunnel expires after 30 days.

Set **Endpoint address** to that `https://…/api/teams/messages` URL and save.

## 4. Create the Teams app

In the Developer Portal, go to **Apps → New app**:

1. **Basic information**: fill in the name, short and long description, developer name, and website, privacy, and terms URLs (any pages you control will do for a personal app).
2. **App features → Bot**: pick the bot from step 1 (select an existing bot) and enable the scopes **Personal**, **Team**, and **Group chat**.
3. **Permissions → Resource-specific consent**: add the application permissions `ChatMessage.Read.Chat` and `ChannelMessage.Read.Group`. These let the bot see group chat and channel messages that don't mention it, which Residuum hands the agent as context when you do mention it. Without them the bot still works, but mentions arrive without the surrounding conversation.

## 5. Install it and say hello

From the app's page in the Developer Portal, choose **Preview in Teams** (or **Download app package** and use **Apps → Manage your apps → Upload an app** in Teams). Add it for yourself.

Send the bot a direct message. **The first person to DM the bot becomes its owner** — that's what lets it tell you apart from coworkers and where it sends scheduled results and other proactive messages. You'll see `teams owner set from first direct message` in the log.

## 6. Add it to chats and channels

Add the app to a group chat or a team the way you would any app; you'll be asked to grant the chat or team permissions from step 4. In those conversations, @mention the bot to talk to it. Whatever was said since the last mention comes along as background, so "@Residuum can you summarize this?" works.

Coworkers in those conversations can see the bot. By default it answers only you and tells anyone else it only takes requests from you. To let coworkers use it too, turn on **Let others talk to this agent** in **Settings → (agent) → Connections → Microsoft Teams** (`respond_to_others = true`) and choose **Save changes**; the agent always knows who is asking and keeps your private information out of its replies to them.

If you add the resource-specific consent permissions after the app is already in a chat, remove and re-add it there — Teams only starts delivering unmentioned messages after a fresh install.

## Troubleshooting

Messages you send never reach the agent:

- `rejected unauthenticated teams request` in the log: the `reason` names the failed check. `token audience does not match` means `app_id` doesn't match the bot's ID; `serviceurl claim does not match` usually means something between Microsoft and Residuum is rewriting the request.
- `rejected teams activity from a tenant other than the configured one`: `tenant_id` is wrong.
- Nothing in the log at all: the tunnel isn't reaching port 7701, or the Developer Portal endpoint address is wrong. It must end in `/api/teams/messages`.

The agent answers but replies never show up in Teams:

- `could not get a bot token from Microsoft`: the client secret is wrong or expired, or `tenant_id` is wrong. Create a new secret in the Developer Portal and update it under **Client secret** in **Settings → (agent) → Connections → Microsoft Teams**.

The bot says it only takes requests from someone else: someone else DMed it first. See [who the agent answers](../systems-usage/teams.md#who-the-agent-answers) for how to reset the owner.
