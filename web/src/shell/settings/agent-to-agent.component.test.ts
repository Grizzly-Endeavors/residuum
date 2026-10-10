import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentState, AgentSummary } from "../../lib/hub-types";
import { hub } from "../../lib/hub.svelte";
import { notifications } from "../../lib/notifications.svelte";
import { router } from "../../lib/router.svelte";
import { settingsModel } from "../../lib/settings-model.svelte";
import { toast } from "../../lib/toast.svelte";
import type { A2aStatusResponse } from "../../lib/types";
import {
  fireEvent,
  jsonResponse,
  mockFetch,
  render,
  screen,
  settle,
  stubWebSocket,
} from "../../test/component";
import { fakeAgentConfig, type FakeAgentConfig } from "../../test/fake-config";
import { snapshot } from "../../test/hub-frames";
import SettingsModal from "../SettingsModal.svelte";
import { waitFor } from "../../test/wait";

let agent = "";
let server: FakeAgentConfig;
let count = 0;
let a2aJson = "";
let patchFails = false;

function summary(state: AgentState, visibility: "public" | "private"): AgentSummary {
  return {
    name: agent,
    display_name: agent,
    state,
    last_error: null,
    autostart: true,
    role: null,
    a2a_visibility: visibility,
    teams_configured: false,
  };
}

const status = (): A2aStatusResponse => ({
  enabled: true,
  port: 7702,
  visibility: "private",
  public_url: null,
  local_url: `http://127.0.0.1:7702/agents/${agent}`,
  relay_access: false,
  relay_access_note: "Reachable locally.",
  listener_running: false,
  card_error: "agent-card.json has no name",
});

const requested = (): string[] =>
  server.requests.map((request) => `${request.method} ${request.url}`);
const saveBar = (): HTMLElement | null => screen.queryByRole("region", { name: "Unsaved changes" });

async function open(
  state: AgentState,
  visibility: "public" | "private" = "private",
): Promise<void> {
  hub.handleFrame(snapshot([summary(state, visibility)]));
  render(SettingsModal);
  await router.openSettings({ scope: agent, section: "a2a" });
  await settle();
  await waitFor(() => {
    expect(screen.getByRole("heading", { name: "Agent-to-agent" })).toBeTruthy();
  });
}

beforeEach(() => {
  stubWebSocket();
  vi.stubGlobal("matchMedia", (media: string) => ({
    media,
    matches: false,
    addEventListener: () => {},
    removeEventListener: () => {},
  }));
  for (const id of [...toast.toasts.keys()]) toast.dismiss(id);
  agent = `a2a-${String(++count)}`;
  a2aJson = JSON.stringify({ agents: { desk: { url: "https://desk.example/a2a" } } });
  patchFails = false;
  server = fakeAgentConfig(agent, { hub: "[a2a]\nenabled = false\n" });
  mockFetch((url, init) => {
    const method = init?.method ?? "GET";
    const sent = typeof init?.body === "string" ? init.body : "";
    const base = `/api/agents/${agent}/a2a`;
    if (url === `/api/hub/agents/${agent}` && method === "PATCH") {
      server.requests.push({ method, url, body: sent });
      if (patchFails) return jsonResponse({ error: "config.toml is read-only" }, 500);
      const body = JSON.parse(sent) as { a2a_visibility: "public" | "private" };
      return jsonResponse(summary("running", body.a2a_visibility));
    }
    if (url.startsWith(base)) server.requests.push({ method, url, body: undefined });
    if (url === `${base}/agents/raw` && method === "GET") return new Response(a2aJson);
    if (url === `${base}/agents/raw` && method === "PUT") {
      a2aJson = sent;
      return jsonResponse({ valid: true });
    }
    if (url === `${base}/status`) return jsonResponse(status());
    if (url === `${base}/card`)
      return jsonResponse({ name: "Desk helper", description: "", skills: [] });
    if (url === `${base}/agents`) {
      return jsonResponse([
        {
          name: "desk",
          url: "https://desk.example/a2a",
          source: "config",
          status: "error",
          error: "connection refused",
          card: null,
        },
      ]);
    }
    return server.handler(url, init);
  });
});

