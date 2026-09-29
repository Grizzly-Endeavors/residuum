import { afterEach, describe, expect, it, vi } from "vitest";
import { fireEvent, jsonResponse, mockFetch, render, screen, settle } from "../test/component";
import FileHistoryModal from "./FileHistoryModal.svelte";

const PAGE = {
  items: [
    {
      id: "cp1",
      timestamp: "2026-09-26T14:00:00.000Z",
      address: "main",
      run_id: null,
      turn_id: null,
      trigger: "turn_end",
      summary: "edited a file",
      changed_path_count: 1,
    },
  ],
  next_cursor: null,
};

afterEach(() => {
  vi.unstubAllGlobals();
});

async function openAndRestore(path: string): Promise<{ urls: string[]; restoreBody: unknown }> {
  const urls: string[] = [];
  let restoreBody: unknown = null;
  mockFetch((url, init) => {
    urls.push(url);
    if (url.includes("/restore")) {
      restoreBody = JSON.parse(init?.body as string);
      return jsonResponse({ checkpoint_id: "cp2", restored_paths: ["x"] });
    }
    if (url.includes("/diff")) return jsonResponse({ diff: "+hello" });
    return jsonResponse(PAGE);
  });
  render(FileHistoryModal, { path, onClose: () => {}, onRestored: () => {} });
  await settle();
  await fireEvent.click(await screen.findByText("Restore this version"));
  await settle();
  return { urls, restoreBody };
}

describe("FileHistoryModal repository routing", () => {
  it("reads and restores a team file from the team repository, relative to team/", async () => {
    const { urls, restoreBody } = await openAndRestore("team/wiki/x.md");
    const list = urls.find((u) => u.startsWith("/api/checkpoints?"));
    expect(list).toContain("repo=team");
    expect(list).toContain(`path=${encodeURIComponent("wiki/x.md")}`);
    const diff = urls.find((u) => u.includes("/diff"));
    expect(diff).toContain("repo=team");
    expect(diff).toContain(`path=${encodeURIComponent("wiki/x.md")}`);
    expect(restoreBody).toEqual({ repo: "team", path: "wiki/x.md" });
  });

  it("reads and restores an agent file from the workspace repository unchanged", async () => {
    const { urls, restoreBody } = await openAndRestore("memory/notes.md");
    const list = urls.find((u) => u.startsWith("/api/checkpoints?"));
    expect(list).toContain("repo=workspace");
    expect(list).toContain(`path=${encodeURIComponent("memory/notes.md")}`);
    expect(restoreBody).toEqual({ repo: "workspace", path: "memory/notes.md" });
  });
});
