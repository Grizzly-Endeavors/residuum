import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { router } from "../../lib/router.svelte";
import type { AppLocation } from "../../lib/routes";
import { toast } from "../../lib/toast.svelte";
import { jsonResponse, mockFetch, render, screen, settle } from "../../test/component";
import FilePanelHarness from "../../test/ui/FilePanelHarness.svelte";
import { waitFor } from "../../test/wait";
import { panelFile } from "./file-buffer.svelte";

const ATLAS = { agent: "atlas", scope: "agent" } as const;

let files: Record<string, string> = {};
let diagnostics: unknown[] = [];

function serve(): void {
  mockFetch((url, init) => {
    const parsed = new URL(url, "http://localhost");
    if (parsed.pathname.endsWith("/workspace/validate")) return jsonResponse({ diagnostics });
    if (parsed.pathname.endsWith("/workspace/file") && (init?.method ?? "GET") === "GET") {
      const content = files[parsed.searchParams.get("path") ?? ""];
      if (content === undefined) return new Response("path not found", { status: 404 });
      return new Response(content, { headers: { ETag: "v1" } });
    }
    if (parsed.pathname.endsWith("/workspace/file")) {
      const body = typeof init?.body === "string" ? init.body : "{}";
      const sent = JSON.parse(body) as { path: string; content: string };
      files[sent.path] = sent.content;
      return jsonResponse({ saved: true, version: "v2", diagnostics: [] });
    }
    return new Promise<Response>(() => {});
  });
}

/** Where the router would go: a place, with or without a file in the panel. */
function at(place: AppLocation["place"], path?: string): AppLocation {
  return {
    place,
    panel: path === undefined ? null : { kind: "file", path },
    settings: null,
  };
}

beforeEach(() => {
  files = { "notes/plan.md": "line one\nline two\nline three\n" };
  diagnostics = [];
  serve();
  vi.spyOn(toast, "success").mockImplementation(() => 0);
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("the file panel", () => {
  it("names the file and its folder, and edits it", async () => {
    render(FilePanelHarness, { source: ATLAS, path: "notes/plan.md" });
    const editor = await screen.findByRole("textbox", { name: "Contents of plan.md" });
    expect(screen.getByRole("heading", { name: "plan.md" })).toBeTruthy();
    expect(screen.getByText("notes/plan.md")).toBeTruthy();
    expect(screen.queryByText("Unsaved changes")).toBeNull();

    await userEvent.type(editor, "more");
    expect(screen.getByText("Unsaved changes")).toBeTruthy();
    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    await settle();
    expect(files["notes/plan.md"]).toBe("line one\nline two\nline three\nmore");
    expect(screen.queryByRole("button", { name: "Save" })).toBeNull();
  });

  it("saves with Ctrl+S, and Discard puts the text back", async () => {
    render(FilePanelHarness, { source: ATLAS, path: "notes/plan.md" });
    const editor = await screen.findByRole("textbox", { name: "Contents of plan.md" });
    await userEvent.type(editor, "x");
    await userEvent.click(screen.getByRole("button", { name: "Discard" }));
    expect((editor as HTMLTextAreaElement).value).toBe("line one\nline two\nline three\n");

    await userEvent.type(editor, "y{Control>}s{/Control}");
    await settle();
    expect(files["notes/plan.md"]).toBe("line one\nline two\nline three\ny");
  });

  it("shows a missing file plainly, with nothing to edit", async () => {
    render(FilePanelHarness, { source: ATLAS, path: "notes/gone.md" });
    expect(await screen.findByRole("heading", { name: "This file doesn't exist" })).toBeTruthy();
    expect(screen.queryByRole("textbox")).toBeNull();
    expect(screen.queryByRole("button", { name: /History/ })).toBeNull();
  });

  it("lists diagnostics, and a diagnostic's line moves the caret there", async () => {
    vi.useFakeTimers();
    diagnostics = [
      {
        severity: "error",
        message: "expected a value",
        location: { kind: "line_column", line: 2, column: 6 },
      },
    ];
    render(FilePanelHarness, { source: ATLAS, path: "notes/plan.md" });
    const editor = await waitFor(() =>
      screen.getByRole("textbox", { name: "Contents of plan.md" }),
    );
    if (!(editor instanceof HTMLTextAreaElement)) throw new Error("the editor is not a text area");
    await vi.advanceTimersByTimeAsync(600);
    const problems = await waitFor(() => screen.getByRole("list", { name: "Problems in plan.md" }));
    expect(problems.textContent).toContain("expected a value");

    const user = userEvent.setup({ advanceTimers: (ms) => vi.advanceTimersByTime(ms) });
    await user.click(screen.getByRole("button", { name: "line 2, column 6" }));
    expect(document.activeElement).toBe(editor);
    expect(editor.selectionStart).toBe("line one\n".length + 5);
    vi.useRealTimers();
  });

  it("is the panel's file while it shows, for the tree to tell about renames", async () => {
    const { unmount } = render(FilePanelHarness, { source: ATLAS, path: "notes/plan.md" });
    await screen.findByRole("textbox");
    expect(panelFile.shown?.shows(ATLAS, "notes/plan.md")).toBe(true);
    unmount();
    expect(panelFile.shown).toBeNull();
  });
});

describe("the unsaved-edit guard", () => {
  it("lets every navigation through while there are no edits", async () => {
    render(FilePanelHarness, { source: ATLAS, path: "notes/plan.md" });
    await screen.findByRole("textbox");
    expect(router.guard.losses(at({ kind: "home" }))).toEqual([]);
    expect(router.guard.losses(null)).toEqual([]);
  });

  it("asks before edits are lost: another place, another file, closing the panel, leaving the app", async () => {
    render(FilePanelHarness, { source: ATLAS, path: "notes/plan.md" });
    await userEvent.type(await screen.findByRole("textbox"), "edit");
    const loss = ["Unsaved changes to plan.md"];

    expect(router.guard.losses(at({ kind: "home" }))).toEqual(loss);
    expect(router.guard.losses(at({ kind: "files", agent: "atlas" }, "SOUL.md"))).toEqual(loss);
    expect(router.guard.losses(at({ kind: "files", agent: "atlas" }))).toEqual(loss);
    expect(router.guard.losses(at({ kind: "chat", agent: "scout" }, "notes/plan.md"))).toEqual(
      loss,
    );
    expect(router.guard.losses(at({ kind: "shared-files" }, "notes/plan.md"))).toEqual(loss);
    expect(router.guard.losses(null)).toEqual(loss);
  });

  it("lets a navigation through that keeps the same file in the panel", async () => {
    render(FilePanelHarness, { source: ATLAS, path: "notes/plan.md" });
    await userEvent.type(await screen.findByRole("textbox"), "edit");
    expect(router.guard.losses(at({ kind: "chat", agent: "atlas" }, "notes/plan.md"))).toEqual([]);
  });

  it("stops asking once the panel is gone", async () => {
    const { unmount } = render(FilePanelHarness, { source: ATLAS, path: "notes/plan.md" });
    await userEvent.type(await screen.findByRole("textbox"), "edit");
    unmount();
    expect(router.guard.losses(at({ kind: "home" }))).toEqual([]);
  });
});
