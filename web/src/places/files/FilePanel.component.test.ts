import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { router } from "../../lib/router.svelte";
import { HOME, INBOX, type AppLocation } from "../../lib/routes";
import { toast } from "../../lib/toast.svelte";
import { jsonResponse, mockFetch, render, screen, settle } from "../../test/component";
import FilePanelHarness from "../../test/ui/FilePanelHarness.svelte";
import { waitFor } from "../../test/wait";
import { FileBuffer, panelFile } from "./file-buffer.svelte";

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
function at(place: AppLocation["place"], path?: string, agent: string | null = null): AppLocation {
  return {
    place,
    panel: path === undefined ? null : { kind: "file", path, agent },
    settings: null,
  };
}

/** Markdown opens rendered; Edit switches to the editor. */
async function openEditor(name = "plan.md"): Promise<HTMLElement> {
  await userEvent.click(await screen.findByRole("button", { name: "Edit" }));
  return screen.findByRole("textbox", { name: `Contents of ${name}` });
}

beforeEach(() => {
  files = {
    "notes/plan.md": "line one\nline two\nline three\n",
    "notes/readme.txt": "plain text\n",
  };
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
    const editor = await openEditor();
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
    const editor = await openEditor();
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
    const user = userEvent.setup({ advanceTimers: (ms) => vi.advanceTimersByTime(ms) });
    await user.click(await waitFor(() => screen.getByRole("button", { name: "Edit" })));
    const editor = await waitFor(() =>
      screen.getByRole("textbox", { name: "Contents of plan.md" }),
    );
    if (!(editor instanceof HTMLTextAreaElement)) throw new Error("the editor is not a text area");
    await vi.advanceTimersByTimeAsync(600);
    const problems = await waitFor(() => screen.getByRole("list", { name: "Problems in plan.md" }));
    expect(problems.textContent).toContain("expected a value");

    await user.click(screen.getByRole("button", { name: "line 2, column 6" }));
    expect(document.activeElement).toBe(editor);
    expect(editor.selectionStart).toBe("line one\n".length + 5);
    vi.useRealTimers();
  });

  it("is the panel's file while it shows, for the tree to tell about renames", async () => {
    const { unmount } = render(FilePanelHarness, { source: ATLAS, path: "notes/plan.md" });
    await openEditor();
    expect(panelFile.shown?.shows(ATLAS, "notes/plan.md")).toBe(true);
    unmount();
    expect(panelFile.shown).toBeNull();
  });
});

