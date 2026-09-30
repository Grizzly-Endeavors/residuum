import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { WorkspaceEntry } from "../src/lib/types";
import { workspaceRoutes } from "./workspace";
import { batchRead, buildTree, type BatchReadResponse, type TreeResponse } from "./workspace-bulk";
import { fileVersion, writeFile } from "./workspace-tree";
import { createMockEnv } from "./env";
import { createState } from "./state";
import { fetchJson, fetchText, startRouteHarness, type RouteHarness } from "./test-support";

describe("the workspace directory, raw, tree and read routes", () => {
  let harness: RouteHarness;

  beforeEach(async () => {
    harness = await startRouteHarness(workspaceRoutes, createMockEnv({ deterministic: true }));
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
  ): Promise<{ status: number; body: unknown }> {
    const res = await fetch(url, {
      method,
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
    const raw = await res.text();
    try {
      return { status: res.status, body: JSON.parse(raw) as unknown };
    } catch {
      return { status: res.status, body: raw };
    }
  }
  const tree = async (url: string): Promise<TreeResponse> => {
    const res = await fetchJson(url);
    expect(res.status).toBe(200);
    return res.body as TreeResponse;
  };
  const paths = (response: TreeResponse): string[] => response.entries.map((e) => e.path);

  describe("creating a directory", () => {
    it("creates it with its parents, and lists it", async () => {
      expect(await send(agent("/dir"), "POST", { path: "notes/2026/march" })).toEqual({
        status: 200,
        body: { created: true },
      });
      const listed = (await fetchJson(agent("/files?path=notes/2026"))).body as WorkspaceEntry[];
      expect(listed.map((e) => [e.name, e.entry_type])).toEqual([["march", "directory"]]);
    });

    it("succeeds for a directory that is already there", async () => {
      expect((await send(agent("/dir"), "POST", { path: "skills" })).status).toBe(200);
      expect((await send(agent("/dir"), "POST", { path: "skills" })).status).toBe(200);
    });

    it("answers 409 over a file, 400 for no path, and 403 outside the workspace", async () => {
      expect(await send(agent("/dir"), "POST", { path: "SOUL.md" })).toEqual({
        status: 409,
        body: "SOUL.md already exists and is not a directory",
      });
      expect(await send(agent("/dir"), "POST", { path: " " })).toEqual({
        status: 400,
        body: "path is required",
      });
      expect((await send(agent("/dir"), "POST", { path: "../up" })).status).toBe(403);
      expect((await send(agent("/dir"), "POST", {})).status).toBe(422);
    });

    it("creates a team directory through either scope", async () => {
      await send(team("/dir"), "POST", { path: "wiki/archive" });
      await send(agent("/dir"), "POST", { path: "team/wiki/shared" });
      const listed = (await fetchJson(team("/files?path=wiki"))).body as WorkspaceEntry[];
      expect(listed.map((e) => e.name)).toEqual(expect.arrayContaining(["archive", "shared"]));
    });
  });

  describe("raw files", () => {
    it("reads a file with its content type and version", async () => {
      const res = await fetch(agent("/raw?path=SOUL.md"));
      expect(res.status).toBe(200);
      expect(res.headers.get("content-type")).toBe("text/markdown");
      const body = await res.text();
      expect(res.headers.get("etag")).toBe(fileVersion(body));
      expect(body).toContain("# Soul");
    });

    it("guesses the type from the extension, and falls back to bytes", async () => {
      await send(agent("/file"), "PUT", { path: "data.json", content: "{}" });
      await send(agent("/file"), "PUT", { path: "blob.xyz", content: "x" });
      expect((await fetch(agent("/raw?path=data.json"))).headers.get("content-type")).toBe(
        "application/json",
      );
      expect((await fetch(agent("/raw?path=blob.xyz"))).headers.get("content-type")).toBe(
        "application/octet-stream",
      );
    });

    it("answers 404 for a missing file, 500 for a directory, and 400 without a path", async () => {
      expect((await fetchText(agent("/raw?path=nope.md"))).status).toBe(404);
      expect((await fetchText(agent("/raw?path=skills"))).status).toBe(500);
      expect((await fetchText(agent("/raw"))).status).toBe(400);
    });

    it("writes the body as the file, and answers with its version and diagnostics", async () => {
      const res = await fetch(agent("/raw?path=notes/new.md"), {
        method: "PUT",
        body: "# New\n",
      });
      expect(res.status).toBe(200);
      expect(await res.json()).toEqual({
        saved: true,
        version: fileVersion("# New\n"),
        diagnostics: [],
      });
      expect((await fetchText(agent("/file?path=notes/new.md"))).body).toBe("# New\n");
    });

    it("honors If-Match and If-None-Match like the text write", async () => {
      const stale = await fetch(agent("/raw?path=SOUL.md"), {
        method: "PUT",
        headers: { "If-Match": "stale" },
        body: "x",
      });
      expect(stale.status).toBe(412);
      expect(((await stale.json()) as { error: string }).error).toContain("changed");
      const exists = await fetch(agent("/raw?path=SOUL.md"), {
        method: "PUT",
        headers: { "If-None-Match": "*" },
        body: "x",
      });
      expect(exists.status).toBe(412);
    });

    it("refuses to replace the team folder, and reads team files in either scope", async () => {
      expect((await fetch(agent("/raw?path=team"), { method: "PUT", body: "x" })).status).toBe(400);
      expect(await (await fetch(team("/raw?path=USER.md"))).text()).toContain("# User Profile");
      expect(await (await fetch(agent("/raw?path=team/USER.md"))).text()).toContain(
        "# User Profile",
      );
    });
  });

  describe("the tree", () => {
    it("lists every file and directory below the root, in path order, with sizes and versions", async () => {
      const response = await tree(agent("/tree"));
      expect(response).toMatchObject({
        path: "",
        listing_truncated: false,
        content_truncated: false,
      });
      const listed = paths(response);
      expect(listed).toEqual([...listed].sort());
      expect(listed).toEqual(
        expect.arrayContaining([
          "SOUL.md",
          "skills",
          "skills/research/SKILL.md",
          "team",
          "team/AGENTS.md",
        ]),
      );
      const soul = response.entries.find((e) => e.path === "SOUL.md");
      expect(soul).toMatchObject({ type: "file", version: expect.any(String) as string });
      expect(soul?.size).toBeGreaterThan(0);
      expect(response.entries.find((e) => e.path === "skills")?.size).toBeUndefined();
      expect(soul?.content).toBeUndefined();
    });

    it("lists a subdirectory with paths relative to the workspace", async () => {
      const response = await tree(agent("/tree?path=skills"));
      expect(response.path).toBe("skills");
      expect(paths(response)).toEqual(
        expect.arrayContaining(["skills/research", "skills/research/SKILL.md"]),
      );
      expect(paths(response).every((p) => p.startsWith("skills/"))).toBe(true);
    });

    it("lists the team tree with paths relative to team/", async () => {
      const response = await tree(team("/tree"));
      expect(response.path).toBe("");
      expect(paths(response)).toEqual(
        expect.arrayContaining(["AGENTS.md", "wiki", "wiki/index.md"]),
      );
      expect(paths(response).some((p) => p.startsWith("team/"))).toBe(false);
    });

    it("limits the depth", async () => {
      expect(paths(await tree(agent("/tree?depth=1")))).not.toContain("skills/research");
      expect(paths(await tree(agent("/tree?depth=2")))).toContain("skills/research");
      expect(paths(await tree(agent("/tree?depth=2")))).not.toContain("skills/research/SKILL.md");
      expect((await tree(agent("/tree?depth=0"))).entries).toEqual([]);
    });

    it("filters files by glob, and leaves directories out when it does", async () => {
      const byName = paths(await tree(agent("/tree?glob=*.md")));
      expect(byName).toEqual(
        expect.arrayContaining(["SOUL.md", "skills/research/SKILL.md", "team/wiki/index.md"]),
      );
      expect(byName.every((p) => p.endsWith(".md"))).toBe(true);
      const byPath = paths(await tree(agent("/tree?path=skills&glob=research/*.md")));
      expect(byPath).toEqual(["skills/research/SKILL.md", "skills/research/prompt.md"]);
      const several = paths(await tree(agent("/tree?glob=*.yml&glob=*.toml")));
      expect(several).toEqual(expect.arrayContaining(["HEARTBEAT.yml", "PRESENCE.toml"]));
    });

    it("carries each file's content when asked", async () => {
      const response = await tree(agent("/tree?path=skills&content=true"));
      const skill = response.entries.find((e) => e.path === "skills/research/SKILL.md");
      expect(skill?.content).toContain("# Research Skill");
      expect(response.entries.find((e) => e.path === "skills")?.content).toBeUndefined();
    });

    it("answers 404, 400 and 403 the way the backend does", async () => {
      expect((await fetchText(agent("/tree?path=nope"))).status).toBe(404);
      expect(await fetchText(agent("/tree?path=SOUL.md"))).toEqual({
        status: 400,
        body: "SOUL.md is not a directory",
      });
      expect((await fetchText(agent("/tree?depth=x"))).status).toBe(400);
      expect((await fetchText(agent("/tree?path=../x"))).status).toBe(403);
    });
  });

  describe("reading many files", () => {
    const read = (
      url: string,
      body: unknown,
    ): Promise<{ status: number; body: BatchReadResponse }> =>
      send(url, "POST", body) as Promise<{ status: number; body: BatchReadResponse }>;

    it("reads the files in request order, with their versions and content", async () => {
      const { status, body } = await read(agent("/read"), {
        paths: ["SOUL.md", "team/USER.md", "HEARTBEAT.yml"],
      });
      expect(status).toBe(200);
      expect(body.content_truncated).toBe(false);
      expect(body.files.map((f) => f.path)).toEqual(["SOUL.md", "team/USER.md", "HEARTBEAT.yml"]);
      const [soul] = body.files;
      expect(soul?.content).toContain("# Soul");
      expect(soul?.size).toBe(Buffer.byteLength(soul?.content ?? ""));
      expect(soul?.version).toBe(fileVersion(soul?.content ?? ""));
      expect(soul?.error).toBeUndefined();
    });

    it("gives a path that can't be read an error, and still answers 200", async () => {
      const { status, body } = await read(agent("/read"), {
        paths: ["nope.md", "skills", "../etc/passwd", "SOUL.md"],
      });
      expect(status).toBe(200);
      expect(body.files.map((f) => [f.path, f.error])).toEqual([
        ["nope.md", "not_found"],
        ["skills", "is_directory"],
        ["../etc/passwd", "blocked"],
        ["SOUL.md", undefined],
      ]);
    });

    it("reads team files by their team-relative path in the team scope", async () => {
      const { body } = await read(team("/read"), { paths: ["USER.md"] });
      expect(body.files[0]?.content).toContain("# User Profile");
    });

    it("answers 422 without a list of paths", async () => {
      expect((await send(agent("/read"), "POST", { paths: "SOUL.md" })).status).toBe(422);
    });
  });
});

describe("the bulk read budgets", () => {
  it("leaves a file over 1 MiB without its content, and what doesn't fit the response", () => {
    const state = createState("atlas", true, createMockEnv({ deterministic: true }));
    writeFile(state, "big.txt", "x".repeat(1024 * 1024 + 1));
    for (let i = 0; i < 9; i++)
      writeFile(state, `chunks/part-${String(i)}.txt`, "y".repeat(1024 * 1024));

    const walked = buildTree(
      "",
      { stateAt: () => state, label: (key) => key },
      { content: true, globs: [], depth: null },
    );
    const big = walked.entries.find((e) => e.path === "big.txt");
    expect(big).toMatchObject({ skipped: "too_large" });
    expect(big?.content).toBeUndefined();
    const parts = walked.entries.filter((e) => e.path.startsWith("chunks/part-"));
    expect(parts.filter((e) => e.skipped === "budget").length).toBeGreaterThan(0);
    expect(parts.filter((e) => e.content !== undefined).length).toBeGreaterThan(0);
    expect(walked.content_truncated).toBe(true);

    const read = batchRead(["big.txt", "chunks/part-0.txt"], (key) => ({ state, key }));
    expect(read.files[0]).toMatchObject({ path: "big.txt", error: "too_large" });
    expect(read.files[1]?.content).toHaveLength(1024 * 1024);
  });
});
