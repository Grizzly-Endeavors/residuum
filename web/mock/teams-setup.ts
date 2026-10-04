import type {
  CleanupResult,
  LogLine,
  LogStream,
  TeamsSetupJob,
  TeamsSetupPrereqs,
  TeamsSetupStart,
} from "../src/lib/generated/protocol";
import { json, parseJsonObject, readBody, readJsonObject } from "./http";
import type { Route, RouteContext } from "./routes";
import type { MockState } from "./state";

/**
 * Default prerequisites for Microsoft Teams setup wizard in mock mode.
 */
export function defaultTeamsSetupPrereqs(state: MockState): TeamsSetupPrereqs {
  const teamsAlreadyConfigured =
    state.configToml.includes("teams_app_id") && !state.configToml.includes('teams_app_id = ""');

  return {
    node: { found: true, version: "v20.11.0", path: "/usr/bin/node" },
    npm: { found: true, version: "10.2.4", path: "/usr/bin/npm" },
    atk: {
      installed: false,
      version: null,
      path: null,
      pinned_version: "1.1.17",
      install_dir: "~/.residuum/hub/tools/m365agentstoolkit",
    },
    min_node_version: "18.0.0",
    teams_already_configured: teamsAlreadyConfigured,
    suggested_endpoint: "https://my-hub.residuum.cloud/api/teams/messages",
    suggested_endpoint_source: "residuum_cloud",
    manual_guide_url: "https://residuum.dev/docs/guides/teams-setup",
  };
}

function getPrereqsForState(state: MockState): TeamsSetupPrereqs {
  const teamsConfigured = /(?:^|\n)[ \t]*\[teams\]/.test(state.configToml);

  state.teamsSetupPrereqs ??= defaultTeamsSetupPrereqs(state);
  if (teamsConfigured) {
    state.teamsSetupPrereqs.teams_already_configured = true;
  }
  return state.teamsSetupPrereqs;
}

let logSeqCounter = 1;

function addLog(state: MockState, stream: LogStream, text: string): LogLine {
  state.teamsSetupAllLogs ??= [];
  const line: LogLine = {
    seq: logSeqCounter++,
    stream,
    text,
    at: state.env.clock.iso(),
  };
  state.teamsSetupAllLogs.push(line);
  if (state.teamsSetupJob) {
    state.teamsSetupJob.last_seq = line.seq;
  }
  return line;
}

interface StoredFormData {
  botName: string;
}

const jobFormData = new Map<string, StoredFormData>();

