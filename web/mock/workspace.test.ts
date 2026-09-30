import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { WorkspaceEntry } from "../src/lib/types";
import { createState } from "./state";
import { workspaceRoutes } from "./workspace";
import { fileVersion } from "./workspace-tree";
import { fetchJson, fetchText, startRouteHarness, type RouteHarness } from "./test-support";

describe("workspace routes", () => {
  let harness: RouteHarness;

  beforeEach(async () => {
    harness = await startRouteHarness(workspaceRoutes);
  });

  afterEach(async () => {
    await harness.close();
  });

  const agent = (route: string): string => `${harness.baseUrl}/api/workspace${route}`;
  const team = (route: string): string => `${harness.baseUrl}/api/team/workspace${route}`;

  /** A request with a JSON body. The response body is parsed when it is JSON, and left as text when it isn't. */
  async function send(
    url: string,
    method: string,
    body: unknown,
    headers: Record<string, string> = {},
  ): Promise<{ status: number; body: unknown }> {
    const res = await fetch(url, {
      method,
      headers: { "Content-Type": "application/json", ...headers },
      body: JSON.stringify(body),
    });
    const raw = await res.text();
    try {
      return { status: res.status, body: JSON.parse(raw) as unknown };
    } catch {
      return { status: res.status, body: raw };
    }
  }

  async function listing(url: string): Promise<WorkspaceEntry[]> {
    const res = await fetchJson(url);
    expect(res.status).toBe(200);
    return res.body as WorkspaceEntry[];
  }

  async function etag(url: string): Promise<string> {
    const res = await fetch(url);
    expect(res.status).toBe(200);
    return res.headers.get("etag") ?? "";
  }

  describe("listing", () => {
    it("gives every entry the modification time and version the backend reports", async () => {
      for (const entry of await listing(agent("/files"))) {
        expect(entry.modified).toBeGreaterThan(0);
        expect(entry.version).not.toBe("");
      }
    });

    it("lists directories first, and each group by name", async () => {
      const entries = await listing(agent("/files"));
      const kinds = entries.map((e) => e.entry_type);
      expect(kinds.lastIndexOf("directory")).toBeLessThan(kinds.indexOf("file"));
      const dirs = entries.filter((e) => e.entry_type === "directory").map((e) => e.name);
      expect(dirs).toEqual([...dirs].sort());
      const files = entries.filter((e) => e.entry_type === "file").map((e) => e.name);
      expect(files).toEqual([...files].sort());
    });

    it("reports a file's version as the ETag a read of it reports", async () => {
      const entries = await listing(agent("/files?path=skills/research"));
      const skill = entries.find((e) => e.name === "SKILL.md");
      expect(skill?.version).toBe(await etag(agent("/file?path=skills/research/SKILL.md")));
    });

    it("answers 404 for a directory that isn't there, and 403 for a path leaving the workspace", async () => {
      expect(await fetchText(agent("/files?path=nowhere"))).toEqual({
        status: 404,
        body: "path not found: nowhere",
      });
      expect((await fetchText(agent("/files?path=../etc"))).status).toBe(403);
      expect((await fetchText(agent("/files?path=/etc"))).status).toBe(403);
    });

    it("lists an empty directory as no entries", async () => {
      expect(await fetchJson(agent("/files?path=archive"))).toEqual({ status: 200, body: [] });
    });

    it("lists the shared team folder in an agent's root as the hub's", async () => {
      const before = (await listing(agent("/files"))).find((e) => e.name === "team");
      const put = await send(team("/file"), "PUT", { path: "notes.md", content: "hi" });
      expect(put.status).toBe(200);
      const after = (await listing(agent("/files"))).find((e) => e.name === "team");
      expect(after?.version).not.toBe(before?.version);
      expect(after?.modified).toBeGreaterThan(before?.modified ?? 0);
    });

    it("lists the team tree relative to team/", async () => {
      const entries = await listing(team("/files"));
      expect(entries.map((e) => e.name)).toEqual(["wiki", "workbench", "AGENTS.md", "USER.md"]);
      expect((await listing(team("/files?path=wiki"))).map((e) => e.name)).toContain("index.md");
    });
  });

  describe("reading", () => {
    it("serves a file's content with its version as the ETag", async () => {
      const res = await fetch(agent("/file?path=PRESENCE.toml"));
      expect(res.status).toBe(200);
      expect(res.headers.get("content-type")).toBe("text/plain; charset=utf-8");
      expect(await res.text()).toContain("[presence]");
      expect(res.headers.get("etag")).toMatch(/^[0-9a-f]+-[0-9a-f]+$/);
    });

    it("serves the team tree from either scope", async () => {
      const viaAgent = await fetchText(agent("/file?path=team/USER.md"));
      const viaTeam = await fetchText(team("/file?path=USER.md"));
      expect(viaAgent).toEqual(viaTeam);
      expect(viaTeam.body).toContain("User Profile");
    });

    it("answers 404 for a file that isn't there, and 400 without a path", async () => {
      expect(await fetchText(agent("/file?path=missing.md"))).toEqual({
        status: 404,
        body: "path not found: missing.md",
      });
      expect((await fetchText(agent("/file"))).status).toBe(400);
    });
  });

  describe("writing", () => {
    it("saves a file, and lists it with its size in bytes and the new version", async () => {
      const put = await send(agent("/file"), "PUT", { path: "notes/today.md", content: "héllo" });
      expect(put).toEqual({
        status: 200,
        body: {
          saved: true,
          version: await etag(agent("/file?path=notes/today.md")),
          diagnostics: [],
        },
      });
      const entries = await listing(agent("/files?path=notes"));
      expect(entries).toEqual([
        expect.objectContaining({ name: "today.md", entry_type: "file", size: 6 }),
      ]);
      expect((await listing(agent("/files"))).map((e) => e.name)).toContain("notes");
      expect((await fetchText(agent("/file?path=notes/today.md"))).body).toBe("héllo");
    });

    it("changes the version when the content changes, and the listing follows", async () => {
      const first = await etag(agent("/file?path=SOUL.md"));
      await send(agent("/file"), "PUT", { path: "SOUL.md", content: "# Soul\n" });
      const second = await etag(agent("/file?path=SOUL.md"));
      expect(second).not.toBe(first);
      const soul = (await listing(agent("/files"))).find((e) => e.name === "SOUL.md");
      expect(soul).toMatchObject({ version: second, size: 7 });
    });

    it("reports a conflict when the file changed since the client read it", async () => {
      const read = await etag(agent("/file?path=SOUL.md"));
      await send(agent("/file"), "PUT", { path: "SOUL.md", content: "someone else" });
      const current = await etag(agent("/file?path=SOUL.md"));
      const res = await send(
        agent("/file"),
        "PUT",
        { path: "SOUL.md", content: "mine" },
        { "If-Match": read },
      );
      expect(res).toEqual({
        status: 412,
        body: { error: "file has changed since it was last read", current_version: current },
      });
      expect((await fetchText(agent("/file?path=SOUL.md"))).body).toBe("someone else");
    });

    it("saves when the client's version is current", async () => {
      const read = await etag(agent("/file?path=SOUL.md"));
      const res = await send(
        agent("/file"),
        "PUT",
        { path: "SOUL.md", content: "mine" },
        { "If-Match": read },
      );
      expect(res.status).toBe(200);
    });

    it("reports no current version when the file the client read is gone", async () => {
      const res = await send(
        agent("/file"),
        "PUT",
        { path: "gone.md", content: "x" },
        { "If-Match": "abc-1" },
      );
      expect(res).toEqual({
        status: 412,
        body: { error: "file has changed since it was last read", current_version: null },
      });
    });

    it("refuses to create a file that exists when asked to create only", async () => {
      const res = await send(
        agent("/file"),
        "PUT",
        { path: "SOUL.md", content: "x" },
        { "If-None-Match": "*" },
      );
      expect(res.status).toBe(412);
      expect(res.body).toMatchObject({ error: "file already exists" });
      const created = await send(
        agent("/file"),
        "PUT",
        { path: "fresh.md", content: "x" },
        { "If-None-Match": "*" },
      );
      expect(created.status).toBe(200);
    });

    it("writes the team tree from either scope", async () => {
      await send(team("/file"), "PUT", { path: "wiki/new.md", content: "via team" });
      expect((await fetchText(agent("/file?path=team/wiki/new.md"))).body).toBe("via team");
      await send(agent("/file"), "PUT", { path: "team/wiki/other.md", content: "via agent" });
      expect((await fetchText(team("/file?path=wiki/other.md"))).body).toBe("via agent");
      expect(harness.hub.hubState.workspaceFileContents["team/wiki/new.md"]).toBe("via team");
      expect(harness.state.workspaceFileContents["team/wiki/new.md"]).toBeUndefined();
    });

    it("answers 422 for a body without the fields, and refuses the team folder itself", async () => {
      expect((await send(agent("/file"), "PUT", { path: "x.md" })).status).toBe(422);
      expect((await send(agent("/file"), "PUT", { path: "team", content: "x" })).status).toBe(400);
    });
  });

  describe("deleting", () => {
    it("removes a file and its entry, and offers no checkpoint", async () => {
      const res = await fetchJson(agent("/file?path=PRESENCE.toml"), { method: "DELETE" });
      expect(res).toEqual({ status: 200, body: { deleted: true, checkpoint_id: null } });
      expect((await fetchText(agent("/file?path=PRESENCE.toml"))).status).toBe(404);
      expect((await listing(agent("/files"))).map((e) => e.name)).not.toContain("PRESENCE.toml");
    });

    it("needs recursive=true for a directory, then removes everything in it", async () => {
      const refused = await fetchText(agent("/file?path=skills"), { method: "DELETE" });
      expect(refused).toEqual({
        status: 409,
        body: "skills is a directory; pass recursive=true to delete it",
      });
      const res = await fetchJson(agent("/file?path=skills&recursive=true"), { method: "DELETE" });
      expect(res.status).toBe(200);
      expect((await fetchText(agent("/files?path=skills"))).status).toBe(404);
      expect((await fetchText(agent("/file?path=skills/research/SKILL.md"))).status).toBe(404);
      expect((await listing(agent("/files"))).map((e) => e.name)).not.toContain("skills");
    });

    it("answers 404 for a path that isn't there, and refuses the roots", async () => {
      expect((await fetchText(agent("/file?path=nope"), { method: "DELETE" })).status).toBe(404);
      expect(await fetchText(agent("/file?path="), { method: "DELETE" })).toEqual({
        status: 400,
        body: "the workspace root cannot be deleted",
      });
      expect(await fetchText(team("/file?path="), { method: "DELETE" })).toEqual({
        status: 400,
        body: "the team folder cannot be deleted",
      });
    });

    it("refuses a delete of a file that changed since it was read", async () => {
      const res = await fetchJson(agent("/file?path=SOUL.md"), {
        method: "DELETE",
        headers: { "If-Match": "stale-1" },
      });
      expect(res.status).toBe(412);
    });

    it("deletes from the team tree", async () => {
      const res = await fetchJson(team("/file?path=wiki/log.md"), { method: "DELETE" });
      expect(res).toEqual({ status: 200, body: { deleted: true, checkpoint_id: null } });
      expect((await listing(team("/files?path=wiki"))).map((e) => e.name)).not.toContain("log.md");
    });
  });

  describe("moving", () => {
    it("renames a file, keeping its content and reporting its version", async () => {
      const before = await fetchText(agent("/file?path=PRESENCE.toml"));
      const res = await send(agent("/move"), "POST", {
        from: "PRESENCE.toml",
        to: "config/p.toml",
      });
      expect(res).toEqual({
        status: 200,
        body: { moved: true, version: await etag(agent("/file?path=config/p.toml")) },
      });
      expect((await fetchText(agent("/file?path=config/p.toml"))).body).toBe(before.body);
      expect((await fetchText(agent("/file?path=PRESENCE.toml"))).status).toBe(404);
    });

    it("moves a directory with everything in it, and reports no version", async () => {
      const res = await send(agent("/move"), "POST", { from: "skills", to: "abilities" });
      expect(res).toEqual({ status: 200, body: { moved: true, version: null } });
      expect((await fetchText(agent("/file?path=abilities/research/SKILL.md"))).status).toBe(200);
      expect((await listing(agent("/files?path=abilities"))).map((e) => e.name)).toEqual([
        "code-review",
        "research",
      ]);
      expect((await fetchText(agent("/files?path=skills"))).status).toBe(404);
    });

    it("refuses to replace a destination unless told to", async () => {
      const refused = await send(agent("/move"), "POST", {
        from: "PRESENCE.toml",
        to: "HEARTBEAT.yml",
      });
      expect(refused).toEqual({
        status: 409,
        body: "HEARTBEAT.yml already exists; pass overwrite: true to replace it",
      });
      const replaced = await send(agent("/move"), "POST", {
        from: "PRESENCE.toml",
        to: "HEARTBEAT.yml",
        overwrite: true,
      });
      expect(replaced.status).toBe(200);
      expect((await fetchText(agent("/file?path=HEARTBEAT.yml"))).body).toContain("[presence]");
    });

    it("answers 404 for a source that isn't there, and refuses a directory moved into itself", async () => {
      expect((await send(agent("/move"), "POST", { from: "nope", to: "x" })).status).toBe(404);
      expect(await send(agent("/move"), "POST", { from: "skills", to: "skills/inner" })).toEqual({
        status: 400,
        body: "can't move skills into itself (skills/inner)",
      });
    });

    it("treats a move onto itself as a success that changes nothing", async () => {
      const read = await etag(agent("/file?path=SOUL.md"));
      const res = await send(agent("/move"), "POST", { from: "SOUL.md", to: "SOUL.md" });
      expect(res).toEqual({ status: 200, body: { moved: true, version: read } });
    });

    it("refuses a move of a file that changed since it was read", async () => {
      const res = await send(
        agent("/move"),
        "POST",
        { from: "SOUL.md", to: "moved.md" },
        { "If-Match": "stale-1" },
      );
      expect(res.status).toBe(412);
    });

    it("moves files between an agent's workspace and the team tree", async () => {
      const soul = (await fetchText(agent("/file?path=SOUL.md"))).body;
      const res = await send(agent("/move"), "POST", { from: "SOUL.md", to: "team/wiki/soul.md" });
      expect(res.status).toBe(200);
      expect((await fetchText(team("/file?path=wiki/soul.md"))).body).toBe(soul);
      expect((await fetchText(agent("/file?path=SOUL.md"))).status).toBe(404);
      expect(harness.hub.hubState.workspaceFileContents["team/wiki/soul.md"]).toBe(soul);
    });

    it("moves within the team tree, and refuses to move the team folder", async () => {
      expect(
        (await send(team("/move"), "POST", { from: "USER.md", to: "wiki/user.md" })).status,
      ).toBe(200);
      expect((await fetchText(team("/file?path=wiki/user.md"))).status).toBe(200);
      expect((await send(agent("/move"), "POST", { from: "team", to: "crew" })).status).toBe(400);
      expect((await send(team("/move"), "POST", { from: "", to: "crew" })).status).toBe(400);
    });
  });

  describe("validating", () => {
    it("finds nothing to report, in either scope", async () => {
      for (const url of [agent("/validate"), team("/validate")]) {
        const res = await send(url, "POST", { path: "HEARTBEAT.yml", content: "not: [valid" });
        expect(res).toEqual({ status: 200, body: { diagnostics: [] } });
      }
    });

    it("answers 422 without a path and content", async () => {
      expect((await send(agent("/validate"), "POST", {})).status).toBe(422);
    });
  });
});

describe("the sample workspace tree", () => {
  it("lists every file with the version of its content, and every directory with a listing", () => {
    const state = createState("atlas");
    for (const [dir, entries] of Object.entries(state.workspaceFiles)) {
      for (const entry of entries) {
        const path = dir === "" ? entry.name : `${dir}/${entry.name}`;
        if (entry.entry_type === "directory") {
          expect(state.workspaceFiles[path], path).toBeDefined();
        } else {
          const content = state.workspaceFileContents[path];
          expect(content, path).toBeDefined();
          expect(entry.version, path).toBe(fileVersion(content ?? ""));
        }
      }
    }
  });
});
