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
import { ALL_SCOPE, type SectionId } from "./lib/settings-sections";

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
  scope: "agent" | "hub",
  section: SectionId,
  onSelectSection = vi.fn<(section: SectionId) => void>(),
): typeof onSelectSection {
  render(Settings, {
    scope: scope === "agent" ? "scout" : ALL_SCOPE,
    section,
    onSelectSection,
    onClose: () => {},
  });
  return onSelectSection;
}

const AGENT_LABELS = [
  "Model",
  "Connections",
  "Tools & skills",
  "Memory",
  "Schedule",
  "Runtime",
  "Tool servers",
];
const HUB_LABELS = [
  "General",
  "Notifications",
  "Residuum Cloud",
  "Saved keys",
  "Updates",
  "Session limits",
  "Diagnostics",
];

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
  it("lists the registry's install-wide sections under All agents", async () => {
    mount("hub", "general");
    await settle();
    expect(screen.getByText("All agents")).toBeTruthy();
    expect(screen.getByText("Applies to every agent")).toBeTruthy();
    for (const label of HUB_LABELS) expect(navButton(label), label).toBeTruthy();
    for (const label of AGENT_LABELS) expect(navButton(label), label).toBeUndefined();
  });

  it("lists the registry's agent sections under an agent", async () => {
    mount("agent", "runtime");
    await settle();
    expect(screen.getByText("scout settings")).toBeTruthy();
    expect(screen.getByText("Only affects scout")).toBeTruthy();
    for (const label of AGENT_LABELS) expect(navButton(label), label).toBeTruthy();
    for (const label of HUB_LABELS) expect(navButton(label), label).toBeUndefined();
  });

  it("selects a section by its registry id", async () => {
    const onSelect = mount("hub", "general");
    await settle();
    await fireEvent.click(navButton("Saved keys") as HTMLElement);
    expect(onSelect).toHaveBeenCalledWith("keys");
  });

  it("shows both of a section's panels when it gathers two old pages", async () => {
    mount("agent", "connections");
    await settle();
    expect(await screen.findByText("Discord")).toBeTruthy();
    expect(screen.getByRole("button", { name: /add webhook/i })).toBeTruthy();
  });

  it("opens the raw editors as the Raw config section, without a form view toggle", async () => {
    mount("agent", "raw");
    await settle();
    expect(await screen.findByRole("button", { name: "providers.toml" })).toBeTruthy();
    expect(screen.queryByRole("group", { name: "Settings view" })).toBeNull();
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
    mount("hub", "keys");
    expect(await screen.findByText("openai")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Remove openai" })).toBeTruthy();
  });

  it("splits the A2A page: the listener under the hub, visibility under the agent", async () => {
    mount("hub", "listener");
    expect(await screen.findByText("Listener")).toBeTruthy();
    expect(screen.getByText("Caller keys")).toBeTruthy();
    expect(screen.queryByText("Remote agents")).toBeNull();
  });
});