function advanceJob(state: MockState, job: TeamsSetupJob): void {
  const formData = jobFormData.get(job.agent);
  const botName = formData?.botName ?? "My Teams Bot";
  const shouldFailProvision = botName.includes("fail_provision") || botName.includes("fail");

  switch (job.phase) {
    case "check_prereqs": {
      addLog(state, "info", "Checking prerequisites: Node.js, npm, Agents Toolkit CLI...");
      addLog(state, "stdout", "Node.js v20.11.0 found at /usr/bin/node");
      addLog(state, "stdout", "npm 10.2.4 found at /usr/bin/npm");
      if (!job.completed_phases.includes("check_prereqs")) {
        job.completed_phases.push("check_prereqs");
      }
      const prereqs = getPrereqsForState(state);
      if (!prereqs.atk.installed) {
        addLog(state, "info", "Installing @microsoft/m365agentstoolkit-cli@1.1.17 locally...");
        addLog(state, "stdout", "added 82 packages in 2.8s");
        addLog(state, "stdout", "Agents Toolkit CLI installed successfully.");
        if (!job.completed_phases.includes("install_cli")) {
          job.completed_phases.push("install_cli");
        }
        prereqs.atk.installed = true;
        prereqs.atk.version = "1.1.17";
        prereqs.atk.path = "~/.residuum/hub/tools/m365agentstoolkit/node_modules/.bin/atk";
      }
      job.phase = "sign_in";
      job.phase_started_at = state.env.clock.iso();
      job.state = "waiting_for_user";
      job.sign_in = {
        login_url: "http://localhost:4321/auth",
        redirect_port: 4321,
      };
      addLog(state, "info", "Waiting for Microsoft 365 sign-in...");
      addLog(state, "info", "Open the sign-in URL or paste the redirect URL to proceed.");
      break;
    }
    case "install_cli": {
      addLog(state, "stdout", "added 82 packages in 2.8s");
      addLog(state, "stdout", "Agents Toolkit CLI installed successfully.");
      if (!job.completed_phases.includes("install_cli")) {
        job.completed_phases.push("install_cli");
      }
      const prereqs = getPrereqsForState(state);
      prereqs.atk.installed = true;
      prereqs.atk.version = "1.1.17";
      prereqs.atk.path = "~/.residuum/hub/tools/m365agentstoolkit/node_modules/.bin/atk";

      job.phase = "sign_in";
      job.phase_started_at = state.env.clock.iso();
      job.state = "waiting_for_user";
      job.sign_in = {
        login_url: "http://localhost:4321/auth",
        redirect_port: 4321,
      };
      addLog(state, "info", "Waiting for Microsoft 365 sign-in...");
      addLog(state, "info", "Open the Microsoft sign-in link to authenticate your account.");
      break;
    }
    case "sign_in": {
      // While waiting_for_user, does not advance automatically
      break;
    }
    case "scaffold": {
      addLog(state, "info", "Scaffolding Teams app package files...");
      addLog(state, "stdout", "Created appPackage/manifest.json");
      addLog(state, "stdout", "Created m365agents.yml");
      addLog(state, "stdout", "Created env/.env.residuum");
      if (!job.completed_phases.includes("scaffold")) {
        job.completed_phases.push("scaffold");
      }
      job.phase = "provision";
      job.phase_started_at = state.env.clock.iso();
      addLog(state, "info", "Provisioning Azure Entra ID app and Teams bot registration...");
      break;
    }
    case "provision": {
      job.created.bot_id = "28374619-abcd-4ef0-9123-abcdef012345";
      job.created.tenant_id = "87654321-dcba-4321-abcd-0987654321ba";

      if (shouldFailProvision) {
        job.state = "failed";
        job.error = {
          phase: "provision",
          message: "Failed to provision Azure AD application in your Microsoft 365 tenant.",
          detail: "ATK provision error: Connection timed out while contacting Entra ID endpoint.",
        };
        addLog(state, "stderr", "Error: AAD application creation timed out in your tenant.");
        addLog(state, "info", "Provisioning stopped with errors.");
        return;
      }

      job.created.teams_app_id = "teams-app-777888999";
      addLog(state, "stdout", `Bot registered: ${job.created.bot_id}`);
      addLog(state, "stdout", `Teams app registered: ${job.created.teams_app_id}`);
      if (!job.completed_phases.includes("provision")) {
        job.completed_phases.push("provision");
      }
      job.phase = "import";
      job.phase_started_at = state.env.clock.iso();
      addLog(state, "info", "Importing credentials into Residuum SecretStore...");
      break;
    }
    case "import": {
      if (!job.completed_phases.includes("import")) {
        job.completed_phases.push("import");
      }
      job.result = {
        bot_id: job.created.bot_id ?? "bot-default-id",
        tenant_id: job.created.tenant_id ?? "tenant-default-id",
        teams_app_id: job.created.teams_app_id,
        package_path: `/home/user/.residuum/${job.agent}/teams-app/appPackage/build/appPackage.residuum.zip`,
        project_dir: `/home/user/.residuum/${job.agent}/teams-app`,
      };
      job.state = "succeeded";
      addLog(state, "info", "Teams configuration saved and checkpointed.");
      addLog(state, "info", "Teams setup completed successfully!");
      break;
    }
    case "install_app": {
      // Handled via POST /job/install-app
      break;
    }
  }
}

/** `GET /api/teams-setup/prereqs` */
function getPrereqs({ res, state }: RouteContext): void {
  const prereqs = getPrereqsForState(state);
  json(res, 200, prereqs);
}

/** `GET /api/teams-setup/job` */
function getJob({ res, state, query }: RouteContext): void {
  const job = state.teamsSetupJob;
  if (!job) {
    json(res, 404, { error: "No Teams setup job found for this agent" });
    return;
  }

  if (job.state === "running") {
    advanceJob(state, job);
  }

  const logSinceParam = query.get("log_since");
  const logSince = logSinceParam !== null ? parseInt(logSinceParam, 10) : undefined;
  const filteredLog =
    logSince !== undefined && !isNaN(logSince)
      ? (state.teamsSetupAllLogs ?? []).filter((l) => l.seq > logSince)
      : (state.teamsSetupAllLogs ?? []);

  const responseJob: TeamsSetupJob = {
    ...job,
    log: filteredLog,
  };
  json(res, 200, responseJob);
}

