import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  advance,
  fireEvent,
  jsonResponse,
  mockFetch,
  render,
  screen,
  settle,
  stubWebSocket,
} from "./test/component";
import Settings from "./Settings.svelte";
import { invalidate } from "./lib/cache";
import type { LegacyScope, LegacySection } from "./lib/legacy-settings-sections";

interface Call {
  method: string;
  url: string;
  body: string | undefined;
}

let calls: Call[] = [];

function textResponse(body: string): Response {
  return new Response(body, { status: 200, headers: { "Content-Type": "text/plain" } });
}

function serve(): void {
  calls = [];
  mockFetch((url, init) => {
    const method = init?.method ?? "GET";
    calls.push({ method, url, body: typeof init?.body === "string" ? init.body : undefined });
    if (method === "PATCH") return jsonResponse({ valid: true });
    if (url.endsWith("/config/raw")) return textResponse('timezone = "UTC"\n');
    if (url.endsWith("/providers/raw")) return textResponse("");
    if (url.endsWith("/mcp/raw")) return textResponse("{}");
    if (url === "/api/hub/secrets") return jsonResponse({ names: ["openai"] });
    if (url === "/api/hub/a2a/keys") return jsonResponse({ keys: [] });
    return jsonResponse({}, 404);
  });
}

function mount(
  scope: LegacyScope,
  section: LegacySection,
  onSelectSection = vi.fn<(section: LegacySection) => void>(),
): typeof onSelectSection {
  render(Settings, {
    scope,
    agent: scope === "agent" ? "scout" : null,
    section,
    onSelectSection,
    onClose: () => {},
  });
  return onSelectSection;
}

const navButton = (name: string): HTMLElement | undefined =>
  screen
    .queryAllByRole("button", { name })
    .find((b) => b.classList.contains("settings-sidebar-btn"));

beforeEach(() => {
  stubWebSocket();
  invalidate("");
  serve();
});

describe("Settings scope split", () => {
  it("shows only the hub's sections under hub settings", async () => {
    mount("hub", "general");
    await settle();
    expect(screen.getByText("Hub settings")).toBeTruthy();
    for (const label of [
      "Gateway & timezone",
      "Cloud",
      "A2A listener & keys",
      "Session budget",
      "Tracing",
      "Update",
      "Secrets",
      "Agent keys",
      "History",
    ]) {
      expect(navButton(label), label).toBeTruthy();
    }
    for (const label of [
      "Runtime",
      "Models & providers",
      "Adapters & channels",
      "Pulses & sessions",
      "Memory",
      "Skills & tools",
      "MCP",
      "A2A visibility & client",
      "Webhooks",
    ]) {
      expect(navButton(label), label).toBeUndefined();
    }
  });

  it("shows only the agent's sections under agent settings", async () => {
    mount("agent", "runtime");
    await settle();
    expect(screen.getByText("scout settings")).toBeTruthy();
    for (const label of [
      "Runtime",
      "Models & providers",
      "Adapters & channels",
      "Pulses & sessions",
      "Memory",
      "Skills & tools",
      "MCP",
      "A2A visibility & client",
      "Webhooks",
      "History",
    ]) {
      expect(navButton(label), label).toBeTruthy();
    }
    for (const label of [
      "Gateway & timezone",
      "Cloud",
      "A2A listener & keys",
      "Session budget",
      "Tracing",
      "Update",
      "Secrets",
      "Agent keys",
    ]) {
      expect(navButton(label), label).toBeUndefined();
    }
  });

  it("selects a section by id", async () => {
    const onSelect = mount("hub", "general");
    await settle();
    await fireEvent.click(navButton("Secrets") as HTMLElement);
    expect(onSelect).toHaveBeenCalledWith("secrets");
  });

  it("reads and writes only hub files on the hub's pages", async () => {
    vi.useFakeTimers();
    mount("hub", "general");
    await settle();
    expect(calls.every((c) => c.url.startsWith("/api/hub/"))).toBe(true);

    await fireEvent.input(screen.getByLabelText("Timezone", { selector: "input" }), {
      target: { value: "America/New_York" },
    });
    await advance(1000);
    const writes = calls.filter((c) => c.method !== "GET");
    expect(writes.map((c) => c.url)).toEqual(["/api/hub/config/patch"]);
    expect(JSON.parse(writes[0]?.body ?? "{}")).toEqual({ timezone: "America/New_York" });
  });

  it("writes only the agent's files on the agent's pages, leaving the hub config alone", async () => {
    vi.useFakeTimers();
    mount("agent", "runtime");
    await settle();
    expect(
      calls.filter((c) => c.url.startsWith("/api/hub/")).every((c) => c.method === "GET"),
    ).toBe(true);

    await fireEvent.input(screen.getByLabelText("Timeout (seconds)"), {
      target: { value: "60" },
    });
    await advance(1000);
    const writes = calls.filter((c) => c.method !== "GET");
    expect(writes.some((c) => c.url.startsWith("/api/hub/"))).toBe(false);
    expect(writes.some((c) => c.url === "/api/agents/scout/config/patch")).toBe(true);
  });

  it("puts the timezone and gateway under the hub and not under the agent's runtime", async () => {
    mount("agent", "runtime");
    await settle();
    expect(screen.queryByLabelText("Timezone")).toBeNull();
    expect(screen.queryByText("Gateway")).toBeNull();
  });

  it("lists secrets by name under hub settings", async () => {
    mount("hub", "secrets");
    expect(await screen.findByText("openai")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Remove openai" })).toBeTruthy();
  });

  it("splits the A2A page: the listener under the hub, visibility under the agent", async () => {
    mount("hub", "a2a");
    expect(await screen.findByText("Listener")).toBeTruthy();
    expect(screen.getByText("Caller keys")).toBeTruthy();
    expect(screen.queryByText("Remote agents")).toBeNull();
  });
});
