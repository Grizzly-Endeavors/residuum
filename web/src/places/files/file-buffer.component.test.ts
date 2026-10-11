import { afterEach, beforeEach, describe, expect, it, vi, type MockInstance } from "vitest";
import { configCoordinator } from "../../lib/config-coordinator";
import { toast } from "../../lib/toast.svelte";
import { jsonResponse, mockFetch } from "../../test/component";
import { FileBuffer } from "./file-buffer.svelte";
import { waitFor } from "../../test/wait";

// A fake workspace: files by path with a version that changes on each write,
// the agent's config.toml behind its raw route, and every request made.

interface Disk {
  files: Map<string, string>;
  versions: Map<string, number>;
  config: string;
  requests: { method: string; url: string; ifMatch?: string; body?: string }[];
}

let disk: Disk;
let saved: MockInstance<(typeof toast)["success"]>;

function version(path: string): string {
  return `v${String(disk.versions.get(path) ?? 0)}`;
}

function write(path: string, content: string): void {
  disk.files.set(path, content);
  disk.versions.set(path, (disk.versions.get(path) ?? 0) + 1);
}

function serve(): void {
  mockFetch((url, init) => {
    const method = init?.method ?? "GET";
    const headers = new Headers(init?.headers);
    const body = typeof init?.body === "string" ? init.body : undefined;
    disk.requests.push({ method, url, ifMatch: headers.get("If-Match") ?? undefined, body });
    const parsed = new URL(url, "http://localhost");
    if (parsed.pathname === "/api/agents/atlas/workspace/file") {
      if (method === "GET") {
        const path = parsed.searchParams.get("path") ?? "";
        const content = disk.files.get(path);
        if (content === undefined) return new Response("path not found", { status: 404 });
        return new Response(content, { headers: { ETag: version(path) } });
      }
      const sent = JSON.parse(body ?? "{}") as { path: string; content: string };
      const match = headers.get("If-Match");
      if (match !== null && match !== version(sent.path)) {
        const current = disk.files.has(sent.path) ? version(sent.path) : null;
        return jsonResponse({ error: "file has changed", current_version: current }, 412);
      }
      write(sent.path, sent.content);
      return jsonResponse({ saved: true, version: version(sent.path), diagnostics: [] });
    }
    if (parsed.pathname === "/api/agents/atlas/config/raw") {
      if (method === "GET") return new Response(disk.config);
      disk.config = body ?? "";
      return jsonResponse({ valid: true });
    }
    if (parsed.pathname === "/api/agents/atlas/workspace/validate") {
      return jsonResponse({ diagnostics: [{ severity: "error", message: "bad" }] });
    }
    return new Response("not served", { status: 500 });
  });
}

const ATLAS = { agent: "atlas", scope: "agent" } as const;

