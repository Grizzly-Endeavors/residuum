import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { ArtifactSummary, WorkbenchInfo } from "../src/lib/types";
import {
  fetchJson,
  fetchText,
  startMockServer,
  startRouteHarness,
  type MockServerHarness,
  type RouteHarness,
} from "./test-support";
import { workbenchRoutes } from "./workbench";
import { listDirectory, writeFile } from "./workspace-tree";

describe("workbench routes", () => {
  let harness: RouteHarness;

  beforeEach(async () => {
    harness = await startRouteHarness(workbenchRoutes);
  });

  afterEach(async () => {
    await harness.close();
  });

  const url = (path: string): string => `${harness.baseUrl}/api/workbench${path}`;

  async function artifacts(): Promise<ArtifactSummary[]> {
    const res = await fetchJson(url("/artifacts"));
    expect(res.status).toBe(200);
    return res.body as ArtifactSummary[];
  }

  describe("listing", () => {
    it("lists the sample artifact with the fields the backend reports", async () => {
      const [artifact, ...rest] = await artifacts();
      expect(rest).toEqual([]);
      expect(artifact).toEqual({
        name: "tip-splitter",
        title: "Tip Splitter",
        modified_at: expect.stringMatching(/^\d{4}-\d{2}-\d{2}T/) as unknown,
        size: Buffer.byteLength(harness.state.workbenchArtifacts.get("tip-splitter")?.html ?? ""),
      });
    });

    it("lists the most recently modified artifact first, then by name", async () => {
      const now = Date.now();
      const page = "<title>x</title>";
      harness.state.workbenchArtifacts.set("older", {
        html: page,
        modifiedAt: new Date(now - 60_000).toISOString(),
      });
      harness.state.workbenchArtifacts.set("a-newest", {
        html: page,
        modifiedAt: new Date(now + 60_000).toISOString(),
      });
      harness.state.workbenchArtifacts.set("b-newest", {
        html: page,
        modifiedAt: new Date(now + 60_000).toISOString(),
      });
      expect((await artifacts()).map((artifact) => artifact.name)).toEqual([
        "a-newest",
        "b-newest",
        "tip-splitter",
        "older",
      ]);
    });

    it("titles a page by its first <title>, decoded and tidy, and by its name when it has none", async () => {
      const modifiedAt = new Date().toISOString();
      harness.state.workbenchArtifacts.clear();
      harness.state.workbenchArtifacts.set("spaced", {
        html: "<head><TITLE lang=en>\n  Tom &amp; Jerry's\n   &quot;Chart&quot; </TITLE></head><title>second</title>",
        modifiedAt,
      });
      harness.state.workbenchArtifacts.set("blank", { html: "<title>  </title>", modifiedAt });
      harness.state.workbenchArtifacts.set("bare", { html: "<p>no title</p>", modifiedAt });
      const titles = Object.fromEntries((await artifacts()).map((a) => [a.name, a.title]));
      expect(titles).toEqual({
        spaced: 'Tom & Jerry\'s "Chart"',
        blank: "blank",
        bare: "bare",
      });
    });

    it("measures a page in bytes", async () => {
      harness.state.workbenchArtifacts.clear();
      harness.state.workbenchArtifacts.set("accents", {
        html: "<title>é</title>",
        modifiedAt: new Date().toISOString(),
      });
      expect((await artifacts())[0]?.size).toBe(17);
    });

    it("lists nothing for a workbench with no artifacts", async () => {
      harness.state.workbenchArtifacts.clear();
      expect(await artifacts()).toEqual([]);
    });
  });

  describe("info", () => {
    it("says why artifacts can't be served until the listener is up", async () => {
      expect(await fetchJson(url("/info"))).toEqual({
        status: 200,
        body: {
          port: null,
          unavailable_reason: "The mock artifacts listener isn't up yet.",
          relay: null,
        } satisfies WorkbenchInfo,
      });
    });

    it("reports the listener's port once it is up, with no relay", async () => {
      harness.state.workbenchPort = 7702;
      expect(await fetchJson(url("/info"))).toEqual({
        status: 200,
        body: { port: 7702, unavailable_reason: null, relay: null } satisfies WorkbenchInfo,
      });
    });
  });

  describe("deleting", () => {
    it("removes the artifact and offers no checkpoint", async () => {
      const res = await fetchJson(url("/artifacts/tip-splitter"), { method: "DELETE" });
      expect(res).toEqual({
        status: 200,
        body: { removed: ["tip-splitter.html"], checkpoint_id: null },
      });
      expect(await artifacts()).toEqual([]);
    });

    it("removes the artifact's data files from the team workbench folder, and only those", async () => {
      const team = harness.hub.hubState;
      writeFile(team, "team/workbench/tip-splitter.state.json", '{"bill":84}');
      writeFile(team, "team/workbench/tip-splitter.log.txt", "x");
      writeFile(team, "team/workbench/tip-splitter-2.state.json", "{}");
      writeFile(team, "team/workbench/other.state.json", "{}");

      const res = await fetchJson(url("/artifacts/tip-splitter"), { method: "DELETE" });

      expect(res.body).toEqual({
        removed: ["tip-splitter.html", "tip-splitter.log.txt", "tip-splitter.state.json"],
        checkpoint_id: null,
      });
      const left = (listDirectory(team, "team/workbench") ?? []).map((entry) => entry.name);
      expect(left).toEqual(["other.state.json", "tip-splitter-2.state.json"]);
      expect(team.workspaceFileContents["team/workbench/tip-splitter.state.json"]).toBeUndefined();
    });

    it("answers 404 for an artifact that is already gone", async () => {
      await fetchJson(url("/artifacts/tip-splitter"), { method: "DELETE" });
      expect(await fetchText(url("/artifacts/tip-splitter"), { method: "DELETE" })).toEqual({
        status: 404,
        body: "That artifact no longer exists. It may already have been deleted.",
      });
    });

    it("refuses a name that can't be an artifact's", async () => {
      for (const name of ["Tip-Splitter", "tip--splitter", "-tip", "tip.html", "a".repeat(65)]) {
        const res = await fetchText(url(`/artifacts/${name}`), { method: "DELETE" });
        expect(res.status, name).toBe(400);
        expect(res.body).toBe(
          `invalid artifact name ${JSON.stringify(name)}: use lowercase letters, digits, and single hyphens`,
        );
      }
    });

    it("answers 404 for a path that isn't an artifact route", async () => {
      expect((await fetchText(url("/artifacts/a/b"), { method: "DELETE" })).status).toBe(404);
      expect((await fetchText(url("/artifacts/tip-splitter"))).status).toBe(404);
    });
  });
});

describe("workbench routes over the whole mock", () => {
  let mock: MockServerHarness;

  beforeEach(async () => {
    mock = await startMockServer();
  });

  afterEach(async () => {
    await mock.close();
  });

  it("serves the team's workbench from the shared state", async () => {
    const listed = await fetchJson(`${mock.baseUrl}/api/team/workbench/artifacts`);
    expect((listed.body as ArtifactSummary[]).map((a) => a.name)).toEqual(["tip-splitter"]);

    await fetchJson(`${mock.baseUrl}/api/team/workspace/file`, {
      method: "PUT",
      body: JSON.stringify({ path: "workbench/tip-splitter.state.json", content: "{}" }),
    });
    const deleted = await fetchJson(`${mock.baseUrl}/api/team/workbench/artifacts/tip-splitter`, {
      method: "DELETE",
    });
    expect(deleted.body).toEqual({
      removed: ["tip-splitter.html", "tip-splitter.state.json"],
      checkpoint_id: null,
    });
    const after = await fetchJson(`${mock.baseUrl}/api/team/workbench/artifacts`);
    expect(after.body).toEqual([]);
  });
});