describe("a Markdown file in the panel", () => {
  it("opens rendered, with no editor until Edit is pressed", async () => {
    files["notes/plan.md"] = "# Plan\n\n- first\n- second\n";
    render(FilePanelHarness, { source: ATLAS, path: "notes/plan.md" });
    expect(await screen.findByRole("heading", { name: "Plan" })).toBeTruthy();
    expect(screen.getAllByRole("listitem")).toHaveLength(2);
    expect(screen.queryByRole("textbox")).toBeNull();
    const edit = screen.getByRole("button", { name: "Edit" });
    expect(edit.getAttribute("aria-pressed")).toBe("false");
    expect(screen.getByRole("button", { name: /History/ })).toBeTruthy();
  });

  it("shows an empty file as a line that says so", async () => {
    files["notes/plan.md"] = "  \n";
    render(FilePanelHarness, { source: ATLAS, path: "notes/plan.md" });
    expect(await screen.findByText("Nothing written yet. Edit to add text.")).toBeTruthy();
  });

  it("toggles back to the rendered buffer, keeping unsaved edits", async () => {
    render(FilePanelHarness, { source: ATLAS, path: "notes/plan.md" });
    const editor = await openEditor();
    expect(screen.getByRole("button", { name: "Edit" }).getAttribute("aria-pressed")).toBe("true");
    await userEvent.type(editor, "*bold*");
    expect(screen.getByText("Unsaved changes")).toBeTruthy();

    await userEvent.click(screen.getByRole("button", { name: "Edit" }));
    expect(screen.queryByRole("textbox")).toBeNull();
    expect(screen.getByRole("button", { name: "Edit" }).getAttribute("aria-pressed")).toBe("false");
    expect(screen.getByText("line three", { exact: false })).toBeTruthy();
    expect(screen.getByText("bold").tagName).toBe("EM");
    expect(screen.getByText("Unsaved changes")).toBeTruthy();
    expect(files["notes/plan.md"]).toBe("line one\nline two\nline three\n");

    await userEvent.click(screen.getByRole("button", { name: "Discard" }));
    expect(screen.queryByText("Unsaved changes")).toBeNull();
    expect(screen.queryByText("bold")).toBeNull();
  });

  it("keeps the editor open when the panel is drawn again with the same file", async () => {
    const buffer = new FileBuffer(ATLAS);
    await buffer.open("notes/plan.md");
    const first = render(FilePanelHarness, { source: ATLAS, path: "notes/plan.md", buffer });
    await openEditor();
    first.unmount();

    render(FilePanelHarness, { source: ATLAS, path: "notes/plan.md", buffer });
    expect(await screen.findByRole("textbox", { name: "Contents of plan.md" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Edit" }).getAttribute("aria-pressed")).toBe("true");
  });

  it("shows another Markdown file rendered, not in the editor", async () => {
    files["notes/other.md"] = "# Other\n";
    const { rerender } = render(FilePanelHarness, { source: ATLAS, path: "notes/plan.md" });
    await openEditor();

    await rerender({ source: ATLAS, path: "notes/other.md" });
    expect(await screen.findByRole("heading", { name: "Other" })).toBeTruthy();
    expect(screen.queryByRole("textbox")).toBeNull();
    expect(screen.getByRole("button", { name: "Edit" }).getAttribute("aria-pressed")).toBe("false");
  });

  it("saves with Ctrl+S from the rendered view too", async () => {
    render(FilePanelHarness, { source: ATLAS, path: "notes/plan.md" });
    await userEvent.type(await openEditor(), "x");
    await userEvent.click(screen.getByRole("button", { name: "Edit" }));
    expect(screen.queryByRole("textbox")).toBeNull();

    await userEvent.keyboard("{Control>}s{/Control}");
    await settle();
    expect(files["notes/plan.md"]).toBe("line one\nline two\nline three\nx");
  });

  it("says the file has problems in the rendered view, and opens the editor to see them", async () => {
    diagnostics = [
      {
        severity: "error",
        message: "expected a value",
        location: { kind: "line_column", line: 2, column: 6 },
      },
    ];
    files["notes/plan.md"] = "# Plan\n";
    render(FilePanelHarness, { source: ATLAS, path: "notes/plan.md" });

    expect(await screen.findByText("This file has a problem.")).toBeTruthy();
    expect(screen.queryByRole("textbox")).toBeNull();
    await userEvent.click(screen.getByRole("button", { name: "Edit to see them" }));
    const problems = await screen.findByRole("list", { name: "Problems in plan.md" });
    expect(problems.textContent).toContain("expected a value");
  });

  it("shows frontmatter as its fields, with the body below them", async () => {
    files["notes/plan.md"] = "---\ntype: Agent\ntitle: Atlas\n---\n# Body\n";
    render(FilePanelHarness, { source: ATLAS, path: "notes/plan.md" });

    expect(await screen.findByRole("heading", { name: "Body" })).toBeTruthy();
    expect(screen.getByText("type")).toBeTruthy();
    expect(screen.getByText("Agent")).toBeTruthy();
    expect(screen.getByText("title")).toBeTruthy();
    expect(screen.queryByRole("heading", { name: /type/ })).toBeNull();
  });

  it("opens a file that is neither Markdown nor text in the editor, with no Edit toggle", async () => {
    files["notes/data.json"] = '{"a": 1}\n';
    render(FilePanelHarness, { source: ATLAS, path: "notes/data.json" });
    expect(await screen.findByRole("textbox", { name: "Contents of data.json" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Edit" })).toBeNull();
  });
});

describe("a plain text file in the panel", () => {
  it("opens as written, with no Markdown formatting, until Edit is pressed", async () => {
    files["notes/readme.txt"] = "# not a heading\n- not a list\n  indented *plain*\n";
    render(FilePanelHarness, { source: ATLAS, path: "notes/readme.txt" });

    const text = await screen.findByText("# not a heading", { exact: false });
    expect(text.tagName).toBe("PRE");
    expect(text.textContent).toBe("# not a heading\n- not a list\n  indented *plain*\n");
    expect(screen.queryByRole("heading", { name: "not a heading" })).toBeNull();
    expect(screen.queryByRole("listitem")).toBeNull();
    expect(screen.queryByRole("textbox")).toBeNull();
    expect(screen.getByRole("button", { name: "Edit" }).getAttribute("aria-pressed")).toBe("false");
  });

  it("edits through the same Edit toggle, keeping unsaved edits in the rendered text", async () => {
    render(FilePanelHarness, { source: ATLAS, path: "notes/readme.txt" });
    const editor = await openEditor("readme.txt");
    await userEvent.type(editor, "more");
    await userEvent.click(screen.getByRole("button", { name: "Edit" }));

    expect(screen.queryByRole("textbox")).toBeNull();
    expect(screen.getByText("plain text", { exact: false }).textContent).toBe("plain text\nmore");
    expect(screen.getByText("Unsaved changes")).toBeTruthy();

    await userEvent.click(screen.getByRole("button", { name: "Save" }));
    await settle();
    expect(files["notes/readme.txt"]).toBe("plain text\nmore");
  });

  it("says an empty file is empty", async () => {
    files["notes/readme.txt"] = "";
    render(FilePanelHarness, { source: ATLAS, path: "notes/readme.txt" });
    expect(await screen.findByText("Nothing written yet. Edit to add text.")).toBeTruthy();
  });
});

describe("relative links in a rendered Markdown file", () => {
  beforeEach(async () => {
    await router.replacePlace({ kind: "chat", agent: "atlas" });
  });

  afterEach(async () => {
    await router.replacePlace(HOME);
  });

  it("opens a Markdown link in the panel, found from the file's own folder", async () => {
    files["notes/plan.md"] = "See [the config](refs/config.md).\n";
    files["notes/refs/config.md"] = "# Config\n";
    const openPanel = vi.spyOn(router, "openPanel");
    render(FilePanelHarness, { source: ATLAS, path: "notes/plan.md" });

    await userEvent.click(await screen.findByRole("link", { name: "the config" }));

    expect(openPanel).toHaveBeenCalledWith({
      kind: "file",
      path: "notes/refs/config.md",
      agent: null,
    });
  });

  it("opens a link as the same agent's file when the panel names its agent, wherever it is", async () => {
    await router.replacePlace(INBOX);
    files["notes/plan.md"] = "See [the config](refs/config.md) and `notes/other.md`.\n";
    const openPanel = vi.spyOn(router, "openPanel");
    const openPlace = vi.spyOn(router, "openPlace");
    render(FilePanelHarness, { source: ATLAS, path: "notes/plan.md", agentNamed: true });

    await userEvent.click(await screen.findByRole("link", { name: "the config" }));
    expect(openPanel).toHaveBeenLastCalledWith({
      kind: "file",
      path: "notes/refs/config.md",
      agent: "atlas",
    });
    await userEvent.click(screen.getByText("notes/other.md"));
    expect(openPanel).toHaveBeenLastCalledWith({
      kind: "file",
      path: "notes/other.md",
      agent: "atlas",
    });
    expect(openPlace).not.toHaveBeenCalled();
  });

  it("keeps a link that climbs out of the workspace as a link in a new tab", async () => {
    files["notes/plan.md"] = "Out: [secret](../../secret.md).\n";
    const openPanel = vi.spyOn(router, "openPanel");
    render(FilePanelHarness, { source: ATLAS, path: "notes/plan.md" });

    const link = await screen.findByRole("link", { name: "secret" });
    expect(link).toHaveAttribute("target", "_blank");
    await userEvent.click(link);
    expect(openPanel).not.toHaveBeenCalled();
  });
});

describe("the unsaved-edit guard", () => {
  it("lets every navigation through while there are no edits", async () => {
    render(FilePanelHarness, { source: ATLAS, path: "notes/plan.md" });
    await openEditor();
    expect(router.guard.losses(at({ kind: "home" }))).toEqual([]);
    expect(router.guard.losses(null)).toEqual([]);
  });

  it("asks before edits are lost: another place, another file, closing the panel, leaving the app", async () => {
    render(FilePanelHarness, { source: ATLAS, path: "notes/plan.md" });
    await userEvent.type(await openEditor(), "edit");
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
    await userEvent.type(await openEditor(), "edit");
    expect(router.guard.losses(at({ kind: "chat", agent: "atlas" }, "notes/plan.md"))).toEqual([]);
  });

  it("reads a panel that names its agent from that agent, whatever place it is beside", async () => {
    render(FilePanelHarness, { source: ATLAS, path: "notes/plan.md" });
    await userEvent.type(await openEditor(), "edit");
    const loss = ["Unsaved changes to plan.md"];

    expect(router.guard.losses(at(INBOX, "notes/plan.md", "atlas"))).toEqual([]);
    expect(
      router.guard.losses(at({ kind: "chat", agent: "scout" }, "notes/plan.md", "atlas")),
    ).toEqual([]);
    expect(router.guard.losses(at(INBOX, "notes/plan.md", "scout"))).toEqual(loss);
    expect(router.guard.losses(at(INBOX, "notes/other.md", "atlas"))).toEqual(loss);
    expect(router.guard.losses(at({ kind: "shared-files" }, "notes/plan.md", "atlas"))).toEqual([]);
    expect(router.guard.losses(at({ kind: "chat", agent: "scout" }, "notes/plan.md"))).toEqual(
      loss,
    );
  });

  it("stops asking once the panel is gone", async () => {
    const { unmount } = render(FilePanelHarness, { source: ATLAS, path: "notes/plan.md" });
    await userEvent.type(await openEditor(), "edit");
    unmount();
    expect(router.guard.losses(at({ kind: "home" }))).toEqual([]);
  });
});
