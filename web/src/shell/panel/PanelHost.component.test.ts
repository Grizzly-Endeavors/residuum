import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { hub } from "../../lib/hub.svelte";
import type { AgentState, AgentSummary } from "../../lib/hub-types";
import { router } from "../../lib/router.svelte";
import { INBOX, locationAt, type AppLocation, type Panel } from "../../lib/routes";
import { toast } from "../../lib/toast.svelte";
import { setViewedAgent } from "../../lib/viewed-agent";
import { jsonResponse, mockFetch, render, screen, settle } from "../../test/component";
import { FakeWebSocket } from "../../test/fake-websocket";
import { snapshot } from "../../test/hub-frames";
import { waitFor } from "../../test/wait";
import { WIDE_QUERY } from "../../styles/breakpoints";
import PanelHost from "./PanelHost.svelte";

let workspaces: Record<string, Record<string, string>> = {};
let requested: string[] = [];
let saved: { agent: string; path: string; content: string }[] = [];

function serve(): void {
  mockFetch((url, init) => {
    const parsed = new URL(url, "http://localhost");
    requested.push(parsed.pathname);
    const agent = /\/api\/agents\/([^/]+)\//.exec(parsed.pathname)?.[1] ?? "";
    if (parsed.pathname.endsWith("/workspace/validate")) return jsonResponse({ diagnostics: [] });
    if (parsed.pathname.endsWith("/workspace/file")) {
      if (init?.method === "PUT") {
        const sent = JSON.parse(typeof init.body === "string" ? init.body : "{}") as {
          path: string;
          content: string;
        };
        saved.push({ agent, path: sent.path, content: sent.content });
        (workspaces[agent] ??= {})[sent.path] = sent.content;
        return jsonResponse({ saved: true, version: "v2", diagnostics: [] });
      }
      const content = workspaces[agent]?.[parsed.searchParams.get("path") ?? ""];
      if (content === undefined) return new Response("path not found", { status: 404 });
      return new Response(content, { headers: { ETag: "v1" } });
    }
    return new Promise<Response>(() => {});
  });
}

function summary(name: string, state: AgentState): AgentSummary {
  return {
    name,
    display_name: name,
    state,
    last_error: null,
    autostart: false,
    role: null,
    a2a_visibility: "private",
    teams_configured: false,
  };
}

function socketOf(agent: string): FakeWebSocket | undefined {
  return FakeWebSocket.sockets.find((s) => new URL(s.url).pathname === `/api/agents/${agent}/ws`);
}

async function go(place: AppLocation["place"], panel: Panel | null): Promise<void> {
  router.location = { ...locationAt(place), panel };
  await settle();
}

const file = (agent: string | null, path = "report.md"): Panel => ({ kind: "file", path, agent });

async function openEditor(name = "report.md"): Promise<HTMLTextAreaElement> {
  await userEvent.click(await screen.findByRole("button", { name: "Edit" }));
  return screen.findByRole("textbox", { name: `Contents of ${name}` });
}

function stubWideScreen(): void {
  vi.stubGlobal(
    "matchMedia",
    vi.fn((query: string) => ({
      matches: query === WIDE_QUERY,
      media: query,
      addEventListener: () => undefined,
      removeEventListener: () => undefined,
    })),
  );
}

beforeEach(() => {
  requested = [];
  saved = [];
  workspaces = {
    atlas: { "report.md": "atlas report" },
    scout: { "report.md": "scout report" },
    drifter: { "report.md": "drifter report", "notes.txt": "drifter notes\n" },
  };
  FakeWebSocket.install();
  stubWideScreen();
  serve();
  hub.handleFrame(
    snapshot([
      summary("atlas", "running"),
      summary("scout", "running"),
      summary("drifter", "stopped"),
    ]),
  );
});