beforeEach(() => {
  disk = { files: new Map(), versions: new Map(), config: "", requests: [] };
  write("notes.md", "first\n");
  serve();
  saved = vi.spyOn(toast, "success").mockImplementation(() => 0);
  vi.spyOn(toast, "error").mockImplementation(() => 0);
  vi.spyOn(console, "error").mockImplementation(() => {});
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

async function opened(path = "notes.md"): Promise<FileBuffer> {
  const buffer = new FileBuffer(ATLAS);
  await buffer.open(path);
  return buffer;
}

describe("opening a file", () => {
  it("reads the file and starts with no edits", async () => {
    const buffer = await opened();
    expect(buffer.status).toBe("ready");
    expect(buffer.text).toBe("first\n");
    expect(buffer.dirty).toBe(false);
    buffer.text = "changed\n";
    expect(buffer.dirty).toBe(true);
  });

  it("says plainly when the file isn't there", async () => {
    const buffer = await opened("gone.md");
    expect(buffer.status).toBe("missing");
    expect(buffer.dirty).toBe(false);
  });

  it("explains a read that failed", async () => {
    mockFetch(() => new Response("boom", { status: 500 }));
    const buffer = await opened();
    expect(buffer.status).toBe("error");
    expect(buffer.error).toMatch(/^Couldn't open notes\.md\./);
  });
});

describe("saving", () => {
  it("writes with the version it read, and keeps what it wrote as saved", async () => {
    const buffer = await opened();
    buffer.text = "second\n";
    await buffer.save();
    expect(disk.files.get("notes.md")).toBe("second\n");
    expect(disk.requests.find((r) => r.method === "PUT")?.ifMatch).toBe("v1");
    expect(buffer.dirty).toBe(false);
    expect(saved).toHaveBeenCalledWith("Saved notes.md.");
  });

  it("asks when the file changed since it was read, and overwrites only when told", async () => {
    const buffer = await opened();
    write("notes.md", "theirs\n");
    buffer.text = "mine\n";
    const saving = buffer.save();
    await waitFor(() => {
      expect(buffer.conflict).not.toBeNull();
    });
    expect(disk.files.get("notes.md")).toBe("theirs\n");

    buffer.answer("keep-mine");
    await saving;
    expect(disk.files.get("notes.md")).toBe("mine\n");
    expect(disk.requests.filter((r) => r.method === "PUT").map((r) => r.ifMatch)).toEqual([
      "v1",
      "v2",
    ]);
    expect(buffer.dirty).toBe(false);
  });

  it("reloads what is on disk when the user takes it", async () => {
    const buffer = await opened();
    write("notes.md", "theirs\n");
    buffer.text = "mine\n";
    const saving = buffer.save();
    await waitFor(() => {
      expect(buffer.conflict).not.toBeNull();
    });
    buffer.answer("use-disk");
    await saving;
    expect(buffer.text).toBe("theirs\n");
    expect(buffer.dirty).toBe(false);
  });

  it("keeps the edits, and writes nothing, when the user puts the question off", async () => {
    const buffer = await opened();
    write("notes.md", "theirs\n");
    buffer.text = "mine\n";
    const saving = buffer.save();
    await waitFor(() => {
      expect(buffer.conflict).not.toBeNull();
    });
    buffer.answer("cancel");
    await saving;
    expect(buffer.text).toBe("mine\n");
    expect(buffer.dirty).toBe(true);
    expect(disk.files.get("notes.md")).toBe("theirs\n");
  });

  it("saves an agent's config file through the config write coordinator", async () => {
    disk.config = 'name = "atlas"\n';
    write("config/config.toml", disk.config);
    const heard = vi.fn();
    const stop = configCoordinator.subscribe(
      { kind: "agent", agent: "atlas", name: "config" },
      heard,
    );
    const buffer = await opened("config/config.toml");
    buffer.text = 'name = "atlas two"\n';
    await buffer.save();
    stop();

    expect(disk.config).toBe('name = "atlas two"\n');
    expect(disk.requests.some((r) => r.method === "PUT" && r.url.includes("/workspace/"))).toBe(
      false,
    );
    expect(heard).toHaveBeenCalledWith(expect.objectContaining({ cause: "write" }));
    expect(buffer.dirty).toBe(false);
  });
});

describe("changes made elsewhere", () => {
  it("takes the new text when there are no edits", async () => {
    const buffer = await opened();
    write("notes.md", "from the agent\n");
    await buffer.refresh();
    expect(buffer.text).toBe("from the agent\n");
    expect(buffer.changedOnDisk).toBeNull();
  });

  it("keeps the edits and says the file changed under them", async () => {
    const buffer = await opened();
    buffer.text = "mine\n";
    write("notes.md", "from the agent\n");
    await buffer.refresh();
    expect(buffer.text).toBe("mine\n");
    expect(buffer.changedOnDisk).toBe("changed");

    disk.files.delete("notes.md");
    await buffer.refresh();
    expect(buffer.changedOnDisk).toBe("removed");
  });

  it("hears its own write come back as no change, so the next save doesn't conflict", async () => {
    const buffer = await opened();
    // Another writer leaves the text the same and moves the version on.
    write("notes.md", "first\n");
    buffer.text = "mine\n";
    await buffer.refresh();
    expect(buffer.changedOnDisk).toBeNull();
    await buffer.save();
    expect(buffer.conflict).toBeNull();
    expect(disk.files.get("notes.md")).toBe("mine\n");
  });

  it("shows the file as gone once it is deleted from the tree, and back when restored", async () => {
    const buffer = await opened();
    buffer.removed();
    expect(buffer.status).toBe("missing");
    await buffer.refresh();
    expect(buffer.status).toBe("ready");
  });

  it("keeps the edits across a rename", async () => {
    const buffer = await opened();
    buffer.text = "mine\n";
    buffer.moved("notes/renamed.md");
    expect(buffer.shows(ATLAS, "notes/renamed.md")).toBe(true);
    expect(buffer.name).toBe("renamed.md");
    expect(buffer.dirty).toBe(true);
  });
});

describe("validation", () => {
  it("checks the text in the editor without writing it", async () => {
    const buffer = await opened();
    buffer.text = "anything";
    await buffer.validate();
    expect(buffer.diagnostics).toEqual([{ severity: "error", message: "bad" }]);
    expect(disk.files.get("notes.md")).toBe("first\n");
  });
});
