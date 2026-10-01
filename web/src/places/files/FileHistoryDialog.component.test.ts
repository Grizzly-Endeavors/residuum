import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { configCoordinator } from "../../lib/config-coordinator";
import { toast } from "../../lib/toast.svelte";
import { jsonResponse, mockFetch, render, screen } from "../../test/component";
import FileHistoryDialog from "./FileHistoryDialog.svelte";
import type { FileSource } from "./file-source";

function checkpoint(id: string, summary: string): Record<string, unknown> {
  return {
    id,
    timestamp: "2026-09-26T14:00:00.000Z",
    address: "main",
    run_id: null,
    turn_id: null,
    trigger: "turn_end",
    summary,
    changed_path_count: 1,
  };
}

let urls: string[] = [];
let restoreBody: unknown = null;
let failList = false;

beforeEach(() => {
  urls = [];
  restoreBody = null;
  failList = false;
  mockFetch((url, init) => {
    urls.push(url);
    if (url.includes("/restore")) {
      restoreBody = JSON.parse(typeof init?.body === "string" ? init.body : "null");
      return jsonResponse({ checkpoint_id: "cp3", restored_paths: ["x"] });
    }
    if (url.includes("/diff")) {
      return jsonResponse({ diff: url.includes("cp1") ? "@@ -1 +1 @@\n-old\n+new" : null });
    }
    if (url.includes("/file?")) return new Response("whole file at cp1");
    if (failList) return new Response("checkpoint history unavailable", { status: 500 });
    return jsonResponse({
      items: [checkpoint("cp1", "edited it"), checkpoint("cp2", "an earlier turn")],
      next_cursor: null,
    });
  });
  vi.spyOn(toast, "success").mockImplementation(() => 0);
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

function open(source: FileSource, path: string): void {
  render(FileHistoryDialog, { source, path, onclose: () => {} });
}

describe("a file's history", () => {
  it("selects the newest version and shows what it changed, line by line", async () => {
    open({ agent: "atlas", scope: "agent" }, "SOUL.md");
    const dialog = await screen.findByRole("dialog", { name: "History of SOUL.md" });
    const versions = await screen.findAllByRole("button", { name: /turn end/i });
    expect(versions[0]?.getAttribute("aria-current")).toBe("true");
    expect(await screen.findByText("+new")).toBeTruthy();
    expect(dialog.querySelector('[data-kind="removed"]')?.textContent).toBe("-old");

    await userEvent.click(screen.getByRole("radio", { name: "Whole file" }));
    expect(await screen.findByText("whole file at cp1")).toBeTruthy();

    await userEvent.click(screen.getByRole("radio", { name: "Changes" }));
    const earlier = versions[1];
    if (earlier === undefined) throw new Error("no earlier version");
    await userEvent.click(earlier);
    expect(await screen.findByText("SOUL.md didn't change at this checkpoint.")).toBeTruthy();
  });

  it("restores a version through the config write coordinator", async () => {
    const restore = vi.spyOn(configCoordinator, "restore");
    open({ agent: "atlas", scope: "agent" }, "memory/notes.md");
    await screen.findByText("+new");
    await userEvent.click(screen.getByRole("button", { name: "Restore this version" }));
    await vi.waitFor(() => {
      expect(restoreBody).toEqual({ repo: "workspace", path: "memory/notes.md" });
    });
    expect(restore).toHaveBeenCalledWith("atlas", "cp1", "workspace", "memory/notes.md");
  });

  it("reads a team file from the team repository, relative to the team folder", async () => {
    open({ agent: null, scope: "team" }, "wiki/x.md");
    await screen.findByText("+new");
    const list = urls.find((u) => u.startsWith("/api/hub/checkpoints?"));
    expect(list).toContain("repo=team");
    expect(list).toContain(`path=${encodeURIComponent("wiki/x.md")}`);
    expect(urls.some((u) => u.startsWith("/api/agents/"))).toBe(false);
  });

  it("reads an agent's config.toml from its agent-config repository", async () => {
    open({ agent: "atlas", scope: "agent" }, "config/config.toml");
    await screen.findByText("+new");
    const list = urls.find((u) => u.startsWith("/api/agents/atlas/checkpoints?"));
    expect(list).toContain("repo=agent_config");
    expect(list).toContain("path=config.toml");
  });

  it("says why the history couldn't load, and tries again", async () => {
    failList = true;
    vi.spyOn(console, "error").mockImplementation(() => {});
    open({ agent: "atlas", scope: "agent" }, "SOUL.md");
    expect((await screen.findByRole("alert")).textContent).toMatch(
      /Couldn't load the history of SOUL\.md\./,
    );
    failList = false;
    await userEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(await screen.findByText("+new")).toBeTruthy();
  });
});