/** `POST /api/teams-setup/job` */
async function startJob({ req, res, state }: RouteContext): Promise<void> {
  const raw = await readBody(req);
  const start = parseJsonObject(raw) as unknown as TeamsSetupStart;

  const prereqs = getPrereqsForState(state);

  // If a job is already running
  if (
    state.teamsSetupJob &&
    (state.teamsSetupJob.state === "running" || state.teamsSetupJob.state === "waiting_for_user")
  ) {
    json(res, 409, state.teamsSetupJob);
    return;
  }

  // Consent check
  if (!start.consent_install_cli && !prereqs.atk.installed) {
    json(res, 400, {
      message: "You must consent to installing the Agents Toolkit CLI to continue.",
    });
    return;
  }

  // Replace existing check
  if (!start.replace_existing && prereqs.teams_already_configured) {
    json(res, 400, {
      message:
        "Microsoft Teams is already configured. Confirm replacing the existing bot to continue.",
    });
    return;
  }

  // Form validation
  if (!start.form.bot_name.trim()) {
    json(res, 400, { message: "Bot name is required." });
    return;
  }
  if (start.form.short_description.length > 80) {
    json(res, 400, { message: "Short description must be 80 characters or fewer." });
    return;
  }
  if (start.form.long_description.length > 4000) {
    json(res, 400, { message: "Long description must be 4000 characters or fewer." });
    return;
  }
  if (!start.form.developer_url.trim()) {
    json(res, 400, { message: "Developer URL is required." });
    return;
  }
  if (!start.form.messaging_endpoint.trim()) {
    json(res, 400, { message: "Messaging endpoint is required." });
    return;
  }

  jobFormData.set(state.agentName, { botName: start.form.bot_name });

  const now = state.env.clock.iso();
  state.teamsSetupAllLogs = [];

  const job: TeamsSetupJob = {
    agent: state.agentName,
    state: "running",
    phase: "check_prereqs",
    completed_phases: [],
    started_at: now,
    phase_started_at: now,
    sign_in: null,
    log: [],
    last_seq: 0,
    error: null,
    created: {
      bot_id: null,
      teams_app_id: null,
      tenant_id: null,
      entra_url:
        "https://portal.azure.com/#view/Microsoft_AAD_RegisteredApps/ApplicationMenuBlade/~/Overview/appId/",
      dev_portal_url: "https://dev.teams.microsoft.com/apps/",
    },
    result: null,
    app_installed: false,
  };
  state.teamsSetupJob = job;

  // Run initial phase
  advanceJob(state, job);

  json(res, 200, { ...job, log: state.teamsSetupAllLogs ?? [] });
}

/** `POST /api/teams-setup/job/redirect` */
async function submitRedirect({ req, res, state }: RouteContext): Promise<void> {
  const job = state.teamsSetupJob;
  if (!job) {
    json(res, 404, { error: "No Teams setup job found" });
    return;
  }

  const raw = await readBody(req);
  const body = parseJsonObject(raw);
  const url = typeof body.url === "string" ? body.url : "";

  if (job.phase !== "sign_in" || job.state !== "waiting_for_user") {
    json(res, 400, { message: "Job is not waiting for a sign-in redirect." });
    return;
  }

  const expectedPort = job.sign_in?.redirect_port ?? 4321;
  const expectedPrefix = `http://localhost:${expectedPort}/`;
  if (!url.startsWith(expectedPrefix)) {
    json(res, 400, {
      message: `Invalid redirect URL: must start with ${expectedPrefix}`,
    });
    return;
  }

  addLog(state, "info", "Redirect URL received. Authentication token verified.");
  if (!job.completed_phases.includes("sign_in")) {
    job.completed_phases.push("sign_in");
  }
  job.sign_in = null;
  job.phase = "scaffold";
  job.phase_started_at = state.env.clock.iso();
  job.state = "running";

  json(res, 200, { ...job, log: state.teamsSetupAllLogs ?? [] });
}

/** `POST /api/teams-setup/job/cancel` */
function cancelJob({ res, state }: RouteContext): void {
  const job = state.teamsSetupJob;
  if (!job) {
    json(res, 404, { error: "No Teams setup job found" });
    return;
  }

  job.state = "cancelled";
  job.sign_in = null;
  addLog(state, "info", "Teams setup job cancelled by user.");
  json(res, 200, { ...job, log: state.teamsSetupAllLogs ?? [] });
}

/** `POST /api/teams-setup/job/retry` */
function retryJob({ res, state }: RouteContext): void {
  const job = state.teamsSetupJob;
  if (!job) {
    json(res, 404, { error: "No Teams setup job found" });
    return;
  }

  job.error = null;
  job.state = "running";
  job.phase_started_at = state.env.clock.iso();

  // If provision failed, clear failure so retry can proceed
  const formData = jobFormData.get(job.agent);
  if (formData?.botName.includes("fail_provision")) {
    formData.botName = "Retried Bot";
  }

  addLog(state, "info", `Retrying from phase '${job.phase}'...`);
  advanceJob(state, job);

  json(res, 200, { ...job, log: state.teamsSetupAllLogs ?? [] });
}

