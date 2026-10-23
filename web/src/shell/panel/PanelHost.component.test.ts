import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { router } from "../../lib/router.svelte";
import { INBOX, locationAt, type AppLocation, type Panel } from "../../lib/routes";
import {
  jsonResponse,
  mockFetch,
  render,
  screen,
  settle,
  stubWebSocket,
} from "../../test/component";
import { WIDE_QUERY } from "../../styles/breakpoints";
import PanelHost from "./PanelHost.svelte";

/** The text each agent's workspace holds at `report.md`, by the agent whose route was asked. */
const REPORTS: Record<string, string> = { atlas: "atlas report", scout: "scout report" };
let requested: string[] = [];

function serve(): void {
  mockFetch((url) => {
    const parsed = new URL(url, "http://localhost");
    requested.push(parsed.pathname);
    if (parsed.pathname.endsWith("/workspace/validate")) return jsonResponse({ diagnostics: [] });
    if (parsed.pathname.endsWith("/workspace/file")) {
      const agent = /\/api\/agents\/([^/]+)\//.exec(parsed.pathname)?.[1] ?? "";
      const content = REPORTS[agent];
      if (content === undefined) return new Response("path not found", { status: 404 });
      return new Response(content, { headers: { ETag: "v1" } });
    }
    return new Promise<Response>(() => {});
  });
}

/** Move the app to `place` with `panel` open, as the router would. */
async function go(place: AppLocation["place"], panel: Panel | null): Promise<void> {
  router.location = { ...locationAt(place), panel };
  await settle();
}

const file = (agent: string | null): Panel => ({ kind: "file", path: "report.md", agent });

/** A wide screen, where the panel is a column beside the main region. */
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
  stubWebSocket();
  stubWideScreen();
  serve();
});

afterEach(async () => {
  await go(INBOX, null);
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("the file in the context panel", () => {
  it("is read from the agent the panel names, beside the Inbox", async () => {
    render(PanelHost);
    await go(INBOX, file("atlas"));
    const editor = await screen.findByRole("textbox", { name: "Contents of report.md" });
    expect((editor as HTMLTextAreaElement).value).toBe("atlas report");
    expect(requested.some((path) => path === "/api/agents/atlas/workspace/file")).toBe(true);
  });

  it("is read from the place's agent when the panel names none", async () => {
    render(PanelHost);
    await go({ kind: "files", agent: "scout" }, file(null));
    const editor = await screen.findByRole("textbox", { name: "Contents of report.md" });
    expect((editor as HTMLTextAreaElement).value).toBe("scout report");
  });

  it("starts a new buffer when the panel switches to another agent's file, keeping nothing of the first", async () => {
    render(PanelHost);
    await go(INBOX, file("atlas"));
    const editor = await screen.findByRole("textbox", { name: "Contents of report.md" });
    await userEvent.type(editor, " edited");
    expect((editor as HTMLTextAreaElement).value).toBe("atlas report edited");

    await go(INBOX, file("scout"));
    const next = await screen.findByDisplayValue("scout report");
    expect(next).toBeTruthy();
    expect(screen.queryByDisplayValue("atlas report edited")).toBeNull();
  });

  it("keeps the buffer and its edits when only the place changes under a panel naming its agent", async () => {
    render(PanelHost);
    await go(INBOX, file("atlas"));
    const editor = await screen.findByRole("textbox", { name: "Contents of report.md" });
    await userEvent.type(editor, " edited");

    await go({ ...INBOX, thread: "scout/user/a" }, file("atlas"));
    expect(await screen.findByDisplayValue("atlas report edited")).toBeTruthy();
  });
});