afterEach(async () => {
  await router.replacePlace({ kind: "home" });
  for (const scope of settingsModel.stagedScopes) scope.discard();
  notifications.clear();
});

describe("the Agent-to-agent section", () => {
  it("applies visibility at once through the hub, with no save bar", async () => {
    await open("running");
    await fireEvent.click(screen.getByRole("radio", { name: "Public" }));
    await waitFor(() => {
      expect(requested()).toContain(`PATCH /api/hub/agents/${agent}`);
    });
    await waitFor(() => {
      expect([...toast.toasts.values()].map((shown) => shown.message)).toContain(
        `${agent} is public: anyone with its address can see what it does.`,
      );
    });
    expect(hub.agent(agent)?.a2a_visibility).toBe("public");
    expect(screen.getByRole("radio", { name: "Public" })).toHaveAttribute("aria-checked", "true");
    expect(saveBar()).toBeNull();
    expect(requested().some((line) => line.includes("/config/patch"))).toBe(false);
  });

  it("goes back to the hub's visibility when the change fails", async () => {
    patchFails = true;
    await open("running");
    await fireEvent.click(screen.getByRole("radio", { name: "Public" }));
    expect(screen.getByRole("radio", { name: "Public" })).toHaveAttribute("aria-checked", "true");
    await waitFor(() => {
      expect(notifications.history.map((shown) => shown.message)).toContainEqual(
        expect.stringContaining(`Couldn't change the A2A visibility of ${agent}.`),
      );
    });
    await waitFor(() => {
      expect(screen.getByRole("radio", { name: "Private" })).toHaveAttribute(
        "aria-checked",
        "true",
      );
    });
    expect(hub.agent(agent)?.a2a_visibility).toBe("private");
  });

  it("shows a running agent's status, address, reachability and card, and the install's listener read-only", async () => {
    await open("running");
    await waitFor(() => {
      expect(screen.getByText(`http://127.0.0.1:7702/agents/${agent}`)).toBeTruthy();
    });
    expect(screen.getByText(/Nothing is answering on port 7702 right now/)).toBeTruthy();
    expect(screen.getByText("agent-card.json has no name")).toBeTruthy();
    expect(screen.getByText(/listener is off, so nothing outside it can reach/)).toBeTruthy();
    await waitFor(() => {
      expect(screen.getByText("Can't reach it")).toBeTruthy();
    });
    expect(screen.getByText("connection refused")).toBeTruthy();
    await waitFor(() => {
      expect(screen.getByText("Desk helper")).toBeTruthy();
    });
  });

  it("asks a stopped agent to start for its status, card and reachability, and keeps its remote agents editable", async () => {
    await open("stopped");
    for (const subject of ["its status and address", "its card", "whether they can be reached"]) {
      expect(screen.getByText(`Start ${agent} to see ${subject}.`)).toBeTruthy();
    }
    await waitFor(() => {
      expect(screen.getByText("https://desk.example/a2a")).toBeTruthy();
    });

    await fireEvent.click(screen.getByRole("button", { name: "Edit a2a.json" }));
    await waitFor(() => {
      expect(screen.getByLabelText("Contents of a2a.json")).toHaveValue(a2aJson);
    });
    const edited = JSON.stringify({ agents: { laptop: { url: "https://laptop.example/a2a" } } });
    await fireEvent.input(screen.getByLabelText("Contents of a2a.json"), {
      target: { value: edited },
    });
    await fireEvent.submit(screen.getByRole("form", { name: "Edit a2a.json" }));
    await waitFor(() => {
      expect(screen.getByText("https://laptop.example/a2a")).toBeTruthy();
    });
    expect(a2aJson).toBe(edited);
    expect(requested()).not.toContain(`GET /api/agents/${agent}/a2a/status`);
    expect(requested()).not.toContain(`GET /api/agents/${agent}/a2a/agents`);
  });
});