/** `POST /api/teams-setup/job/install-app` */
function installApp({ res, state }: RouteContext): void {
  const job = state.teamsSetupJob;
  if (!job) {
    json(res, 404, { error: "No Teams setup job found" });
    return;
  }

  if (job.state !== "succeeded") {
    json(res, 400, { message: "App installation requires a successfully completed setup." });
    return;
  }

  const formData = jobFormData.get(job.agent);
  if (formData?.botName === "fail_install_app") {
    json(res, 400, {
      message:
        "App installation failed: Sideloading apps is blocked by your Microsoft 365 tenant policy. IT administrator approval is required.",
    });
    return;
  }

  job.app_installed = true;
  if (!job.completed_phases.includes("install_app")) {
    job.completed_phases.push("install_app");
  }
  addLog(state, "info", "Teams app package installed directly into your Microsoft 365 account.");
  json(res, 200, { ...job, log: state.teamsSetupAllLogs ?? [] });
}

/** `GET /api/teams-setup/job/package` */
function getPackage({ res, state }: RouteContext): void {
  const job = state.teamsSetupJob;
  if (!job) {
    json(res, 404, { error: "No Teams setup job found" });
    return;
  }

  const zipHeader = Buffer.from("PK\x03\x04mock-teams-app-package-zip");
  res.writeHead(200, {
    "Content-Type": "application/zip",
    "Content-Disposition": 'attachment; filename="appPackage.zip"',
    "Content-Length": zipHeader.length,
  });
  res.end(zipHeader);
}

/** `DELETE /api/teams-setup/job` */
function deleteJob({ res, state }: RouteContext): void {
  const job = state.teamsSetupJob;
  if (!job) {
    json(res, 404, { error: "No Teams setup job found" });
    return;
  }

  if (job.state === "running" || job.state === "waiting_for_user") {
    json(res, 409, { error: "Cannot delete a running Teams setup job" });
    return;
  }

  state.teamsSetupJob = null;
  state.teamsSetupAllLogs = [];
  jobFormData.delete(state.agentName);
  res.writeHead(204).end();
}

/** `POST /api/teams-setup/cleanup` */
async function cleanup({ req, res }: RouteContext): Promise<void> {
  const body = await readJsonObject(req);
  const removed: string[] = [];

  if (body.project_files === true) removed.push("project_files");
  if (body.cli === true) removed.push("cli");
  if (body.sign_out === true) removed.push("sign_out");

  const result: CleanupResult = {
    removed,
    failed: [],
  };
  json(res, 200, result);
}

/** Mock control: `POST /api/mock/teams-setup/prereqs` */
async function mockSetPrereqs({ req, res, state }: RouteContext): Promise<void> {
  const body = (await readJsonObject(req)) as unknown as Partial<TeamsSetupPrereqs>;
  const current = getPrereqsForState(state);
  state.teamsSetupPrereqs = { ...current, ...body };
  json(res, 200, state.teamsSetupPrereqs);
}

/** Mock control: `POST /api/mock/teams-setup/fail-provision` */
async function mockSetFailProvision({ req, res, state }: RouteContext): Promise<void> {
  const raw = await readBody(req);
  const parsed = raw.trim() !== "" ? parseJsonObject(raw) : {};
  const fail = typeof parsed.fail === "boolean" ? parsed.fail : true;
  const current = jobFormData.get(state.agentName) ?? { botName: "Bot" };
  current.botName = fail ? "fail_provision" : "Bot";
  jobFormData.set(state.agentName, current);
  json(res, 200, { fail });
}

export const teamsSetupRoutes: readonly Route[] = [
  { method: "GET", pattern: "/api/teams-setup/prereqs", handler: getPrereqs },
  { method: "GET", pattern: "/api/teams-setup/job", handler: getJob },
  { method: "POST", pattern: "/api/teams-setup/job", handler: startJob },
  { method: "POST", pattern: "/api/teams-setup/job/redirect", handler: submitRedirect },
  { method: "POST", pattern: "/api/teams-setup/job/cancel", handler: cancelJob },
  { method: "POST", pattern: "/api/teams-setup/job/retry", handler: retryJob },
  { method: "POST", pattern: "/api/teams-setup/job/install-app", handler: installApp },
  { method: "GET", pattern: "/api/teams-setup/job/package", handler: getPackage },
  { method: "DELETE", pattern: "/api/teams-setup/job", handler: deleteJob },
  { method: "POST", pattern: "/api/teams-setup/cleanup", handler: cleanup },
  { method: "POST", pattern: "/api/mock/teams-setup/prereqs", handler: mockSetPrereqs },
  {
    method: "POST",
    pattern: "/api/mock/teams-setup/fail-provision",
    handler: mockSetFailProvision,
  },
];
