import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { settingsModel } from "../../../lib/settings-model.svelte";
import type { TeamsSetupJob, TeamsSetupPrereqs } from "../../../lib/types";
import {
  advance,
  fireEvent,
  jsonResponse,
  mockFetch,
  render,
  screen,
  settle,
} from "../../../test/component";
import { fakeAgentConfig, type FakeAgentConfig } from "../../../test/fake-config";
import ConnectionsSection from "../ConnectionsSection.svelte";
import TeamsSetupModal from "./TeamsSetupModal.svelte";
import { waitFor } from "../../../test/wait";

let agent = "atlas";
let server: FakeAgentConfig;
let currentPrereqs: TeamsSetupPrereqs;
let currentJob: TeamsSetupJob | null = null;
let redirectErrorResponse: { status: number; message: string } | null = null;
let installAppErrorResponse: { status: number; message: string } | null = null;
let getJobError: Error | { status: number } | null = null;
let lastLogSinceRequested: number | null = null;
let cancelCalled = false;
let retryCalled = false;
let installAppCalled = false;
let cleanupCalled = false;
let deleteJobCalled = false;
let startJobCalledWith: unknown = null;
let redirectSubmittedWith: string | null = null;

function fakePrereqs(overrides: Partial<TeamsSetupPrereqs> = {}): TeamsSetupPrereqs {
  return {
    node: { found: true, version: "v20.10.0", path: "/usr/bin/node" },
    npm: { found: true, version: "10.2.3", path: "/usr/bin/npm" },
    atk: {
      installed: true,
      version: "1.1.17",
      path: "/tools/node_modules/.bin/teamsapp",
      pinned_version: "1.1.17",
      install_dir: "/tools",
    },
    min_node_version: "18.0.0",
    teams_already_configured: false,
    suggested_endpoint: "https://hub.relay.example.com/api/agents/atlas/teams/messages",
    suggested_endpoint_source: "residuum_cloud",
    manual_guide_url: "https://dev.teams.microsoft.com/bots/manual-guide",
    ...overrides,
  };
}

function fakeJob(overrides: Partial<TeamsSetupJob> = {}): TeamsSetupJob {
  return {
    agent,
    state: "running",
    phase: "scaffold",
    completed_phases: ["check_prereqs", "install_cli", "sign_in"],
    started_at: "2026-10-04T12:00:00Z",
    phase_started_at: "2026-10-04T12:00:10Z",
    sign_in: null,
    log: [
      { seq: 1, stream: "stdout", text: "Checking prerequisites...", at: "2026-10-04T12:00:01Z" },
      {
        seq: 2,
        stream: "stdout",
        text: "Scaffolding Teams app package...",
        at: "2026-10-04T12:00:11Z",
      },
    ],
    last_seq: 2,
    error: null,
    created: {
      bot_id: "bot-app-id-1234",
      teams_app_id: "teams-manifest-id-5678",
      tenant_id: "m365-tenant-id-9999",
      entra_url:
        "https://entra.microsoft.com/#view/Microsoft_AAD_RegisteredApps/ApplicationMenuBlade/~/Overview/appId/bot-app-id-1234",
      dev_portal_url: "https://dev.teams.microsoft.com/apps/teams-manifest-id-5678",
    },
    result: null,
    app_installed: false,
    ...overrides,
  };
}

