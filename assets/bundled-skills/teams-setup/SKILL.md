---
name: teams-setup
description: Set up this agent as a Microsoft Teams bot using Microsoft 365 Agents Toolkit.
---

# Microsoft Teams Setup via Agents Toolkit (ATK)

This skill guides you through setting up this agent as a Microsoft Teams bot using Microsoft 365 Agents Toolkit (`atk`).

## 1. Prerequisites and Consent

Before running any commands:
1. Explain to the user that Node.js and npm are required.
2. Explain that the toolkit CLI (`@microsoft/m365agentstoolkit-cli@1.1.17`) will be installed locally inside Residuum's hub directory (`<hub>/tools/m365agentstoolkit`), without global packages or root privileges.
3. Offer the manual setup guide: [Manual Teams Setup Guide](https://github.com/Grizzly-Endeavors/residuum/blob/main/docs/guides/teams-setup.md).
4. **Wait for explicit user consent** before proceeding.

## 2. Environment & CLI Setup

1. Check Node.js and npm versions using `exec`:
   ```bash
   node -v
   npm -v
   ```
   ATK requires Node.js >= 12 (Node 18+ LTS recommended). If missing, inform the user and point to the manual setup guide.

2. Determine paths:
   - Hub directory: `<hub>` is typically `~/.residuum/hub` (or the `hub/` directory beside the agent).
   - Agent workspace directory: `<agent_dir>` is the absolute path to your workspace directory.
   - Toolkit installation directory: `<hub>/tools/m365agentstoolkit`.
   - ATK binary: `<hub>/tools/m365agentstoolkit/node_modules/.bin/atk` (or `atk.cmd` on Windows).

3. Install pinned ATK if not already present:
   ```bash
   npm install --prefix <hub>/tools/m365agentstoolkit @microsoft/m365agentstoolkit-cli@1.1.17
   ```

## 3. Microsoft 365 Authentication

1. Run the login command non-interactively:
   ```bash
   <hub>/tools/m365agentstoolkit/node_modules/.bin/atk auth login m365
   ```
2. The command outputs a sign-in URL:
   `Log in to your Microsoft 365 account - opening default web browser at <url>`
   Parse the URL from stdout and present it clearly to the user in chat.
3. When the user signs in on a remote or different machine, their browser will redirect to `http://localhost:<port>/?code=...` and fail to connect.
   Ask the user to paste that full address into chat. Then forward the redirect to the local listener:
   ```bash
   residuum teams forward-redirect "<pasted-url>"
   ```
   The local listener completes the token exchange and exits.

## 4. Scaffold the Project

1. Check if `[teams]` is already configured in `config/config.toml`.
   If it is, warn the user: provisioning a new bot will replace the current Teams bot configuration in Residuum. (The existing bot registration will remain in their tenant until manually deleted).

2. Create `<agent_dir>/teams-app/` and copy template files from `team/skills/teams-setup/templates/`:
   ```bash
   mkdir -p <agent_dir>/teams-app/appPackage <agent_dir>/teams-app/env
   cp team/skills/teams-setup/templates/m365agents.yml <agent_dir>/teams-app/
   cp team/skills/teams-setup/templates/appPackage/* <agent_dir>/teams-app/appPackage/
   cp team/skills/teams-setup/templates/env/.env.residuum <agent_dir>/teams-app/env/
   ```

3. Update `<agent_dir>/teams-app/env/.env.residuum`:
   - Set `BOT_DISPLAY_NAME` and `TEAMS_APP_NAME` to the agent's name or the user's preferred bot name.
   - Set `BOT_ENDPOINT` to the agent's public Teams messaging endpoint (e.g. from Residuum Cloud relay `https://<relay-domain>/teams/<instance>/<agent>` or your configured tunnel URL `https://.../api/teams/messages`).

## 5. Provision Resources

Run provision in non-interactive mode:
```bash
<hub>/tools/m365agentstoolkit/node_modules/.bin/atk provision --folder <agent_dir>/teams-app --env residuum --interactive false
```

### Partial Failure Handling
If provision fails:
1. Read `BOT_ID`, `TEAMS_APP_ID`, and `TEAMS_APP_TENANT_ID` from `<agent_dir>/teams-app/env/.env.residuum` if present.
2. Report created resources to the user along with direct links to manage or delete them:
   - [Microsoft Entra Admin Center](https://entra.microsoft.com/#view/Microsoft_AAD_RegisteredApps/ApplicationsListBlade) (Bot App Registration)
   - [Teams Developer Portal](https://dev.teams.microsoft.com/bots) (Bot Framework Registration)
3. You can retry provision in the same `<agent_dir>/teams-app` directory (ATK reuses existing IDs from `.env.residuum`).

## 6. Import Credentials and Configuration

Once provision succeeds:
Run the Residuum import command:
```bash
residuum teams import-atk --agent <agent_name> --dir <agent_dir>/teams-app
```

> **SECURITY RULE**: Never read, print, or log `.env.*.user` directly. `residuum teams import-atk` securely decrypts `SECRET_BOT_PASSWORD`, stores it in the encrypted secret store as `secret:teams`, and patches `[teams]` in `config.toml` through a checkpointed write.

## 7. App Package Installation

1. Sideload the generated app package:
   ```bash
   <hub>/tools/m365agentstoolkit/node_modules/.bin/atk install --file-path <agent_dir>/teams-app/appPackage/build/appPackage.residuum.zip
   ```
2. If sideloading is blocked by tenant policies, provide the absolute path to `<agent_dir>/teams-app/appPackage/build/appPackage.residuum.zip` to the user so they can submit it for IT admin approval or upload it via Teams App Management.

## 8. Cleanup Instructions

Explain cleanup options to the user:
- The local `<agent_dir>/teams-app` directory can be kept for future updates or removed.
- The toolkit CLI at `<hub>/tools/m365agentstoolkit` can be deleted at any time.
- To sign out of Microsoft 365, run `<hub>/tools/m365agentstoolkit/node_modules/.bin/atk auth logout m365`.
- Cloud resources (Entra app registration and Developer Portal bot) are retained in the user's tenant until manually deleted via the portal links.