afterEach(async () => {
  await go(INBOX, null);
  setViewedAgent(null);
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("the file in the context panel", () => {
  it("is read from the agent the panel names, beside the Inbox", async () => {
    render(PanelHost);
    await go(INBOX, file("atlas"));
    const editor = await openEditor();
    expect(editor.value).toBe("atlas report");
    expect(requested.some((path) => path === "/api/agents/atlas/workspace/file")).toBe(true);
  });

  it("is read from the place's agent when the panel names none", async () => {
    render(PanelHost);
    await go({ kind: "files", agent: "scout" }, file(null));
    const editor = await openEditor();
    expect(editor.value).toBe("scout report");
  });

  it("starts a new buffer when the panel switches to another agent's file, keeping nothing of the first", async () => {
    render(PanelHost);
    await go(INBOX, file("atlas"));
    const editor = await openEditor();
    await userEvent.type(editor, " edited");
    expect(editor.value).toBe("atlas report edited");

    await go(INBOX, file("scout"));
    expect(await screen.findByText("scout report")).toBeTruthy();
    expect(screen.queryByDisplayValue("atlas report edited")).toBeNull();
    expect(screen.queryByText("atlas report edited")).toBeNull();
  });

  it("keeps the buffer and its edits when only the place changes under a panel naming its agent", async () => {
    render(PanelHost);
    await go(INBOX, file("atlas"));
    const editor = await openEditor();
    await userEvent.type(editor, " edited");

    await go({ ...INBOX, thread: "scout/user/a" }, file("atlas"));
    expect(await screen.findByDisplayValue("atlas report edited")).toBeTruthy();
  });
});

describe("an agent's file beside the Inbox", () => {
  beforeEach(() => {
    setViewedAgent("scout");
  });

  it("shows a Markdown file rendered and a text file as written", async () => {
    workspaces["atlas"] = {
      "report.md": "# Findings\n\nSee [the notes](notes.md).\n",
      "notes.txt": "# kept as typed\n",
    };
    render(PanelHost);
    await go(INBOX, file("atlas"));
    expect(await screen.findByRole("heading", { name: "Findings" })).toBeTruthy();

    await go(INBOX, file("atlas", "notes.txt"));
    const text = await screen.findByText("# kept as typed");
    expect(text.tagName).toBe("PRE");
    expect(screen.queryByRole("heading", { name: "kept as typed" })).toBeNull();
  });

  it("follows a change to the file that the agent reports", async () => {
    render(PanelHost);
    await go(INBOX, file("atlas"));
    expect(await screen.findByText("atlas report")).toBeTruthy();

    const socket = await waitFor(() => {
      const found = socketOf("atlas");
      if (found === undefined) throw new Error("no connection to atlas yet");
      return found;
    });
    socket.simulateOpen();
    expect(socket.sentFrames()).toContainEqual({
      type: "watch_workspace",
      prefixes: ["report.md"],
    });

    workspaces["atlas"] = { "report.md": "atlas report, revised" };
    socket.simulateMessage({
      type: "workspace_changed",
      changes: [{ path: "report.md", kind: "modified" }],
    });
    expect(await screen.findByText("atlas report, revised")).toBeTruthy();
  });

  it("closes the connection when the panel goes", async () => {
    render(PanelHost);
    await go(INBOX, file("atlas"));
    await screen.findByText("atlas report");
    const socket = socketOf("atlas");
    if (socket === undefined) throw new Error("no connection to atlas");
    socket.simulateOpen();

    await go(INBOX, null);
    expect(socket.readyState).toBe(FakeWebSocket.CLOSED);
  });

  it("opens a link in the file as the same agent's file, staying on the Inbox", async () => {
    workspaces["atlas"] = {
      "reports/q3.md": "See [the summary](summary.md) and [the plan](../plan.md).\n",
      "reports/summary.md": "the summary",
      "plan.md": "the plan",
    };
    const openPlace = vi.spyOn(router, "openPlace");
    const openPanel = vi.spyOn(router, "openPanel");
    render(PanelHost);
    await go(INBOX, file("atlas", "reports/q3.md"));

    await userEvent.click(await screen.findByRole("link", { name: "the summary" }));
    expect(openPanel).toHaveBeenLastCalledWith({
      kind: "file",
      path: "reports/summary.md",
      agent: "atlas",
    });
    expect(router.place.kind).toBe("inbox");
    expect(router.panel).toEqual({ kind: "file", path: "reports/summary.md", agent: "atlas" });
    expect(await screen.findByText("the summary")).toBeTruthy();

    await go(INBOX, file("atlas", "reports/q3.md"));
    await userEvent.click(await screen.findByRole("link", { name: "the plan" }));
    expect(openPanel).toHaveBeenLastCalledWith({ kind: "file", path: "plan.md", agent: "atlas" });
    expect(openPlace).not.toHaveBeenCalled();
  });

  it("saves to the agent's own workspace", async () => {
    vi.spyOn(toast, "success").mockImplementation(() => 0);
    render(PanelHost);
    await go(INBOX, file("atlas"));
    await userEvent.type(await openEditor(), " amended");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    await settle();

    expect(saved).toEqual([{ agent: "atlas", path: "report.md", content: "atlas report amended" }]);
    expect(screen.queryByRole("button", { name: "Save" })).toBeNull();
  });

  it("loads, renders, edits and saves a stopped agent's file, with no connection to follow it", async () => {
    vi.spyOn(toast, "success").mockImplementation(() => 0);
    render(PanelHost);
    await go(INBOX, file("drifter", "notes.txt"));
    expect(await screen.findByText("drifter notes", { exact: false })).toBeTruthy();
    expect(screen.queryByText(/Live updates are off/)).toBeNull();

    await userEvent.type(await openEditor("notes.txt"), "more");
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    await settle();

    expect(saved).toEqual([
      { agent: "drifter", path: "notes.txt", content: "drifter notes\nmore" },
    ]);
    expect(socketOf("drifter")).toBeUndefined();
  });

  it("follows the file once its stopped agent runs", async () => {
    render(PanelHost);
    await go(INBOX, file("drifter"));
    expect(await screen.findByText("drifter report")).toBeTruthy();
    expect(socketOf("drifter")).toBeUndefined();

    hub.handleFrame({ type: "agent_state", agent: summary("drifter", "running") });
    await settle();
    const socket = socketOf("drifter");
    if (socket === undefined) throw new Error("no connection to drifter");
    socket.simulateOpen();
    workspaces["drifter"] = { "report.md": "drifter report, started" };
    socket.simulateMessage({
      type: "workspace_changed",
      changes: [{ path: "report.md", kind: "modified" }],
    });
    expect(await screen.findByText("drifter report, started")).toBeTruthy();
  });

  it("says quietly that it can't follow the file when the agent has live updates off", async () => {
    render(PanelHost);
    await go(INBOX, file("atlas"));
    await screen.findByText("atlas report");
    const socket = socketOf("atlas");
    if (socket === undefined) throw new Error("no connection to atlas");
    socket.simulateOpen();

    socket.simulateMessage({ type: "workspace_watch_unavailable", message: "off" });
    const note = await screen.findByText(/Live updates are off for atlas/);
    expect(note.textContent).toContain("report.md again");
    expect(await openEditor()).toBeTruthy();
  });
});

describe("agent places in the context panel", () => {
  it("renders Activity, Schedule and Files in the panel, and maximizes them to the main region", async () => {
    setViewedAgent("atlas");
    const openPlace = vi.spyOn(router, "open").mockResolvedValue(true);
    render(PanelHost);

    await go({ kind: "chat", agent: "atlas" }, { kind: "activity", agent: "atlas" });
    expect(screen.getByRole("heading", { name: "Activity" })).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: "Maximize Activity" }));
    expect(openPlace).toHaveBeenLastCalledWith(locationAt({ kind: "activity", agent: "atlas" }));

    await go({ kind: "chat", agent: "atlas" }, { kind: "schedule", agent: "atlas" });
    expect(screen.getByRole("heading", { name: "Schedule" })).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: "Maximize Schedule" }));
    expect(openPlace).toHaveBeenLastCalledWith(locationAt({ kind: "schedule", agent: "atlas" }));

    await go({ kind: "chat", agent: "atlas" }, { kind: "files", agent: "atlas" });
    expect(screen.getByRole("heading", { name: "Files" })).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: "Maximize Files" }));
    expect(openPlace).toHaveBeenLastCalledWith(locationAt({ kind: "files", agent: "atlas" }));
  });

  it("offers Back to the panel it replaced, and goes back when pressed", async () => {
    setViewedAgent("atlas");
    vi.spyOn(router, "canReturnToPanel", "get").mockReturnValue(true);
    const returnToPanel = vi.spyOn(router, "returnToPanel").mockImplementation(() => {});
    render(PanelHost);

    await go({ kind: "chat", agent: "atlas" }, { kind: "activity", agent: "atlas" });
    expect(screen.getByRole("heading", { name: "Activity" })).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: "Back to previous panel" }));
    expect(returnToPanel).toHaveBeenCalledOnce();
  });

  it("hides Back when the panel didn't replace another one", async () => {
    setViewedAgent("atlas");
    vi.spyOn(router, "canReturnToPanel", "get").mockReturnValue(false);
    render(PanelHost);

    await go({ kind: "chat", agent: "atlas" }, { kind: "activity", agent: "atlas" });
    expect(screen.getByRole("heading", { name: "Activity" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Back to previous panel" })).toBeNull();
  });

  it("remembers the panel width across Maximize and Restore", async () => {
    setViewedAgent("atlas");
    localStorage.removeItem("residuum-panel-width");
    vi.stubGlobal("innerWidth", 1400);
    render(PanelHost);

    await go({ kind: "chat", agent: "atlas" }, { kind: "activity", agent: "atlas" });
    const grip = screen.getByRole("separator", { name: "Resize panel" });
    grip.focus();
    await userEvent.keyboard("{ArrowLeft}");
    const resizedWidth = grip.getAttribute("aria-valuenow");
    expect(resizedWidth).toBe("456");

    await go({ kind: "activity", agent: "atlas" }, null);
    expect(screen.queryByRole("separator", { name: "Resize panel" })).toBeNull();

    await go({ kind: "chat", agent: "atlas" }, { kind: "activity", agent: "atlas" });
    expect(
      screen.getByRole("separator", { name: "Resize panel" }).getAttribute("aria-valuenow"),
    ).toBe("456");
  });
});
