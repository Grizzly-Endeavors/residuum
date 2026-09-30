import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type {
  CheckpointDetail,
  CheckpointPage,
  CheckpointSummary,
  RepoStats,
  RestoreOutcome,
  UndoOutcome,
} from "../src/lib/generated/protocol";
import { repoStats } from "./checkpoints";
import { fetchJson, fetchText, startMockServer, type MockServerHarness } from "./test-support";

/** The checkpoint routes through the whole mock, since they are served under two scopes. */
describe("the checkpoint routes", () => {
  let mock: MockServerHarness;

  beforeEach(async () => {
    mock = await startMockServer({ deterministic: true });
  });

  afterEach(async () => {
    await mock.close();
  });

  const agent = (path: string): string => `${mock.baseUrl}/api/agents/scout/checkpoints${path}`;
  const hub = (path: string): string => `${mock.baseUrl}/api/hub/checkpoints${path}`;
  const post = (url: string, body: unknown): Promise<{ status: number; body: unknown }> =>
    fetchJson(url, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
  const scout = (): ReturnType<MockServerHarness["hub"]["agents"]["get"]> =>
    mock.hub.agents.get("scout");
  const list = async (url: string): Promise<CheckpointSummary[]> =>
    ((await fetchJson(url)).body as CheckpointPage).items;

  describe("listing", () => {
    it("lists an agent's workspace checkpoints newest first", async () => {
      const { status, body } = await fetchJson(agent("?repo=workspace"));
      expect(status).toBe(200);
      const page = body as CheckpointPage;
      expect(page.items.map((c) => [c.trigger, c.summary])).toEqual([
        ["pre_action", "delete scratch/notes.md"],
        ["turn_end", "updated SOUL.md"],
        ["turn_start", "edits made outside a turn"],
      ]);
      expect(page.next_cursor).toBeNull();
      for (const item of page.items) expect(item.id).toMatch(/^[0-9a-f]{40}$/);
    });

    it("serves the workspace and agent config repositories to an agent, and hub and team to the hub", async () => {
      expect((await list(agent("?repo=agent_config"))).map((c) => c.summary)).toEqual([
        "config patch",
        "config patch",
      ]);
      expect((await list(hub("?repo=team"))).length).toBe(3);
      expect((await list(hub("?repo=hub"))).length).toBe(1);
    });

    it("answers 400 for a repository that belongs to the other scope, or to none", async () => {
      expect(await fetchText(agent("?repo=hub"))).toEqual({
        status: 400,
        body: "this route serves the workspace or agentconfig checkpoint repositories only",
      });
      expect(await fetchText(hub("?repo=workspace"))).toEqual({
        status: 400,
        body: "this route serves the hub or team checkpoint repositories only",
      });
      expect((await fetchText(agent(""))).status).toBe(400);
      expect((await fetchText(agent("?repo=nonsense"))).status).toBe(400);
    });

    it("keeps each agent's history its own", async () => {
      const scoutIds = (await list(agent("?repo=workspace"))).map((c) => c.id);
      const atlas = await list(`${mock.baseUrl}/api/agents/atlas/checkpoints?repo=workspace`);
      expect(atlas.map((c) => c.id).filter((id) => scoutIds.includes(id))).toEqual([]);
    });

    it("lists a stopped agent's checkpoints, which are a repair route", async () => {
      const res = await fetchJson(`${mock.baseUrl}/api/agents/drifter/checkpoints?repo=workspace`);
      expect(res.status).toBe(200);
    });

    it("filters to the checkpoints that changed a path, or a turn's pair", async () => {
      expect((await list(agent("?repo=workspace&path=SOUL.md"))).map((c) => c.trigger)).toEqual([
        "turn_end",
        "turn_start",
      ]);
      expect((await list(agent("?repo=workspace&path=scratch"))).map((c) => c.trigger)).toEqual([
        "turn_start",
      ]);
      expect(
        (await list(agent("?repo=workspace&turn_id=turn-0001"))).map((c) => c.trigger),
      ).toEqual(["turn_end", "turn_start"]);
    });

    it("pages with an opaque cursor, and refuses one it didn't issue", async () => {
      const first = (await fetchJson(agent("?repo=workspace&limit=2"))).body as CheckpointPage;
      expect(first.items).toHaveLength(2);
      expect(first.next_cursor).toBe(first.items[1]?.id);
      const second = (
        await fetchJson(agent(`?repo=workspace&limit=2&before=${first.next_cursor ?? ""}`))
      ).body as CheckpointPage;
      expect(second.items.map((c) => c.trigger)).toEqual(["turn_start"]);
      expect(second.next_cursor).toBeNull();
      expect(await fetchText(agent("?repo=workspace&before=nope"))).toEqual({
        status: 404,
        body: "invalid page cursor",
      });
    });
  });

  describe("stats", () => {
    it("reports the size, count and oldest checkpoint, and the status route agrees", async () => {
      const { body } = await fetchJson(agent("/stats?repo=workspace"));
      const stats = body as RepoStats;
      expect(stats.checkpoint_count).toBe(3);
      expect(stats.on_disk_bytes).toBeGreaterThan(16_384);
      const oldest = (await list(agent("?repo=workspace"))).at(-1)?.timestamp;
      expect(stats.oldest).toBe(oldest);
      const status = (await fetchJson(`${mock.baseUrl}/api/agents/scout/status`)).body as {
        checkpoints: { workspace: RepoStats; team: RepoStats };
      };
      expect(status.checkpoints.workspace).toEqual(stats);
      expect(status.checkpoints.team).toEqual(
        (await fetchJson(hub("/stats?repo=team"))).body as RepoStats,
      );
    });

    it("has no oldest checkpoint for a repository with none", () => {
      const state = scout()?.state;
      if (state === undefined) throw new Error("scout is missing");
      state.checkpoints.workspace = [];
      expect(repoStats(state, "workspace")).toEqual({
        on_disk_bytes: 16_384,
        checkpoint_count: 0,
        oldest: null,
      });
    });
  });

  describe("a checkpoint", () => {
    const newestId = async (): Promise<string> =>
      (await list(agent("?repo=workspace")))[0]?.id ?? "";

    it("shows its summary and the paths it changed", async () => {
      const id = await newestId();
      const { status, body } = await fetchJson(agent(`/${id}?repo=workspace`));
      expect(status).toBe(200);
      const detail = body as CheckpointDetail;
      expect(detail.summary.id).toBe(id);
      expect(detail.changed_paths).toEqual([{ path: "HEARTBEAT.yml", kind: "modified" }]);
      expect(detail.summary.changed_path_count).toBe(1);
    });

    it("lists everything the first checkpoint added", async () => {
      const first = (await list(agent("?repo=workspace"))).at(-1);
      const detail = (await fetchJson(agent(`/${first?.id ?? ""}?repo=workspace`)))
        .body as CheckpointDetail;
      expect(detail.changed_paths.every((c) => c.kind === "added")).toBe(true);
      expect(detail.changed_paths.map((c) => c.path)).toContain("scratch/notes.md");
    });

    it("finds a checkpoint by an id prefix, and answers 404 for one it doesn't have", async () => {
      const id = await newestId();
      expect((await fetchJson(agent(`/${id.slice(0, 8)}?repo=workspace`))).status).toBe(200);
      expect(await fetchText(agent("/ffffffff?repo=workspace"))).toEqual({
        status: 404,
        body: "no checkpoint found matching 'ffffffff'",
      });
    });

    it("diffs one file against the checkpoint before it, and gives null for a file it didn't change", async () => {
      const id = await newestId();
      const changed = (await fetchJson(agent(`/${id}/diff?repo=workspace&path=HEARTBEAT.yml`)))
        .body as { diff: string };
      expect(changed.diff).toMatch(/^--- a\/HEARTBEAT\.yml\n\+\+\+ b\/HEARTBEAT\.yml\n@@ /);
      expect(changed.diff).toContain("\n+");
      expect(await fetchJson(agent(`/${id}/diff?repo=workspace&path=SOUL.md`))).toEqual({
        status: 200,
        body: { diff: null },
      });
    });

    it("reads a file as it was at the checkpoint, and answers 404 for one that wasn't there", async () => {
      const id = await newestId();
      const soul = await fetchText(agent(`/${id}/file?repo=workspace&path=SOUL.md`));
      expect(soul.status).toBe(200);
      expect(soul.body).toBe(mock.hub.agents.get("scout")?.state.workspaceFileContents["SOUL.md"]);
      expect(await fetchText(agent(`/${id}/file?repo=workspace&path=nope.md`))).toEqual({
        status: 404,
        body: "nope.md is not a file at this checkpoint",
      });
    });

    it("answers a file or diff request without a path with 400", async () => {
      const id = await newestId();
      expect((await fetchText(agent(`/${id}/file?repo=workspace`))).status).toBe(400);
      expect((await fetchText(agent(`/${id}/diff?repo=workspace`))).status).toBe(400);
    });
  });

  describe("restoring", () => {
    it("writes a file back from a checkpoint, records the restore, and lists the paths", async () => {
      const state = scout()?.state;
      if (state === undefined) throw new Error("scout is missing");
      const current = state.workspaceFileContents["SOUL.md"];
      const first = (await list(agent("?repo=workspace"))).at(-1);

      const res = await post(agent(`/${first?.id ?? ""}/restore`), {
        repo: "workspace",
        path: "SOUL.md",
      });
      expect(res.status).toBe(200);
      const outcome = res.body as RestoreOutcome;
      expect(outcome.restored_paths).toEqual(["SOUL.md"]);
      expect(state.workspaceFileContents["SOUL.md"]).not.toBe(current);

      const newest = (await list(agent("?repo=workspace")))[0];
      expect(newest).toMatchObject({ id: outcome.checkpoint_id, trigger: "restore" });
    });

    it("brings back a deleted file, which is what Undo does after a delete", async () => {
      const state = scout()?.state;
      if (state === undefined) throw new Error("scout is missing");
      expect(state.workspaceFileContents["scratch/notes.md"]).toBeUndefined();
      const deleteCheckpoint = (await list(agent("?repo=workspace")))[0];
      const res = await post(agent(`/${deleteCheckpoint?.id ?? ""}/restore`), {
        repo: "workspace",
        path: "scratch/notes.md",
      });
      expect((res.body as RestoreOutcome).restored_paths).toEqual(["scratch/notes.md"]);
      expect(state.workspaceFileContents["scratch/notes.md"]).toContain("Scratch notes");
      expect(
        (await fetchJson(`${mock.baseUrl}/api/agents/scout/workspace/files?path=scratch`)).status,
      ).toBe(200);
    });

    it("restores a directory, removing what the checkpoint didn't have under it", async () => {
      const state = scout()?.state;
      if (state === undefined) throw new Error("scout is missing");
      state.workspaceFileContents["skills/research/extra.md"] = "new";
      const newest = (await list(agent("?repo=workspace")))[0];
      const res = await post(agent(`/${newest?.id ?? ""}/restore`), {
        repo: "workspace",
        path: "skills/research",
      });
      const restored = (res.body as RestoreOutcome).restored_paths;
      expect(restored).toContain("skills/research/SKILL.md");
      expect(state.workspaceFileContents["skills/research/extra.md"]).toBeUndefined();
    });

    it("restores a config file in the agent config repository", async () => {
      const state = scout()?.state;
      if (state === undefined) throw new Error("scout is missing");
      const before = state.configToml;
      const oldest = (await list(agent("?repo=agent_config"))).at(-1);
      await post(agent(`/${oldest?.id ?? ""}/restore`), {
        repo: "agent_config",
        path: "config.toml",
      });
      expect(state.configToml).not.toBe(before);
      expect(before.startsWith(state.configToml.trimEnd())).toBe(true);
    });

    it("restores a team file through the hub, as the team tree shows it", async () => {
      const first = (await list(hub("?repo=team"))).at(-1);
      const res = await post(hub(`/${first?.id ?? ""}/restore`), {
        repo: "team",
        path: "AGENTS.md",
      });
      expect((res.body as RestoreOutcome).restored_paths).toEqual(["AGENTS.md"]);
      const read = await fetchText(`${mock.baseUrl}/api/team/workspace/file?path=AGENTS.md`);
      expect(read.status).toBe(200);
      expect(read.body.length).toBeLessThan(
        (mock.hub.hubState.workspaceFileContents["team/AGENTS.md"] ?? "").length + 1,
      );
    });

    it("answers 404 for a path the checkpoint doesn't hold, and 422 for a body without one", async () => {
      const id = (await list(agent("?repo=workspace")))[0]?.id ?? "";
      expect(
        await fetchText(agent(`/${id}/restore`), {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ repo: "workspace", path: "nope.md" }),
        }),
      ).toEqual({ status: 404, body: `path 'nope.md' not found at checkpoint '${id}'` });
      expect(
        (
          await fetchText(agent(`/${id}/restore`), {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({ repo: "workspace" }),
          })
        ).status,
      ).toBe(422);
    });
  });

  describe("undoing", () => {
    it("reverts what a checkpoint changed and records the undo", async () => {
      const state = scout()?.state;
      if (state === undefined) throw new Error("scout is missing");
      const current = state.workspaceFileContents["SOUL.md"];
      const turnEnd = (await list(agent("?repo=workspace"))).find((c) => c.trigger === "turn_end");
      const res = await post(agent(`/${turnEnd?.id ?? ""}/undo`), { repo: "workspace" });
      expect(res.status).toBe(200);
      const outcome = res.body as UndoOutcome;
      expect(outcome).toMatchObject({ reverted_paths: ["SOUL.md"], skipped_paths: [] });
      expect(state.workspaceFileContents["SOUL.md"]).not.toBe(current);
      expect((await list(agent("?repo=workspace")))[0]).toMatchObject({
        id: outcome.checkpoint_id,
        trigger: "undo",
      });
    });

    it("skips a path that changed again since, rather than losing the later edit", async () => {
      const state = scout()?.state;
      if (state === undefined) throw new Error("scout is missing");
      state.workspaceFileContents["SOUL.md"] = "edited afterwards";
      const turnEnd = (await list(agent("?repo=workspace"))).find((c) => c.trigger === "turn_end");
      const res = await post(agent(`/${turnEnd?.id ?? ""}/undo`), { repo: "workspace" });
      expect(res.body).toMatchObject({ reverted_paths: [], skipped_paths: ["SOUL.md"] });
      expect(state.workspaceFileContents["SOUL.md"]).toBe("edited afterwards");
    });

    it("can undo a restore", async () => {
      const state = scout()?.state;
      if (state === undefined) throw new Error("scout is missing");
      const current = state.workspaceFileContents["SOUL.md"];
      const first = (await list(agent("?repo=workspace"))).at(-1);
      const restored = (
        await post(agent(`/${first?.id ?? ""}/restore`), { repo: "workspace", path: "SOUL.md" })
      ).body as RestoreOutcome;
      await post(agent(`/${restored.checkpoint_id}/undo`), { repo: "workspace" });
      expect(state.workspaceFileContents["SOUL.md"]).toBe(current);
    });
  });

  it("seeds the same checkpoints on every run of a deterministic mock", async () => {
    const other = await startMockServer({ deterministic: true });
    try {
      const again = await fetchJson(`${other.baseUrl}/api/agents/scout/checkpoints?repo=workspace`);
      expect(again.body).toEqual((await fetchJson(agent("?repo=workspace"))).body);
    } finally {
      await other.close();
    }
  });
});
