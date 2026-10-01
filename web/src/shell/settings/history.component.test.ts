import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  agentConfigFile,
  configCoordinator,
  type ConfigChange,
} from "../../lib/config-coordinator";
import { toast } from "../../lib/toast.svelte";
import type { CheckpointSummary } from "../../lib/types";
import { fireEvent, jsonResponse, mockFetch, render, screen, settle } from "../../test/component";
import HistoryBrowser from "./HistoryBrowser.svelte";

// The History browser against a stand-in for the checkpoint routes: the list,
// a checkpoint opened in place, restore and undo through the config write
// coordinator, and the hub's repositories.

const summary = (id: string, text: string, count = 1): CheckpointSummary => ({
  id,
  timestamp: "2026-09-30T10:00:00Z",
  address: "main",
  run_id: null,
  turn_id: null,
  trigger: "turn_end",
  summary: text,
  changed_path_count: count,
});

let agent = "";
let count = 0;
let requests: string[] = [];
let failList = false;

function serve(): void {
  mockFetch((url, init) => {
    const method = init?.method ?? "GET";
    requests.push(`${method} ${url}`);
    const { pathname, searchParams } = new URL(url, "http://ui");
    const repo = searchParams.get("repo");
    if (pathname.endsWith("/checkpoints/stats")) {
      return jsonResponse({ on_disk_bytes: 2048, checkpoint_count: 3, oldest: null });
    }
    if (pathname.endsWith("/checkpoints") && method === "GET") {
      if (failList) return jsonResponse({ error: "the repository is locked" }, 500);
      if (searchParams.get("path") === "nowhere.md")
        return jsonResponse({ items: [], next_cursor: null });
      if (searchParams.get("before") === "cp-2") {
        return jsonResponse({ items: [summary("cp-1", "the first turn")], next_cursor: null });
      }
      const items =
        repo === "hub"
          ? [summary("hub-1", "set a secret")]
          : [summary("cp-3", "updated SOUL.md", 2), summary("cp-2", "config patch")];
      return jsonResponse({ items, next_cursor: repo === "hub" ? null : "cp-2" });
    }
    if (pathname.endsWith("/checkpoints/hub-1")) {
      return jsonResponse({
        summary: summary("hub-1", "set a secret"),
        changed_paths: [{ path: "secrets.toml.enc", kind: "modified" }],
      });
    }
    if (pathname.endsWith("/checkpoints/cp-3")) {
      return jsonResponse({
        summary: summary("cp-3", "updated SOUL.md", 2),
        changed_paths: [
          { path: "SOUL.md", kind: "modified" },
          { path: "config/mcp.json", kind: "modified" },
        ],
      });
    }
    if (pathname.endsWith("/cp-3/diff")) {
      return jsonResponse({ diff: "--- a/SOUL.md\n+++ b/SOUL.md\n@@ -1 +1 @@\n-old\n+new" });
    }
    if (pathname.endsWith("/cp-3/restore")) {
      return jsonResponse({ checkpoint_id: "cp-4", restored_paths: ["config/mcp.json"] });
    }
    if (pathname.endsWith("/cp-3/undo")) {
      return jsonResponse({
        checkpoint_id: "cp-4",
        reverted_paths: ["SOUL.md"],
        skipped_paths: ["config/mcp.json"],
      });
    }
    if (pathname.endsWith("/mcp/raw")) return new Response('{"mcpServers":{}}');
    return new Promise<Response>(() => {});
  });
}

async function shown(): Promise<void> {
  await vi.waitFor(() => {
    expect(screen.queryByText("Loading the history")).toBeNull();
  });
}

