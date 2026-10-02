import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { settingsModel, type AllScopeModel } from "../../lib/settings-model.svelte";
import type {
  CloudStatusResponse,
  RollbackNoticeResponse,
  UpdateStatusResponse,
} from "../../lib/types";
import { toast } from "../../lib/toast.svelte";
import {
  advance,
  fireEvent,
  jsonResponse,
  mockFetch,
  render,
  screen,
  settle,
  type FetchHandler,
} from "../../test/component";
import { fakeAgentConfig, type FakeAgentConfig } from "../../test/fake-config";
import CloudSection from "./CloudSection.svelte";
import DiagnosticsSection from "./DiagnosticsSection.svelte";
import GeneralSection from "./GeneralSection.svelte";
import LimitsSection from "./LimitsSection.svelte";
import UpdatesSection from "./UpdatesSection.svelte";

// The All agents sections that turn the hub's `config.toml` into forms
// (General, Session limits, Diagnostics), and the two that act at once
// through their own endpoints (Updates, Residuum Cloud).

let server: FakeAgentConfig;
let scope: AllScopeModel;
let count = 0;

/** The hub's config over the fake server, and `extra` answering first. */
function serve(hub: string, extra: FetchHandler = () => new Response("", { status: 599 })): void {
  server = fakeAgentConfig(`all-sections-${String(++count)}`, { hub });
  mockFetch((url, init) => {
    const answer = extra(url, init);
    return answer instanceof Response && answer.status === 599 ? server.handler(url, init) : answer;
  });
}

async function open(): Promise<void> {
  scope = settingsModel.all();
  await scope.reload();
}

afterEach(() => {
  scope.discard();
  for (const id of [...toast.toasts.keys()]) toast.dismiss(id);
});

const patches = (): unknown[] =>
  server.requests
    .filter((request) => request.method === "PATCH")
    .map((request) => JSON.parse(request.body ?? "{}") as unknown);

async function type(input: HTMLElement, value: string): Promise<void> {
  await fireEvent.input(input, { target: { value } });
  await settle();
}

