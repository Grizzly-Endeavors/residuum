import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { RepoKind, RestoreOutcome } from "../src/lib/generated/protocol";
import type { ArtifactSummary, WorkbenchInfo } from "../src/lib/types";
import { MOCK_WORKBENCH_ARTIFACT } from "./data/workbench";
import { createMockEnv } from "./env";
import {
  fetchJson,
  fetchText,
  startMockServer,
  startRouteHarness,
  type MockServerHarness,
  type RouteHarness,
} from "./test-support";
import { workbenchRoutes } from "./workbench";
import { listDirectory, removePath, writeFile } from "./workspace-tree";

describe("workbench routes", () => {
  let harness: RouteHarness;

  beforeEach(async () => {
    harness = await startRouteHarness(workbenchRoutes, createMockEnv({ deterministic: true }));
  });

  afterEach(async () => {
    await harness.close();
  });

  const url = (path: string): string => `${harness.baseUrl}/api/workbench${path}`;
  const team = (): RouteHarness["hub"]["hubState"] => harness.hub.hubState;

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
        modified_at: "2026-03-14T12:00:00.000Z",
        size: Buffer.byteLength(MOCK_WORKBENCH_ARTIFACT),
      });
    });

    it("lists the most recently modified artifact first, then by name", async () => {
      const page = "<title>x</title>";
      writeFile(team(), "team/workbench/same-time.html", page);
      harness.hub.env.clock.advance(60_000);
      writeFile(team(), "team/workbench/b-newest.html", page);
      writeFile(team(), "team/workbench/a-newest.html", page);
      expect((await artifacts()).map((artifact) => artifact.name)).toEqual([
        "a-newest",
        "b-newest",
        "same-time",
        "tip-splitter",
      ]);
    });

    it("titles a page by its first <title>, decoded and tidy, and by its name when it has none", async () => {
      removePath(team(), "team/workbench/tip-splitter.html");
      writeFile(
        team(),
        "team/workbench/spaced.html",
        "<head><TITLE lang=en>\n  Tom &amp; Jerry's\n   &quot;Chart&quot; </TITLE></head><title>second</title>",
      );
      writeFile(team(), "team/workbench/blank.html", "<title>  </title>");
      writeFile(team(), "team/workbench/bare.html", "<p>no title</p>");
      const titles = Object.fromEntries((await artifacts()).map((a) => [a.name, a.title]));
      expect(titles).toEqual({
        spaced: 'Tom & Jerry\'s "Chart"',
        blank: "blank",
        bare: "bare",
      });
    });

    it("measures a page in bytes", async () => {
      removePath(team(), "team/workbench/tip-splitter.html");
      writeFile(team(), "team/workbench/accents.html", "<title>é</title>");
      expect((await artifacts())[0]?.size).toBe(17);
    });

    it("lists nothing for a workbench with no artifacts", async () => {
      removePath(team(), "team/workbench/tip-splitter.html");
      expect(await artifacts()).toEqual([]);
    });

    it("lists a folder with an index page, sized and dated by every file in it", async () => {
      removePath(team(), "team/workbench/tip-splitter.html");
      writeFile(team(), "team/workbench/graph/index.html", "<title>Graph</title>");
      harness.hub.env.clock.advance(5_000);
      writeFile(team(), "team/workbench/graph/js/app.js", "let x = 1;");
      expect(await artifacts()).toEqual([
        {
          name: "graph",
          title: "Graph",
          modified_at: "2026-03-14T12:00:05.000Z",
          size: Buffer.byteLength("<title>Graph</title>let x = 1;"),
        },
      ]);
    });

    it("lets a folder win over a page of the same name", async () => {
      writeFile(team(), "team/workbench/graph.html", "<title>Page</title>");
      writeFile(team(), "team/workbench/graph/index.html", "<title>Folder</title>");
      const titles = Object.fromEntries((await artifacts()).map((a) => [a.name, a.title]));
      expect(titles.graph).toBe("Folder");
    });

    it("lists only pages and folders with an index page, under names an artifact can have", async () => {
      removePath(team(), "team/workbench/tip-splitter.html");
      for (const path of [
        "team/workbench/no-index/app.js",
        "team/workbench/notes.txt",
        "team/workbench/chart.state.json",
        "team/workbench/Bad.html",
        "team/workbench/double--hyphen.html",
        "team/workbench/double--hyphen/index.html",
        "team/workbench/chart.html",
      ]) {
        writeFile(team(), path, "<title>x</title>");
      }
      expect((await artifacts()).map((artifact) => artifact.name)).toEqual(["chart"]);
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
    const newestTeamCheckpoint = (): { id: string; summary: string; trigger: string } => {
      const newest = team().checkpoints.team?.at(-1)?.summary;
      if (newest === undefined) throw new Error("the team has no checkpoints");
      return { id: newest.id, summary: newest.summary, trigger: newest.trigger };
    };

    it("removes the artifact and names the team checkpoint taken just before", async () => {
      const res = await fetchJson(url("/artifacts/tip-splitter"), { method: "DELETE" });
      const { id, summary, trigger } = newestTeamCheckpoint();
      expect(res).toEqual({
        status: 200,
        body: { removed: ["tip-splitter.html"], checkpoint_id: id },
      });
      expect(summary).toBe("delete workbench artifact tip-splitter");
      expect(trigger).toBe("pre_action");
      expect(await artifacts()).toEqual([]);
    });

    it("records the tree as it was before the delete", async () => {
      await fetchJson(url("/artifacts/tip-splitter"), { method: "DELETE" });
      const recorded = team().checkpoints.team?.at(-1)?.files;
      expect(recorded?.["workbench/tip-splitter.html"]).toBe(MOCK_WORKBENCH_ARTIFACT);
      expect(team().workspaceFileContents["team/workbench/tip-splitter.html"]).toBeUndefined();
    });

    it("removes the artifact's data files from the team workbench folder, and only those", async () => {
      writeFile(team(), "team/workbench/tip-splitter.state.json", '{"bill":84}');
      writeFile(team(), "team/workbench/tip-splitter.log.txt", "x");
      writeFile(team(), "team/workbench/tip-splitter-2.state.json", "{}");
      writeFile(team(), "team/workbench/other.state.json", "{}");

      const res = await fetchJson(url("/artifacts/tip-splitter"), { method: "DELETE" });

      expect(res.body).toEqual({
        removed: ["tip-splitter.html", "tip-splitter.log.txt", "tip-splitter.state.json"],
        checkpoint_id: newestTeamCheckpoint().id,
      });
      const left = (listDirectory(team(), "team/workbench") ?? []).map((entry) => entry.name);
      expect(left).toEqual(["other.state.json", "tip-splitter-2.state.json"]);
      expect(
        team().workspaceFileContents["team/workbench/tip-splitter.state.json"],
      ).toBeUndefined();
    });

    it("removes a folder artifact whole, and reports it as name/", async () => {
      writeFile(team(), "team/workbench/graph/index.html", "<title>Graph</title>");
      writeFile(team(), "team/workbench/graph/js/app.js", "let x = 1;");
      writeFile(team(), "team/workbench/graph.state.json", "{}");

      const res = await fetchJson(url("/artifacts/graph"), { method: "DELETE" });

      expect(res.body).toEqual({
        removed: ["graph.state.json", "graph/"],
        checkpoint_id: newestTeamCheckpoint().id,
      });
      expect(Object.keys(team().workspaceFileContents).filter((p) => p.includes("graph"))).toEqual(
        [],
      );
      expect(team().workspaceFiles["team/workbench/graph/js"]).toBeUndefined();
    });

    it("answers 404 for an artifact that is already gone, and records nothing", async () => {
      await fetchJson(url("/artifacts/tip-splitter"), { method: "DELETE" });
      const recorded = team().checkpoints.team?.length;
      expect(await fetchText(url("/artifacts/tip-splitter"), { method: "DELETE" })).toEqual({
        status: 404,
        body: "That artifact no longer exists. It may already have been deleted.",
      });
      expect(team().checkpoints.team).toHaveLength(recorded ?? -1);
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
    mock = await startMockServer({ deterministic: true });
  });

  afterEach(async () => {
    await mock.close();
  });

  const team = (): MockServerHarness["hub"]["hubState"] => mock.hub.hubState;

  const post = (path: string, body: unknown): Promise<{ status: number; body: unknown }> =>
    fetchJson(`${mock.baseUrl}${path}`, { method: "POST", body: JSON.stringify(body) });

  const remove = (name: string): Promise<{ status: number; body: unknown }> =>
    fetchJson(`${mock.baseUrl}/api/team/workbench/artifacts/${name}`, { method: "DELETE" });

  const listed = async (): Promise<string[]> =>
    (
      (await fetchJson(`${mock.baseUrl}/api/team/workbench/artifacts`)).body as ArtifactSummary[]
    ).map((artifact) => artifact.name);

  /** What Undo does: restore each path the delete removed, from the checkpoint it returned. */
  async function undo(deleted: { removed: string[]; checkpoint_id: string }): Promise<void> {
    for (const entry of deleted.removed) {
      const restored = await post(`/api/hub/checkpoints/${deleted.checkpoint_id}/restore`, {
        repo: "team" satisfies RepoKind,
        path: `workbench/${entry.replace(/\/$/, "")}`,
      });
      expect(restored.status, entry).toBe(200);
    }
  }

  it("serves the team's workbench from the shared state", async () => {
    expect(await listed()).toEqual(["tip-splitter"]);

    await fetchJson(`${mock.baseUrl}/api/team/workspace/file`, {
      method: "PUT",
      body: JSON.stringify({ path: "workbench/tip-splitter.state.json", content: "{}" }),
    });
    const deleted = await remove("tip-splitter");
    expect(deleted.body).toEqual({
      removed: ["tip-splitter.html", "tip-splitter.state.json"],
      checkpoint_id: expect.stringMatching(/^[0-9a-f]{40}$/) as unknown,
    });
    expect(await listed()).toEqual([]);
  });

  it("brings the page and its data files back when the delete is undone", async () => {
    writeFile(team(), "team/workbench/tip-splitter.state.json", '{"bill":84}');
    writeFile(team(), "team/workbench/tip-splitter.log.txt", "saved");
    const deleted = (await remove("tip-splitter")).body as {
      removed: string[];
      checkpoint_id: string;
    };
    expect(await listed()).toEqual([]);

    await undo(deleted);

    expect(await listed()).toEqual(["tip-splitter"]);
    const files = team().workspaceFileContents;
    expect(files["team/workbench/tip-splitter.html"]).toBe(MOCK_WORKBENCH_ARTIFACT);
    expect(files["team/workbench/tip-splitter.state.json"]).toBe('{"bill":84}');
    expect(files["team/workbench/tip-splitter.log.txt"]).toBe("saved");
  });

  it("brings a folder artifact back with every file in it", async () => {
    writeFile(team(), "team/workbench/graph/index.html", "<title>Graph</title>");
    writeFile(team(), "team/workbench/graph/js/app.js", "let x = 1;");
    const deleted = (await remove("graph")).body as { removed: string[]; checkpoint_id: string };
    expect(deleted.removed).toEqual(["graph/"]);

    await undo(deleted);

    expect(await listed()).toEqual(["graph", "tip-splitter"]);
    expect(team().workspaceFileContents["team/workbench/graph/js/app.js"]).toBe("let x = 1;");
  });

  it("checkpoints a delete only when the team changed since its newest checkpoint", async () => {
    const first = (await remove("tip-splitter")).body as {
      removed: string[];
      checkpoint_id: string;
    };
    const afterFirst = team().checkpoints.team?.length;
    await undo(first);
    const afterUndo = team().checkpoints.team?.length;
    expect(afterUndo).toBe((afterFirst ?? 0) + 1);

    // The undo left the newest checkpoint holding exactly the tree now on disk.
    const second = (await remove("tip-splitter")).body as { checkpoint_id: string };
    expect(second.checkpoint_id).toBe(team().checkpoints.team?.at(-1)?.summary.id);
    expect(team().checkpoints.team).toHaveLength(afterUndo ?? 0);

    const restored = await post(`/api/hub/checkpoints/${second.checkpoint_id}/restore`, {
      repo: "team",
      path: "workbench/tip-splitter.html",
    });
    expect(restored.status).toBe(200);
    expect((restored.body as RestoreOutcome).restored_paths).toEqual([
      "workbench/tip-splitter.html",
    ]);
  });
});
