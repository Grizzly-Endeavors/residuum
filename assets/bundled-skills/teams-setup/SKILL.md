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

2. Inspect standard project paths using Residuum:
   ```bash
   residuum teams atk-paths --agent <agent_name> --json
   ```
   This returns absolute paths:
   - `project_dir`: `<residuum_root>/<agent_name>/teams-app`
   - `atk_bin`: `<hub>/tools/m365agentstoolkit/node_modules/.bin/atk`
   - `package_zip`: `<project_dir>/appPackage/build/appPackage.residuum.zip`
   - `env_file`: `<project_dir>/env/.env.residuum`

3. Install pinned ATK if not already present:
   ```bash
   npm install --prefix <hub>/tools/m365agentstoolkit @microsoft/m365agentstoolkit-cli@1.1.17
   ```

## 3. Microsoft 365 Authentication

Because agent tool execution via `exec` runs synchronously and waits for command exit, running interactive login directly will hang. Instead, launch login detached using the cross-platform `atk-login` helper:

1. Start login detached via Residuum:
   ```bash
   residuum teams atk-login --agent <agent_name>
   ```
   This command starts the login process in the background, waits until the sign-in URL appears, prints the URL and redirect port, and returns while the login keeps running.
   You can also inspect its progress or cancel:
   - Check status: `residuum teams atk-login --agent <agent_name> --status`
   - Cancel: `residuum teams atk-login --agent <agent_name> --cancel`

2. Present the printed Microsoft sign-in URL clearly to the user in chat.

3. When the user signs in on a remote or different machine, their browser redirects to `http://localhost:<port>/?code=...` and cannot connect.
   Ask the user to paste that full address into chat. Then forward the redirect to the local listener:
   ```bash
   residuum teams forward-redirect "<pasted-url>"
   ```
   The local listener completes the token exchange and exits.
   Verify with `residuum teams atk-login --agent <agent_name> --status` that sign-in completed.

## 4. Scaffold the Project

1. Check if `[teams]` is already configured in `config/config.toml`.
   If it is, warn the user: provisioning a new bot will replace the current Teams bot configuration in Residuum. (The existing bot registration will remain in their tenant until manually deleted).

2. Scaffold the ATK project using the built-in subcommand:
   ```bash
   residuum teams atk-scaffold --agent <agent_name>
   ```
   - If connected to Residuum Cloud, `atk-scaffold` automatically derives your cloud Teams messaging endpoint (`https://<origin>/teams/<instance>/<agent_name>`).
   - If Residuum Cloud is not connected or using a custom tunnel, specify `--endpoint <URL>`.
   - Templates are copied from `team/skills/teams-setup/templates/`, falling back to embedded templates.
   - Icon dimensions (192x192 color PNG, 32x32 outline PNG) and dotenv variable escaping are strictly validated.
   - Existing project files are protected against accidental overwriting (pass `--force` to overwrite).

## 5. Provision Resources

Run provision in non-interactive mode:
```bash
<atk_bin> provision --folder <project_dir> --env residuum --interactive false
```

### Partial Failure Handling
If provision fails:
1. Read `BOT_ID`, `TEAMS_APP_ID`, and `TEAMS_APP_TENANT_ID` from `<project_dir>/env/.env.residuum` if present.
2. Report created resources to the user along with direct links to manage or delete them:
   - [Microsoft Entra Admin Center](https://entra.microsoft.com/#view/Microsoft_AAD_RegisteredApps/ApplicationsListBlade) (Bot App Registration)
   - [Teams Developer Portal](https://dev.teams.microsoft.com/bots) (Bot Framework Registration)
3. You can retry provision in the same `<project_dir>` directory (ATK reuses existing IDs from `.env.residuum`).

## 6. Import Credentials and Configuration

Once provision succeeds:
Run the Residuum import command:
```bash
residuum teams import-atk --agent <agent_name> --dir <project_dir>
```

> **SECURITY RULE**: Never read, print, or log `.env.*.user` directly. `residuum teams import-atk` securely decrypts `SECRET_BOT_PASSWORD`, stores it in the encrypted secret store as `secret:teams`, and patches `[teams]` in `config/config.toml` through a checkpointed write.

**Live Daemon Pickup**:
The running Residuum hub daemon watches `<agent_dir>/config/config.toml` via an in-process filesystem watcher (debounced 500ms). When `import-atk` updates `config.toml` and the secret store, the daemon automatically reloads the agent configuration and active secrets. No manual restart of `residuum serve` is required.

## 7. App Package Installation

1. Sideload the generated app package:
   ```bash
   <atk_bin> install --file-path <package_zip>
   ```
2. If sideloading is blocked by tenant policies, provide the absolute path `<package_zip>` to the user so they can submit it for IT admin approval or upload it via Teams App Management.

## 8. Cleanup Instructions

Explain cleanup options to the user:
- The local `<project_dir>` directory can be kept for future updates or removed.
- The toolkit CLI at `<hub>/tools/m365agentstoolkit` can be deleted at any time.
- To sign out of Microsoft 365, run `<atk_bin> auth logout m365`.
- Cleanup can be performed using Residuum:
  ```bash
  residuum teams cleanup --agent <agent_name> [--project-files] [--cli] [--sign-out]
  ```
- Cloud resources (Entra app registration and Developer Portal bot) are retained in the user's tenant until manually deleted via the portal links.