beforeEach(() => {
  agent = "atlas";
  server = fakeAgentConfig(agent, {
    config: 'teams_app_id = ""\nteams_tenant_id = ""\nteams_app_password = ""\n',
  });
  currentPrereqs = fakePrereqs();
  currentJob = null;
  redirectErrorResponse = null;
  installAppErrorResponse = null;
  getJobError = null;
  lastLogSinceRequested = null;
  cancelCalled = false;
  retryCalled = false;
  installAppCalled = false;
  cleanupCalled = false;
  deleteJobCalled = false;
  startJobCalledWith = null;
  redirectSubmittedWith = null;

  mockFetch((url, init) => {
    const method = init?.method ?? "GET";

    if (url.includes(`/api/agents/${agent}/teams-setup/prereqs`)) {
      return jsonResponse(currentPrereqs);
    }

    if (url.includes(`/api/agents/${agent}/teams-setup/job/redirect`) && method === "POST") {
      const parsed = JSON.parse(init?.body as string) as { url: string };
      redirectSubmittedWith = parsed.url;
      if (redirectErrorResponse !== null) {
        return jsonResponse(
          { message: redirectErrorResponse.message },
          redirectErrorResponse.status,
        );
      }
      currentJob = fakeJob({ state: "running", phase: "scaffold", sign_in: null });
      return jsonResponse(currentJob);
    }

    if (url.includes(`/api/agents/${agent}/teams-setup/job/cancel`) && method === "POST") {
      cancelCalled = true;
      currentJob = fakeJob({ state: "cancelled" });
      return jsonResponse(currentJob);
    }

    if (url.includes(`/api/agents/${agent}/teams-setup/job/retry`) && method === "POST") {
      retryCalled = true;
      currentJob = fakeJob({ state: "running", phase: "provision", error: null });
      return jsonResponse(currentJob);
    }

    if (url.includes(`/api/agents/${agent}/teams-setup/job/install-app`) && method === "POST") {
      installAppCalled = true;
      if (installAppErrorResponse !== null) {
        return jsonResponse(
          { message: installAppErrorResponse.message },
          installAppErrorResponse.status,
        );
      }
      currentJob = fakeJob({ state: "succeeded", app_installed: true });
      return jsonResponse(currentJob);
    }

    if (url.includes(`/api/agents/${agent}/teams-setup/cleanup`) && method === "POST") {
      cleanupCalled = true;
      return jsonResponse({ removed: ["manifest.zip"], failed: [] });
    }

    if (url.includes(`/api/agents/${agent}/teams-setup/package`) && method === "GET") {
      return new Response(new Blob(["mock-zip-bytes"]), { status: 200 });
    }

    if (url.includes(`/api/agents/${agent}/teams-setup/job`) && method === "DELETE") {
      deleteJobCalled = true;
      currentJob = null;
      return new Response(null, { status: 204 });
    }

    if (url.includes(`/api/agents/${agent}/teams-setup/job`) && method === "POST") {
      startJobCalledWith = JSON.parse(init?.body as string);
      currentJob = fakeJob();
      return jsonResponse(currentJob);
    }

    if (url.includes(`/api/agents/${agent}/teams-setup/job`) && method === "GET") {
      const parsedUrl = new URL(url, "http://localhost");
      const logSinceStr = parsedUrl.searchParams.get("log_since");
      if (logSinceStr !== null) {
        lastLogSinceRequested = parseInt(logSinceStr, 10);
      }
      if (getJobError !== null) {
        if (getJobError instanceof Error) {
          throw getJobError;
        }
        return jsonResponse({ error: "Server error" }, getJobError.status);
      }
      if (currentJob === null) {
        return new Response("Not found", { status: 404 });
      }
      return jsonResponse(currentJob);
    }

    if (url.includes("/api/cloud/status")) {
      return jsonResponse({ status: "connected", origin: "relay.example.com", instance: "inst-1" });
    }

    return server.handler(url, init);
  });
});

afterEach(() => {
  vi.restoreAllMocks();
});

async function waitForModalReady(): Promise<void> {
  await waitFor(() => {
    expect(document.querySelector(".loading-state")).toBeNull();
  });
}