describe("General", () => {
  beforeEach(() => {
    serve('timezone = "UTC"\n\n[gateway]\nbind = "127.0.0.1"\nport = 7700\n');
  });

  it("stages a new timezone with the rest of the install-wide settings", async () => {
    await open();
    render(GeneralSection, { scope, section: "general" });
    expect(screen.getByLabelText("Timezone")).toHaveValue("UTC");

    await fireEvent.change(screen.getByLabelText("Timezone"), {
      target: { value: "Europe/Berlin" },
    });

    expect(scope.dirty).toBe(true);
    expect(scope.configFile.patch).toEqual({ timezone: "Europe/Berlin" });
    expect(patches()).toEqual([]);
  });

  it("flags a saved name that isn't a timezone", async () => {
    serve('timezone = "Mars/Olympus"\n');
    await open();
    render(GeneralSection, { scope, section: "general" });

    expect(screen.getByLabelText("Timezone")).toHaveValue("Mars/Olympus");
    expect(screen.getByText(/doesn't look like a timezone name/)).toBeInTheDocument();
  });

  it("offers this device's timezone when it differs, and takes it", async () => {
    const device = Intl.DateTimeFormat().resolvedOptions().timeZone;
    serve(`timezone = "${device === "Asia/Tokyo" ? "UTC" : "Asia/Tokyo"}"\n`);
    await open();
    render(GeneralSection, { scope, section: "general" });

    await fireEvent.click(
      screen.getByRole("button", { name: `Use this device's timezone (${device})` }),
    );

    expect(screen.getByLabelText("Timezone")).toHaveValue(device);
    expect(
      screen.queryByRole("button", { name: /Use this device's timezone/ }),
    ).not.toBeInTheDocument();
  });

  it("keeps the gateway address under More options and stages its port as a number", async () => {
    await open();
    render(GeneralSection, { scope, section: "general" });
    await fireEvent.click(screen.getByRole("button", { name: "More options" }));

    await type(screen.getByLabelText("Port"), "8080");
    await type(screen.getByLabelText("Bind address"), "0.0.0.0");

    expect(scope.config.gateway_port).toBe("8080");
    expect(scope.configFile.patch).toEqual({ gateway: { port: 8080, bind: "0.0.0.0" } });
  });

  it("shows a problem the server found at the gateway port on its field, with the options open", async () => {
    serve('timezone = "UTC"\n', (url, init) =>
      init?.method === "PATCH" && url === "/api/hub/config/patch"
        ? jsonResponse({
            valid: false,
            diagnostics: [
              {
                severity: "error",
                message: "gateway.port can't be 0",
                location: { kind: "path", path: "gateway.port" },
              },
            ],
          })
        : new Response("", { status: 599 }),
    );
    await open();
    render(GeneralSection, { scope, section: "general" });
    await fireEvent.click(screen.getByRole("button", { name: "More options" }));
    await type(screen.getByLabelText("Port"), "0");

    await scope.save(() => Promise.resolve("keep-mine"));
    await settle();

    expect(screen.getByText("gateway.port can't be 0")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "More options" })).toHaveAttribute(
      "aria-expanded",
      "true",
    );
  });
});

describe("Session limits", () => {
  beforeEach(() => {
    serve(
      'timezone = "UTC"\n\n[background]\nmax_concurrent = 3\nhop_soft_limit = 8\nhop_hard_limit = 32\n',
    );
  });

  it("shows the numbers and stages a change to one", async () => {
    await open();
    render(LimitsSection, { scope, section: "limits" });
    expect(screen.getByLabelText("Turns at once")).toHaveValue(3);
    expect(screen.getByLabelText("Ask for fewer replies after")).toHaveValue(8);
    expect(screen.getByLabelText("Stop delivering after")).toHaveValue(32);

    await type(screen.getByLabelText("Turns at once"), "5");

    expect(scope.configFile.patch).toEqual({ background: { max_concurrent: 5 } });
  });

  it("flags a limit of none, and a note limit at or above the refusal limit", async () => {
    await open();
    render(LimitsSection, { scope, section: "limits" });
    expect(screen.queryByRole("status")).not.toBeInTheDocument();

    await type(screen.getByLabelText("Turns at once"), "0");
    expect(screen.getByText(/can never run a turn/)).toBeInTheDocument();

    await type(screen.getByLabelText("Ask for fewer replies after"), "40");
    expect(screen.getByText(/the first one's note never appears/)).toBeInTheDocument();
    expect(scope.dirty).toBe(true);
  });
});

describe("Diagnostics", () => {
  beforeEach(() => {
    serve('timezone = "UTC"\n');
  });

  it("stages the redaction and error-report switches", async () => {
    await open();
    render(DiagnosticsSection, { scope, section: "diagnostics" });
    const redact = screen.getByRole("switch", { name: "Redact content in trace exports" });
    const report = screen.getByRole("switch", { name: "Report errors automatically" });
    expect(redact).toBeChecked();
    expect(report).not.toBeChecked();

    await fireEvent.click(redact);
    await fireEvent.click(report);

    expect(scope.configFile.patch).toEqual({
      tracing: { sanitize_content: false, auto_error_reporting: true },
    });
  });

  it("stages the log detail, and its default writes nothing", async () => {
    await open();
    render(DiagnosticsSection, { scope, section: "diagnostics" });
    const detail = screen.getByLabelText("Log detail");
    expect(detail).toHaveValue("");

    await fireEvent.change(detail, { target: { value: "trace" } });
    expect(scope.configFile.patch).toEqual({ tracing: { log_level: "trace" } });

    await fireEvent.change(detail, { target: { value: "" } });
    expect(scope.dirty).toBe(false);
  });
});

// ── Updates ────────────────────────────────────────────────────────────

const RESTART_POLL_MS = 1500;
const RESTART_TIMEOUT_MS = 90_000;

function updateStatus(overrides: Partial<UpdateStatusResponse> = {}): UpdateStatusResponse {
  return {
    current: "1.0.0",
    latest: "1.1.0",
    update_available: true,
    last_checked: "2026-09-26T14:00:00.000Z",
    checking: false,
    rollback_notice: null,
    unverified_update: null,
    ...overrides,
  };
}

function rollbackNotice(overrides: Partial<RollbackNoticeResponse> = {}): RollbackNoticeResponse {
  return {
    attempted_version: "1.2.0",
    reason: "The new version never became ready",
    at: "2026-09-26T14:30:00.000Z",
    ...overrides,
  };
}

describe("Updates", () => {
  let gatewayUp = true;
  let status = updateStatus();
  let checkAnswer: Response | null = null;
  let applyAnswer: Response | null = null;

  beforeEach(async () => {
    vi.useFakeTimers({ now: new Date("2026-09-26T15:00:00Z") });
    gatewayUp = true;
    status = updateStatus();
    checkAnswer = null;
    applyAnswer = null;
    serve('timezone = "UTC"\n', (url, init) => {
      const method = init?.method ?? "GET";
      if (url === "/api/hub/update/check" && method === "POST") {
        return checkAnswer ?? jsonResponse(status);
      }
      if (url === "/api/hub/update/apply" && method === "POST") {
        return applyAnswer ?? jsonResponse(status);
      }
      if (url === "/api/hub/update/status" && method === "GET") {
        return gatewayUp ? jsonResponse(status) : Promise.reject(new TypeError("Failed to fetch"));
      }
      return new Response("", { status: 599 });
    });
    await open();
  });

  async function openAndInstall(): Promise<void> {
    render(UpdatesSection, { scope, section: "updates" });
    await settle();
    gatewayUp = false;
    await fireEvent.click(screen.getByRole("button", { name: "Update and restart" }));
    await settle();
  }

  it("shows the running and latest versions, when it was checked, and that one is available", async () => {
    render(UpdatesSection, { scope, section: "updates" });
    await settle();

    expect(screen.getByText("Update available")).toBeInTheDocument();
    expect(screen.getByText("1.0.0")).toBeInTheDocument();
    expect(screen.getByText("1.1.0")).toBeInTheDocument();
    expect(screen.getByText("1h ago")).toBeInTheDocument();
  });

  it("says it is up to date once a check finds nothing newer", async () => {
    status = updateStatus({ latest: null, update_available: false, last_checked: null });
    render(UpdatesSection, { scope, section: "updates" });
    await settle();
    expect(screen.getByText("Not checked yet")).toBeInTheDocument();

    status = updateStatus({ latest: "1.0.0", update_available: false });
    await fireEvent.click(screen.getByRole("button", { name: "Check for updates" }));
    await settle();

    expect(screen.getByText("Up to date")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Update and restart" })).not.toBeInTheDocument();
  });

  it("tells the user a check that failed, in plain language", async () => {
    checkAnswer = new Response("", { status: 502 });
    render(UpdatesSection, { scope, section: "updates" });
    await settle();

    await fireEvent.click(screen.getByRole("button", { name: "Check for updates" }));
    await settle();

    expect(screen.getByRole("alert")).toHaveTextContent(/Couldn't check for updates\./);
  });

  it("offers Try again when the status can't be read at all", async () => {
    gatewayUp = false;
    render(UpdatesSection, { scope, section: "updates" });
    await settle();
    expect(screen.getByRole("alert")).toHaveTextContent(/Couldn't read the update status\./);

    gatewayUp = true;
    await fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    await settle();

    expect(screen.getByText("Update available")).toBeInTheDocument();
  });

  it("shows elapsed time while the restarted gateway is not answering", async () => {
    await openAndInstall();

    expect(screen.getByText(/Restarting Residuum… \(0s\)/)).toBeInTheDocument();
    expect(screen.getByText(/within 90s/)).toBeInTheDocument();

    await advance(RESTART_POLL_MS);

    expect(screen.getByText(/Restarting Residuum… \(1s\)/)).toBeInTheDocument();
    expect(screen.queryByText(/Updated to/)).not.toBeInTheDocument();
  });

  it("shows the new version once the gateway answers with it", async () => {
    await openAndInstall();

    status = updateStatus({ current: "1.1.0", latest: "1.1.0", update_available: false });
    gatewayUp = true;
    await advance(RESTART_POLL_MS);

    expect(screen.getByText("Updated to 1.1.0.")).toBeInTheDocument();
    expect(screen.queryByText(/Restarting/)).not.toBeInTheDocument();
  });

  it("keeps waiting while the old process still answers with the old version", async () => {
    await openAndInstall();

    status = updateStatus({ update_available: false });
    gatewayUp = true;
    await advance(RESTART_POLL_MS);

    expect(screen.getByText(/Restarting Residuum…/)).toBeInTheDocument();
    expect(screen.queryByText(/Updated to/)).not.toBeInTheDocument();
  });

  it("shows the rollback reason when the status reports one during the restart", async () => {
    await openAndInstall();

    status = updateStatus({
      latest: "1.2.0",
      update_available: false,
      rollback_notice: rollbackNotice(),
    });
    gatewayUp = true;
    await advance(RESTART_POLL_MS);

    expect(screen.getByText("Update to 1.2.0 failed and was rolled back.")).toBeInTheDocument();
    expect(
      screen.getByText("The new version never became ready. Now running 1.0.0."),
    ).toBeInTheDocument();
  });

  it("shows a rollback that was already on the status when the section opened", async () => {
    status = updateStatus({
      latest: "1.2.0",
      update_available: false,
      rollback_notice: rollbackNotice({ reason: "The new version exited while starting" }),
    });

    render(UpdatesSection, { scope, section: "updates" });
    await settle();

    expect(screen.getByText("Update to 1.2.0 failed and was rolled back.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Update and restart" })).not.toBeInTheDocument();
  });

  it("says it is still waiting once the health window passes with no response", async () => {
    await openAndInstall();

    await advance(RESTART_TIMEOUT_MS + RESTART_POLL_MS);

    expect(screen.getByText("Still waiting for Residuum to come back.")).toBeInTheDocument();
    expect(screen.getByText("residuum logs")).toBeInTheDocument();
    expect(screen.queryByText(/Restarting/)).not.toBeInTheDocument();
  });

  it("warns that the installed update couldn't be verified", async () => {
    status = updateStatus({
      update_available: false,
      unverified_update: { version: "1.0.0", at: "2026-09-26T13:00:00.000Z" },
    });
    render(UpdatesSection, { scope, section: "updates" });
    await settle();

    expect(screen.getByText("This update couldn't be verified.")).toBeInTheDocument();
  });
});

// ── Residuum Cloud ─────────────────────────────────────────────────────

function cloudStatus(overrides: Partial<CloudStatusResponse> = {}): CloudStatusResponse {
  return {
    status: "disconnected",
    user_id: null,
    has_token: false,
    enabled: false,
    viewed_via_tunnel: false,
    ...overrides,
  };
}

describe("Residuum Cloud", () => {
  let status = cloudStatus();
  let statusReadable = true;
  let disconnectAnswer: Response | null = null;
  let stored: { name: string; value: string }[] = [];

  beforeEach(async () => {
    status = cloudStatus();
    statusReadable = true;
    disconnectAnswer = null;
    stored = [];
    serve('timezone = "UTC"\n\n[gateway]\nport = 7701\n', (url, init) => {
      const method = init?.method ?? "GET";
      if (url === "/api/hub/cloud/status") {
        return statusReadable ? jsonResponse(status) : new Response("", { status: 502 });
      }
      if (url === "/api/hub/cloud/disconnect" && method === "POST") {
        if (disconnectAnswer !== null) return disconnectAnswer;
        status = cloudStatus({ has_token: true, enabled: false });
        return jsonResponse({ ok: true });
      }
      if (url === "/api/hub/secrets" && method === "POST") {
        const body = typeof init?.body === "string" ? init.body : "{}";
        stored.push(JSON.parse(body) as { name: string; value: string });
        return jsonResponse({ reference: "secret:cloud_token" });
      }
      return new Response("", { status: 599 });
    });
    await open();
  });

  /** The section, once it has read the status. The answers arrive over more than one tick, so a test waits for what it expects. */
  async function view(): Promise<void> {
    render(CloudSection, { scope, section: "cloud" });
    await vi.waitFor(() => {
      expect(document.querySelector(".ui-skeleton")).toBeNull();
    });
  }

  const button = (name: string): Promise<HTMLElement> => screen.findByRole("button", { name });

  it("offers to sign in when there is no account, at the relay the config names", async () => {
    const opened = vi.spyOn(window, "open").mockReturnValue(null);
    await view();
    expect(screen.getByText("Not connected")).toBeInTheDocument();
    expect(screen.getByText(/Opens agent-residuum.com in a new tab/)).toBeInTheDocument();

    await fireEvent.click(await button("Connect to Residuum Cloud"));
    expect(opened).toHaveBeenLastCalledWith(
      "https://agent-residuum.com/connect?port=7701",
      "_blank",
      "noopener",
    );

    await type(screen.getByLabelText("Relay URL"), "ws://127.0.0.1:8080/tunnel/register");
    expect(screen.getByText(/Opens 127.0.0.1:8080 in a new tab/)).toBeInTheDocument();
    expect(screen.getByText(/Save your relay change first/)).toBeInTheDocument();
    await fireEvent.click(await button("Connect to Residuum Cloud"));
    expect(opened).toHaveBeenLastCalledWith(
      "http://127.0.0.1:8080/connect?port=7701",
      "_blank",
      "noopener",
    );
  });

  it("has nowhere to sign in while the relay address isn't a ws address", async () => {
    await view();
    await type(screen.getByLabelText("Relay URL"), "relay.example.com");

    expect(await button("Connect to Residuum Cloud")).toBeDisabled();
    expect(screen.getByText(/isn't a ws:\/\/ or wss:\/\/ address/)).toBeInTheDocument();
  });

  it("keeps a pasted token as a secret and switches the tunnel on with it", async () => {
    await view();
    await fireEvent.click(await button("Use a token instead"));
    await type(screen.getByLabelText("Tunnel token"), "rst_abc");
    status = cloudStatus({ status: "connected", has_token: true, enabled: true, user_id: "bear" });

    await fireEvent.click(await button("Connect with token"));

    expect(await screen.findByText("bear")).toBeInTheDocument();
    expect(screen.getByText("Connected")).toBeInTheDocument();
    expect(stored).toEqual([{ name: "cloud_token", value: "rst_abc" }]);
    expect(patches()).toEqual([{ cloud: { enabled: true, token: "secret:cloud_token" } }]);
  });

  it("disconnects at once and then offers Reconnect and Remove account", async () => {
    status = cloudStatus({ status: "connected", has_token: true, enabled: true, user_id: "bear" });
    await view();

    await fireEvent.click(await button("Disconnect"));

    expect(await screen.findByText("Disconnected")).toBeInTheDocument();
    expect(await button("Reconnect")).toBeInTheDocument();
    expect(scope.dirty).toBe(false);
  });

  it("says why a disconnect that was refused didn't happen", async () => {
    status = cloudStatus({ status: "connected", has_token: true, enabled: true });
    disconnectAnswer = new Response("Shutting down or disconnecting can't be done remotely.", {
      status: 403,
    });
    await view();

    await fireEvent.click(await button("Disconnect"));

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Couldn't disconnect from Residuum Cloud. It can't be done remotely",
    );
    expect(screen.getByText("Connected")).toBeInTheDocument();
  });

  it("hides Disconnect from someone viewing Residuum through the tunnel, and says why", async () => {
    status = cloudStatus({
      status: "connected",
      has_token: true,
      enabled: true,
      viewed_via_tunnel: true,
    });
    await view();

    expect(screen.queryByRole("button", { name: "Disconnect" })).not.toBeInTheDocument();
    expect(screen.getByText(/can't be disconnected from here/)).toBeInTheDocument();
  });

  it("offers Cancel while connecting, and not through the tunnel", async () => {
    status = cloudStatus({ status: "connecting", has_token: true, enabled: true });
    await view();
    expect(screen.getByText("Connecting…")).toBeInTheDocument();
    expect(await button("Cancel")).toBeInTheDocument();

    status = cloudStatus({
      status: "connecting",
      has_token: true,
      enabled: true,
      viewed_via_tunnel: true,
    });
    await fireEvent.focus(window);

    expect(await screen.findByText(/can't be cancelled from here/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Cancel" })).not.toBeInTheDocument();
  });

  it("reconnects by switching the tunnel back on in the hub's config, not by staging it", async () => {
    status = cloudStatus({ has_token: true, enabled: false });
    await view();

    status = cloudStatus({ status: "connecting", has_token: true, enabled: true });
    await fireEvent.click(await button("Reconnect"));

    expect(await screen.findByText("Connecting…")).toBeInTheDocument();
    expect(patches()).toEqual([{ cloud: { enabled: true } }]);
    expect(scope.dirty).toBe(false);
  });

  it("stages removing the account, and Keep account takes it back", async () => {
    serve('timezone = "UTC"\n\n[cloud]\nenabled = false\ntoken = "secret:cloud_token"\n', (url) =>
      url === "/api/hub/cloud/status"
        ? jsonResponse(cloudStatus({ has_token: true, enabled: false }))
        : new Response("", { status: 599 }),
    );
    await open();
    await view();

    await fireEvent.click(await button("Remove account"));
    expect(scope.configFile.patch).toEqual({ cloud: { token: null } });
    expect(
      await screen.findByText("The account is removed when you save changes."),
    ).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Reconnect" })).not.toBeInTheDocument();

    await fireEvent.click(await button("Keep account"));
    expect(scope.dirty).toBe(false);
    expect(await button("Reconnect")).toBeInTheDocument();
  });

  it("shows a status it can't read with Try again, instead of an empty state", async () => {
    statusReadable = false;
    render(CloudSection, { scope, section: "cloud" });
    expect(await screen.findByRole("alert")).toHaveTextContent(/Couldn't read the Cloud status\./);

    statusReadable = true;
    await fireEvent.click(await button("Try again"));

    expect(await screen.findByText("Not connected")).toBeInTheDocument();
  });

  it("looks again on its own while connecting, until the tunnel is up", async () => {
    vi.useFakeTimers();
    status = cloudStatus({ status: "connecting", has_token: true, enabled: true });
    render(CloudSection, { scope, section: "cloud" });
    await settle();
    expect(screen.getByText("Connecting…")).toBeInTheDocument();

    status = cloudStatus({ status: "connected", has_token: true, enabled: true, user_id: "bear" });
    await advance(3100);

    expect(screen.getByText("Connected")).toBeInTheDocument();
  });
});