beforeEach(() => {
  vi.spyOn(console, "error").mockImplementation(() => {});
  for (const id of [...toast.toasts.keys()]) toast.dismiss(id);
  agent = `history-${String(++count)}`;
  requests = [];
  failList = false;
  serve();
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("the History browser", () => {
  it("lists the workspace's checkpoints with the history's size, and pages older ones in", async () => {
    render(HistoryBrowser, { agent });
    await shown();
    expect(screen.getByRole("radio", { name: "Workspace" })).toBeChecked();
    expect(screen.getByRole("radio", { name: "Config files" })).toBeTruthy();
    expect(screen.getByText("3 checkpoints, 2.0 KB on disk.")).toBeTruthy();
    expect(screen.getByRole("button", { name: /updated SOUL\.md/ })).toBeTruthy();

    await fireEvent.click(screen.getByRole("button", { name: "Show older" }));
    await vi.waitFor(() => {
      expect(screen.getByRole("button", { name: /the first turn/ })).toBeTruthy();
    });
    expect(requests).toContain(
      `GET /api/agents/${agent}/checkpoints?repo=workspace&before=cp-2&limit=30`,
    );
    expect(screen.queryByRole("button", { name: "Show older" })).toBeNull();
  });

  it("opens a checkpoint in place with its files and their changes", async () => {
    render(HistoryBrowser, { agent });
    await shown();
    const row = screen.getByRole("button", { name: /updated SOUL\.md/ });
    await fireEvent.click(row);
    expect(row).toHaveAttribute("aria-expanded", "true");
    await vi.waitFor(() => {
      expect(screen.getByRole("list", { name: "Files this checkpoint changed" })).toBeTruthy();
    });

    await fireEvent.click(screen.getByRole("button", { name: "Changes to SOUL.md" }));
    const diff = await screen.findByRole("region", { name: "Changes to SOUL.md" });
    expect(diff.querySelector('[data-kind="add"]')?.textContent).toBe("+new\n");
    expect(diff.querySelector('[data-kind="remove"]')?.textContent).toBe("-old\n");
  });

  it("restores a file through the coordinator, so a form showing it reads it again", async () => {
    const heard: ConfigChange[] = [];
    const stop = configCoordinator.subscribe(agentConfigFile(agent, "mcp"), (change) => {
      heard.push(change);
    });
    render(HistoryBrowser, { agent });
    await shown();
    await fireEvent.click(screen.getByRole("button", { name: /updated SOUL\.md/ }));
    await fireEvent.click(
      await screen.findByRole("button", { name: "Restore config/mcp.json as it was then" }),
    );
    await vi.waitFor(() => {
      expect(heard.map((change) => change.cause)).toEqual(["restore"]);
    });
    expect(requests).toContain(`POST /api/agents/${agent}/checkpoints/cp-3/restore`);
    expect([...toast.toasts.values()].map((t) => t.message)).toContain("Restored config/mcp.json.");
    stop();
  });

  it("undoes a checkpoint and says what came back and what was left alone", async () => {
    render(HistoryBrowser, { agent });
    await shown();
    await fireEvent.click(screen.getByRole("button", { name: /updated SOUL\.md/ }));
    await fireEvent.click(await screen.findByRole("button", { name: "Undo these changes" }));
    await vi.waitFor(() => {
      expect(
        screen.getByText(
          "Put back SOUL.md. Left config/mcp.json alone, because it changed again since.",
        ),
      ).toBeTruthy();
    });
    expect(requests).toContain(`POST /api/agents/${agent}/checkpoints/cp-3/undo`);
  });

  it("filters by path, and says when nothing matched", async () => {
    render(HistoryBrowser, { agent });
    await shown();
    await fireEvent.input(screen.getByRole("textbox", { name: "Only changes to" }), {
      target: { value: "nowhere.md" },
    });
    await fireEvent.click(screen.getByRole("button", { name: "Filter" }));
    await vi.waitFor(() => {
      expect(screen.getByText(/No checkpoint changed/)).toHaveTextContent(
        "No checkpoint changed nowhere.md.",
      );
    });
    await fireEvent.click(screen.getByRole("button", { name: "Show all" }));
    await vi.waitFor(() => {
      expect(screen.getByRole("button", { name: /updated SOUL\.md/ })).toBeTruthy();
    });
  });

  it("shows a failed load with Try again", async () => {
    failList = true;
    render(HistoryBrowser, { agent });
    await vi.waitFor(() => {
      expect(screen.getByRole("alert")).toHaveTextContent("Couldn't load this history.");
    });
    failList = false;
    await fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    await vi.waitFor(() => {
      expect(screen.getByRole("button", { name: /updated SOUL\.md/ })).toBeTruthy();
    });
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("shows the hub's repositories for no agent, with an encrypted store's restore in words", async () => {
    render(HistoryBrowser, { agent: null });
    await shown();
    expect(screen.getByRole("radio", { name: "Shared files" })).toBeChecked();
    await fireEvent.click(screen.getByRole("radio", { name: "Install-wide config" }));
    await settle();
    await fireEvent.click(await screen.findByRole("button", { name: /set a secret/ }));
    await vi.waitFor(() => {
      expect(
        screen.getByText("Restores the saved secrets to how they were at this point."),
      ).toBeTruthy();
    });
    expect(screen.queryByRole("button", { name: "Changes to secrets.toml.enc" })).toBeNull();
    expect(requests).toContain("GET /api/hub/checkpoints?repo=hub&limit=30");
  });
});