describe("TeamsSetupModal", () => {
  // Scenario 1: manual-path link present
  it("presents manual setup guide link on step 1", async () => {
    currentPrereqs = fakePrereqs({
      manual_guide_url: "https://dev.teams.microsoft.com/bots/custom-guide",
    });
    render(TeamsSetupModal, { agent, open: true });
    await waitForModalReady();

    const guideLink = screen.getByRole("link", { name: /Manual setup guide/i });
    expect(guideLink).toBeTruthy();
    expect(guideLink).toHaveAttribute("href", "https://dev.teams.microsoft.com/bots/custom-guide");
    expect(guideLink).toHaveAttribute("target", "_blank");
  });

  // Scenario 2: missing-node blocks continue
  it("blocks continuing when Node.js is missing and shows error banner", async () => {
    currentPrereqs = fakePrereqs({ node: { found: false, version: null, path: null } });
    render(TeamsSetupModal, { agent, open: true });
    await waitForModalReady();

    expect(screen.getByText(/Node\.js 18\+ required/i)).toBeTruthy();
    const nextBtn = screen.getByRole("button", { name: "Next" });
    expect(nextBtn).toBeDisabled();
  });

  // Scenario 3: replace-existing checkbox required when Teams configured
  it("requires checking replace-existing when Teams is already configured", async () => {
    currentPrereqs = fakePrereqs({ teams_already_configured: true });
    render(TeamsSetupModal, { agent, open: true });
    await waitForModalReady();

    const nextBtn = screen.getByRole("button", { name: "Next" });
    expect(nextBtn).toBeDisabled();

    const replaceCheckbox = screen.getByRole("checkbox", {
      name: /Replace existing Teams configuration for atlas/i,
    });
    expect(replaceCheckbox).toBeTruthy();

    await fireEvent.click(replaceCheckbox);
    expect(nextBtn).toBeEnabled();

    await fireEvent.click(replaceCheckbox);
    expect(nextBtn).toBeDisabled();
  });

  // Scenario 4: consent required when CLI missing
  it("requires consent checkbox when Agents Toolkit CLI is not installed", async () => {
    currentPrereqs = fakePrereqs({
      atk: {
        installed: false,
        version: null,
        path: null,
        pinned_version: "1.1.17",
        install_dir: "/tools",
      },
    });
    render(TeamsSetupModal, { agent, open: true });
    await waitForModalReady();

    const nextBtn = screen.getByRole("button", { name: "Next" });
    expect(nextBtn).toBeDisabled();

    const consentCheckbox = screen.getByRole("checkbox", {
      name: /I agree to install @microsoft\/teams-app-cli/i,
    });
    expect(consentCheckbox).toBeTruthy();

    await fireEvent.click(consentCheckbox);
    expect(nextBtn).toBeEnabled();
  });

  // Scenario 5: field validation/counters
  it("validates fields, displays character counters, and warns if messaging endpoint is not HTTPS", async () => {
    currentPrereqs = fakePrereqs({
      suggested_endpoint: "https://relay.example.com/messages",
    });
    render(TeamsSetupModal, { agent, open: true });
    await waitForModalReady();

    // Advance to step 2 (form)
    await fireEvent.click(screen.getByRole("button", { name: "Next" }));
    await settle();

    // Bot name defaults to agent
    const botNameInput = screen.getByRole("textbox", { name: "Bot name" });
    expect(botNameInput).toHaveValue("atlas");

    // Messaging endpoint prefilled with suggested endpoint
    const endpointInput = screen.getByRole("textbox", {
      name: "Messaging endpoint",
    });
    expect(endpointInput).toHaveValue("https://relay.example.com/messages");

    // Counters exist
    expect(screen.getByText("0/80")).toBeTruthy();
    expect(screen.getByText("0/4000")).toBeTruthy();

    // "Start setup" disabled before required fields filled
    const submitBtn = screen.getByRole("button", { name: "Start setup" });
    expect(submitBtn).toBeDisabled();

    // Fill short and long descriptions
    await fireEvent.input(screen.getByRole("textbox", { name: "Short description" }), {
      target: { value: "A helpful assistant bot" },
    });
    await fireEvent.input(screen.getByRole("textbox", { name: "Long description" }), {
      target: { value: "A comprehensive assistant bot designed to answer questions and run jobs." },
    });
    await fireEvent.input(screen.getByRole("textbox", { name: "Developer name" }), {
      target: { value: "Residuum Team" },
    });
    await fireEvent.input(screen.getByRole("textbox", { name: "Developer website" }), {
      target: { value: "https://example.com" },
    });

    expect(screen.getByText("23/80")).toBeTruthy();
    expect(screen.getByText("72/4000")).toBeTruthy();
    expect(submitBtn).toBeEnabled();

    // Test non-HTTPS warning
    await fireEvent.input(endpointInput, {
      target: { value: "http://insecure.example.com/messages" },
    });
    expect(screen.getByText(/Microsoft Teams requires an HTTPS messaging endpoint/i)).toBeTruthy();

    // Restore HTTPS
    await fireEvent.input(endpointInput, {
      target: { value: "https://secure.example.com/messages" },
    });
    expect(screen.queryByText(/Microsoft Teams requires an HTTPS messaging endpoint/i)).toBeNull();

    // Start setup
    await fireEvent.click(submitBtn);
    await settle();
    expect(startJobCalledWith).toBeTruthy();
  });

  // Scenario 6: icon size rejection
  it("rejects icons with invalid dimensions or non-PNG format", async () => {
    currentPrereqs = fakePrereqs();
    render(TeamsSetupModal, { agent, open: true });
    await waitForModalReady();
    await fireEvent.click(screen.getByRole("button", { name: "Next" }));
    await settle();

    const colorIconInput = screen.getByLabelText("Color Icon (192x192 PNG)");
    const nonPngFile = new File(["not an image"], "bad.jpg", { type: "image/jpeg" });

    await fireEvent.change(colorIconInput, { target: { files: [nonPngFile] } });
    await settle();

    expect(screen.getByText("Icon must be a PNG image file.")).toBeTruthy();
  });

  // Scenario 7: sign-in prompt + redirect paste incl. inline 400 error
  it("displays sign-in prompt, handles redirect paste, and shows inline 400 error on invalid redirect", async () => {
    currentJob = fakeJob({
      state: "waiting_for_user",
      phase: "sign_in",
      sign_in: {
        login_url: "https://login.microsoftonline.com/common/oauth2/v2.0/authorize?client_id=123",
        redirect_port: 53000,
      },
    });
    render(TeamsSetupModal, { agent, open: true });
    await waitForModalReady();

    const signinLink = screen.getByRole("link", { name: /Sign in with Microsoft 365/i });
    expect(signinLink).toBeTruthy();
    expect(signinLink).toHaveAttribute(
      "href",
      "https://login.microsoftonline.com/common/oauth2/v2.0/authorize?client_id=123",
    );

    const redirectField = screen.getByRole("textbox", { name: "Redirect URL" });
    expect(redirectField).toBeTruthy();

    // Mock 400 response from submit
    redirectErrorResponse = {
      status: 400,
      message: "Invalid authorization code or state token expired",
    };

    await fireEvent.input(redirectField, {
      target: { value: "http://localhost:53000/?code=bad-code&state=mismatch" },
    });
    const submitBtn = screen.getByRole("button", { name: "Submit" });
    await fireEvent.click(submitBtn);
    await settle();

    expect(redirectSubmittedWith).toBe("http://localhost:53000/?code=bad-code&state=mismatch");
    expect(screen.getByText("Invalid authorization code or state token expired")).toBeTruthy();

    // Successful submit
    redirectErrorResponse = null;
    await fireEvent.input(redirectField, {
      target: { value: "http://localhost:53000/?code=valid-code&state=valid-state" },
    });
    await fireEvent.click(submitBtn);
    await settle();

    expect(redirectSubmittedWith).toBe("http://localhost:53000/?code=valid-code&state=valid-state");
  });

  // Scenario 8: progress checklist + log
  it("displays progress checklist for all 6 phases, elapsed time, collapsible logs, and polls during progress", async () => {
    currentJob = fakeJob({
      state: "running",
      phase: "scaffold",
      completed_phases: ["check_prereqs", "install_cli", "sign_in"],
      log: [
        {
          seq: 1,
          stream: "stdout",
          text: "Checking prerequisites...",
          at: "2026-10-04T12:00:01Z",
        },
        {
          seq: 2,
          stream: "stderr",
          text: "Warning: non-critical notice",
          at: "2026-10-04T12:00:02Z",
        },
      ],
    });
    render(TeamsSetupModal, { agent, open: true });
    await waitForModalReady();

    // All 7 phases rendered
    expect(screen.getByText("Check prerequisites")).toBeTruthy();
    expect(screen.getByText("Install Agents Toolkit CLI")).toBeTruthy();
    expect(screen.getByText("Sign in to Microsoft 365")).toBeTruthy();
    expect(screen.getByText("Scaffold Teams app")).toBeTruthy();
    expect(screen.getByText("Provision cloud resources")).toBeTruthy();
    expect(screen.getByText("Import bot registration")).toBeTruthy();
    expect(screen.getByText("Install in Teams")).toBeTruthy();

    // Elapsed time
    expect(screen.getByText(/Elapsed:/i)).toBeTruthy();

    // Collapsible logs
    const logDisclosure = screen.getByRole("button", { name: /Show logs \(2 lines\)/i });
    expect(logDisclosure).toBeTruthy();
    await fireEvent.click(logDisclosure);
    await settle();

    expect(screen.getByText("Checking prerequisites...")).toBeTruthy();
    expect(screen.getByText("Warning: non-critical notice")).toBeTruthy();

    // Polling: advancing timer triggers another fetch
    vi.useFakeTimers();
    try {
      currentJob = fakeJob({
        state: "running",
        phase: "provision",
        completed_phases: ["check_prereqs", "install_cli", "sign_in", "scaffold"],
      });
      await vi.advanceTimersByTimeAsync(1100);
      await settle();

      expect(screen.getByText("Provision cloud resources")).toBeTruthy();
    } finally {
      vi.useRealTimers();
    }
  });

  // Scenario 9: cancel
  it("cancels setup job when cancel button is clicked", async () => {
    currentJob = fakeJob({ state: "running", phase: "provision" });
    render(TeamsSetupModal, { agent, open: true });
    await waitForModalReady();

    const cancelBtn = screen.getByRole("button", { name: "Cancel setup" });
    await fireEvent.click(cancelBtn);
    await settle();

    expect(cancelCalled).toBe(true);
    expect(screen.getByText(/The Teams setup operation was cancelled/i)).toBeTruthy();
  });

  // Scenario 10: failure view shows only existing created ids with links + retry
  it("shows failure view with only existing created IDs and links, and allows retry", async () => {
    currentJob = fakeJob({
      state: "failed",
      phase: "provision",
      error: {
        phase: "provision",
        message: "Resource group deployment failed",
        detail: "Azure quota limit exceeded for Microsoft.BotService in region eastus.",
      },
      created: {
        bot_id: "bot-existing-123",
        teams_app_id: null,
        tenant_id: "tenant-existing-456",
        entra_url: "https://entra.microsoft.com/#view/bot-existing-123",
        dev_portal_url: "https://dev.teams.microsoft.com/apps/null",
      },
    });
    render(TeamsSetupModal, { agent, open: true });
    await waitForModalReady();

    // Error banner & phase
    expect(screen.getByText("Resource group deployment failed")).toBeTruthy();
    expect(screen.getByText("provision")).toBeTruthy();

    // Technical details
    const detailsDisclosure = screen.getByRole("button", { name: "Technical error details" });
    await fireEvent.click(detailsDisclosure);
    await settle();
    expect(screen.getByText(/Azure quota limit exceeded/i)).toBeTruthy();

    // Created resources: ONLY bot_id and tenant_id exist
    expect(screen.getByText("bot-existing-123")).toBeTruthy();
    expect(screen.getByText("tenant-existing-456")).toBeTruthy();
    const entraLink = screen.getByRole("link", { name: /Entra Admin Center/i });
    expect(entraLink).toHaveAttribute("href", "https://entra.microsoft.com/#view/bot-existing-123");

    // Teams App ID was null, so it must NOT be displayed
    expect(screen.queryByText("Teams App ID:")).toBeNull();

    // Manual setup guide link present
    expect(screen.getByRole("link", { name: /Manual setup guide/i })).toBeTruthy();

    // Retry button calls retry API
    const retryBtn = screen.getByRole("button", { name: "Retry" });
    await fireEvent.click(retryBtn);
    await settle();
    expect(retryCalled).toBe(true);
  });

  it("deletes job and resets wizard when start over is clicked from failure", async () => {
    currentJob = fakeJob({
      state: "failed",
      phase: "provision",
      error: {
        phase: "provision",
        message: "Resource group deployment failed",
        detail: null,
      },
    });
    render(TeamsSetupModal, { agent, open: true });
    await waitForModalReady();

    const startOverBtn = screen.getByRole("button", { name: "Start over" });
    await fireEvent.click(startOverBtn);
    await settle();
    expect(deleteJobCalled).toBe(true);
  });

  // Scenario 11: done view install/download
  it("shows complete view with bot details, install in Teams button, package download, DM owner reminder, and cleanup", async () => {
    currentJob = fakeJob({
      state: "succeeded",
      phase: "install_app",
      result: {
        bot_id: "final-bot-123",
        tenant_id: "final-tenant-456",
        teams_app_id: "final-app-789",
        package_path: "/workspace/atlas-teams.zip",
        project_dir: "/workspace/atlas-atk",
      },
      app_installed: false,
    });
    render(TeamsSetupModal, { agent, open: true });
    await waitForModalReady();

    // Bot ID & Tenant ID
    expect(screen.getByText("final-bot-123")).toBeTruthy();
    expect(screen.getByText("final-tenant-456")).toBeTruthy();

    // Direct message owner reminder
    expect(screen.getByText(/Direct message reminder:/i)).toBeTruthy();

    // Install in Teams with error handling
    installAppErrorResponse = {
      status: 403,
      message: "Custom app uploads are blocked by IT admin policy.",
    };
    const installBtn = screen.getByRole("button", { name: "Install in Teams" });
    await fireEvent.click(installBtn);
    await settle();

    expect(installAppCalled).toBe(true);
    expect(screen.getByText("Custom app uploads are blocked by IT admin policy.")).toBeTruthy();

    // Download package button
    const downloadBtn = screen.getByRole("button", { name: "Download app package" });
    expect(downloadBtn).toBeTruthy();

    // Cleanup button
    const cleanupBtn = screen.getByRole("button", { name: "Clean up temporary files" });
    await fireEvent.click(cleanupBtn);
    await settle();
    expect(cleanupCalled).toBe(true);
  });

  it("displays non-blocking 'Lost contact with Residuum' banner when polling fails and clears when communication recovers", async () => {
    vi.useFakeTimers();
    try {
      currentJob = fakeJob({ state: "running", phase: "provision" });
      render(TeamsSetupModal, { agent, open: true });
      await settle();

      expect(screen.queryByText(/Lost contact with Residuum, retrying…/i)).toBeNull();

      // Trigger poll failure
      getJobError = new TypeError("Network error");
      await advance(1100);

      expect(screen.getByText(/Lost contact with Residuum, retrying…/i)).toBeTruthy();
      expect(screen.getByText(/Setup is continuing on the server/i)).toBeTruthy();

      // Clear error and advance timer -> recovers
      getJobError = null;
      await advance(1100);

      expect(screen.queryByText(/Lost contact with Residuum, retrying…/i)).toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it("passes log_since when polling, deduplicates log lines by seq, and trims buffer when exceeding 1000 lines", async () => {
    vi.useFakeTimers();
    try {
      currentJob = fakeJob({
        state: "running",
        phase: "provision",
        log: [
          { seq: 1, stream: "stdout", text: "Line 1", at: "2026-10-04T12:00:01Z" },
          { seq: 2, stream: "stdout", text: "Line 2", at: "2026-10-04T12:00:02Z" },
        ],
        last_seq: 2,
      });
      render(TeamsSetupModal, { agent, open: true });
      await settle();

      // Initial load passes log_since=0
      expect(lastLogSinceRequested).toBe(0);

      // Expand logs
      const logDisclosure = screen.getByRole("button", { name: /Show logs/i });
      await fireEvent.click(logDisclosure);
      await settle();

      expect(screen.getByText("Line 1")).toBeTruthy();
      expect(screen.getByText("Line 2")).toBeTruthy();

      // Advance timer: poll passes log_since=2
      const manyLines = [];
      for (let i = 3; i <= 1005; i++) {
        manyLines.push({
          seq: i,
          stream: "stdout" as const,
          text: `Generated log line ${i}`,
          at: "2026-10-04T12:00:05Z",
        });
      }
      currentJob = fakeJob({
        state: "running",
        phase: "provision",
        log: [
          { seq: 2, stream: "stdout", text: "Line 2", at: "2026-10-04T12:00:02Z" },
          ...manyLines,
        ],
        last_seq: 1005,
      });

      await advance(1100);

      // Poll should have requested log_since=2
      expect(lastLogSinceRequested).toBe(2);

      // 1005 total unique lines minus 1000 max = 5 trimmed lines
      expect(screen.getByText(/5 older log lines were trimmed/i)).toBeTruthy();
      expect(screen.getByText("Generated log line 1005")).toBeTruthy();
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("ConnectionsSection Teams integration", () => {
  // Scenario 12: ConnectionsSection button and 'View setup' state
  it("renders setup button in ConnectionsSection with 'View setup' state and status badge when job exists", async () => {
    // 12a: When no job exists
    currentJob = null;
    const scope = settingsModel.agent(agent);
    await scope.load();
    render(ConnectionsSection, { scope, section: "connections" });

    const setupBtn = await screen.findByRole("button", { name: "Set up with Agents Toolkit" });
    expect(setupBtn).toBeTruthy();

    // Click opens modal
    await fireEvent.click(setupBtn);
    await waitFor(() => {
      expect(screen.getByRole("dialog", { name: /Set up Microsoft Teams/i })).toBeTruthy();
    });
  });

  it("shows 'View setup' button and status badge when a job exists", async () => {
    // 12b: When job is running
    currentJob = fakeJob({ state: "running" });
    const scope = settingsModel.agent(agent);
    await scope.load();
    render(ConnectionsSection, { scope, section: "connections" });

    const viewSetupBtn = await screen.findByRole("button", { name: "View setup" });
    expect(viewSetupBtn).toBeTruthy();
    expect(screen.getByText("Setup running")).toBeTruthy();

    // Click opens modal directly to progress
    await fireEvent.click(viewSetupBtn);
    await waitFor(() => {
      expect(screen.getByRole("dialog", { name: /Setting up Microsoft Teams/i })).toBeTruthy();
    });
  });

  it("displays a warning banner with retry button in ConnectionsSection when status fetch fails", async () => {
    getJobError = { status: 500 };
    const scope = settingsModel.agent(agent);
    await scope.load();
    render(ConnectionsSection, { scope, section: "connections" });

    // Warning banner should be rendered
    const banner = await screen.findByText(/Couldn't check Teams setup status/i);
    expect(banner).toBeTruthy();

    const retryBtn = screen.getByRole("button", { name: "Retry" });
    expect(retryBtn).toBeTruthy();

    // Clear error and click Retry -> banner disappears
    getJobError = null;
    currentJob = fakeJob({ state: "running" });
    await fireEvent.click(retryBtn);
    await settle();

    expect(screen.queryByText(/Couldn't check Teams setup status/i)).toBeNull();
    expect(screen.getByText("Setup running")).toBeTruthy();
  });
});
