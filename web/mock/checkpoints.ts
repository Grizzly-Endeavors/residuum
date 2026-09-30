import { createHash } from "node:crypto";
import type {
  ChangedPath,
  CheckpointDetail,
  CheckpointPage,
  CheckpointSummary,
  CheckpointTrigger,
  RepoKind,
  RepoStats,
  RestoreOutcome,
  UndoOutcome,
} from "../src/lib/generated/protocol";
import { HUB_STATE_NAME } from "./constants";
import { json, readJsonObject, stringField, text, type JsonObject } from "./http";
import { decodedParam, type Route, type RouteContext } from "./routes";
import type { MockState } from "./state";
import { removePath, writeFile } from "./workspace-tree";

/**
 * Checkpoint histories, for the checkpoint routes. Each repository keeps every
 * checkpoint's whole tree, so diffs, file reads, restores and undos are
 * answered from the trees the way the backend answers them from git. An
 * agent's state holds its `workspace` and `agent_config` repositories, and the
 * hub's state the `hub` and `team` ones.
 */
export type MockCheckpoints = Partial<Record<RepoKind, MockCheckpoint[]>>;

/** A path relative to a repository's root, to its content. */
type Files = Record<string, string>;

export interface MockCheckpoint {
  summary: CheckpointSummary;
  /** The repository's tree as of this checkpoint. */
  files: Files;
}

const AGENT_REPOS: readonly RepoKind[] = ["workspace", "agent_config"];
const HUB_REPOS: readonly RepoKind[] = ["hub", "team"];
const TEAM_PREFIX = "team/";
const DEFAULT_PAGE_SIZE = 50;
const MAX_PAGE_SIZE = 200;
const DAY_MS = 86_400_000;

/** The repositories a scope's routes serve: the hub's for the hub's state, an agent's otherwise. */
function reposServed(ctx: RouteContext): readonly RepoKind[] {
  return ctx.state.agentName === HUB_STATE_NAME ? HUB_REPOS : AGENT_REPOS;
}

// ─── The live trees ────────────────────────────────────────────────────────────

/** The files a repository tracks as they are now. */
function liveFiles(state: MockState, repo: RepoKind): Files {
  const contents = Object.entries(state.workspaceFileContents);
  switch (repo) {
    case "workspace":
      return Object.fromEntries(contents.filter(([path]) => !path.startsWith(TEAM_PREFIX)));
    case "team":
      return Object.fromEntries(
        contents
          .filter(([path]) => path.startsWith(TEAM_PREFIX))
          .map(([path, content]) => [path.slice(TEAM_PREFIX.length), content]),
      );
    case "agent_config":
      return { "config.toml": state.configToml, "providers.toml": state.providersToml };
    case "hub":
      return { "config.toml": state.hubConfigToml };
  }
}

/** Write a file of a repository back to disk, or remove it when `content` is `null`. */
function writeLive(state: MockState, repo: RepoKind, path: string, content: string | null): void {
  const inTree = repo === "team" ? `${TEAM_PREFIX}${path}` : path;
  switch (repo) {
    case "workspace":
    case "team":
      if (content === null) removePath(state, inTree);
      else writeFile(state, inTree, content);
      return;
    case "agent_config":
      if (path === "config.toml") state.configToml = content ?? "";
      else state.providersToml = content ?? "";
      return;
    case "hub":
      state.hubConfigToml = content ?? "";
  }
}

// ─── Recording ─────────────────────────────────────────────────────────────────

/** Every path whose content differs between two trees. */
function changedPaths(before: Files, after: Files): ChangedPath[] {
  const paths = [...new Set([...Object.keys(before), ...Object.keys(after)])].sort();
  return paths.flatMap((path): ChangedPath[] => {
    const was = before[path];
    const now = after[path];
    if (was === now) return [];
    if (was === undefined) return [{ path, kind: "added" }];
    return [{ path, kind: now === undefined ? "deleted" : "modified" }];
  });
}

interface Recorded {
  trigger: CheckpointTrigger;
  summary: string;
  address?: string;
  turnId?: string;
  timestamp?: string;
}

function checkpointId(state: MockState, repo: RepoKind, index: number): string {
  return createHash("sha1")
    .update(`${state.agentName}:${repo}:${String(index)}`)
    .digest("hex");
}

/** Record `files` as the repository's newest checkpoint. */
function record(state: MockState, repo: RepoKind, files: Files, meta: Recorded): MockCheckpoint {
  const history = (state.checkpoints[repo] ??= []);
  const previous = history.at(-1)?.files ?? {};
  const checkpoint: MockCheckpoint = {
    files,
    summary: {
      id: checkpointId(state, repo, history.length),
      timestamp: meta.timestamp ?? state.env.clock.iso(),
      address: meta.address ?? "system",
      run_id: null,
      turn_id: meta.turnId ?? null,
      trigger: meta.trigger,
      summary: meta.summary,
      changed_path_count: changedPaths(previous, files).length,
    },
  };
  history.push(checkpoint);
  return checkpoint;
}

/**
 * The id of the checkpoint that holds `repo` as it is now, which an action is
 * about to change: a new one, unless the newest already holds the same tree.
 * Undo restores from it.
 */
export function checkpointBeforeAction(state: MockState, repo: RepoKind, summary: string): string {
  const files = liveFiles(state, repo);
  const newest = state.checkpoints[repo]?.at(-1);
  if (newest !== undefined && changedPaths(newest.files, files).length === 0) {
    return newest.summary.id;
  }
  return record(state, repo, files, { trigger: "pre_action", summary }).summary.id;
}

/** What `text` was before its last three lines, for the earlier version of a sample file. */
function earlier(content: string | undefined): string {
  return `${(content ?? "").split("\n").slice(0, -3).join("\n")}\n`;
}

/** Where each repository's sample history touches the tree, besides its config files. */
const SAMPLE_PATHS = {
  workspace: { edited: "SOUL.md", later: "HEARTBEAT.yml", scratch: "scratch/notes.md" },
  team: { edited: "AGENTS.md", later: "USER.md", scratch: "wiki/scratch.md" },
} as const;

/**
 * Give a state the histories of the repositories it holds. Each ends where
 * the live tree is, or one write short of it, as a pre-write checkpoint does.
 */
export function seedCheckpoints(state: MockState): void {
  const { clock } = state.env;
  const daysAgo = (days: number): string => clock.isoAgo(days * DAY_MS);
  const turn = { address: "main", turnId: "turn-0001" };

  for (const repo of state.agentName === HUB_STATE_NAME ? HUB_REPOS : AGENT_REPOS) {
    const live = liveFiles(state, repo);
    if (repo === "workspace" || repo === "team") {
      const { edited, later, scratch } = SAMPLE_PATHS[repo];
      const withScratch = { ...live, [scratch]: "Scratch notes from the last run.\n" };
      const first = {
        ...withScratch,
        [edited]: earlier(live[edited]),
        [later]: earlier(live[later]),
      };
      record(state, repo, first, {
        ...turn,
        trigger: "turn_start",
        summary: "edits made outside a turn",
        timestamp: clock.isoAgo(3 * DAY_MS + 600_000),
      });
      const second = { ...first, [edited]: live[edited] ?? "" };
      record(state, repo, second, {
        ...turn,
        trigger: "turn_end",
        summary: `updated ${edited}`,
        timestamp: daysAgo(3),
      });
      record(
        state,
        repo,
        { ...withScratch, [later]: live[later] ?? "" },
        {
          address: "web",
          trigger: "pre_action",
          summary: `delete ${scratch}`,
          timestamp: daysAgo(1),
        },
      );
    } else {
      const older = Object.fromEntries(Object.entries(live).map(([p, c]) => [p, earlier(c)]));
      const meta = {
        address: "web",
        trigger: "pre_config_write",
        summary: "config patch",
      } as const;
      record(state, repo, older, { ...meta, timestamp: daysAgo(6) });
      if (repo === "agent_config") {
        record(
          state,
          repo,
          { ...older, "config.toml": live["config.toml"] ?? "" },
          {
            ...meta,
            timestamp: daysAgo(2),
          },
        );
      }
    }
  }
}

// ─── Reading ───────────────────────────────────────────────────────────────────

/** The repository's size and history, as the stats route and `status` report them. */
export function repoStats(state: MockState, repo: RepoKind): RepoStats {
  const history = state.checkpoints[repo] ?? [];
  let bytes = 16_384;
  let previous: Files = {};
  for (const { files } of history) {
    for (const { path } of changedPaths(previous, files)) {
      bytes += Buffer.byteLength(files[path] ?? "") + 600;
    }
    previous = files;
  }
  return {
    on_disk_bytes: bytes,
    checkpoint_count: history.length,
    oldest: history[0]?.summary.timestamp ?? null,
  };
}

/** A minimal unified diff of one file: the changed lines, with up to three lines of context. */
function unifiedDiff(path: string, before: string | undefined, after: string | undefined): string {
  const lines = (content: string | undefined): string[] =>
    content === undefined ? [] : content.replace(/\n$/, "").split("\n");
  const [a, b] = [lines(before), lines(after)];
  let start = 0;
  while (start < a.length && start < b.length && a[start] === b[start]) start++;
  let endA = a.length;
  let endB = b.length;
  while (endA > start && endB > start && a[endA - 1] === b[endB - 1]) {
    endA--;
    endB--;
  }
  const from = Math.max(0, start - 3);
  const to = Math.min(a.length, endA + 3);
  const body = [
    ...a.slice(from, start).map((line) => ` ${line}`),
    ...a.slice(start, endA).map((line) => `-${line}`),
    ...b.slice(start, endB).map((line) => `+${line}`),
    ...a.slice(endA, to).map((line) => ` ${line}`),
  ];
  const newLength = to - from - (endA - start) + (endB - start);
  return [
    `--- ${before === undefined ? "/dev/null" : `a/${path}`}`,
    `+++ ${after === undefined ? "/dev/null" : `b/${path}`}`,
    `@@ -${String(from + 1)},${String(to - from)} +${String(from + 1)},${String(newLength)} @@`,
    ...body,
    "",
  ].join("\n");
}

// ─── Routes ────────────────────────────────────────────────────────────────────

/** A checkpoint the route names, with the tree before it. */
interface Found {
  history: MockCheckpoint[];
  at: number;
}

/** The `repo` query parameter of a read, or `null` after answering `400`. */
function repoOf(ctx: RouteContext, raw: string | null): RepoKind | null {
  if (raw === null) {
    text(ctx.res, 400, "Failed to deserialize query string: missing field `repo`");
    return null;
  }
  const known: readonly string[] = [...AGENT_REPOS, ...HUB_REPOS];
  if (!known.includes(raw)) {
    text(
      ctx.res,
      400,
      `Failed to deserialize query string: unknown variant \`${raw}\`, expected one of \`workspace\`, \`team\`, \`agent_config\`, \`hub\``,
    );
    return null;
  }
  const repo = raw as RepoKind;
  const served = reposServed(ctx);
  if (!served.includes(repo)) {
    const names = served.map((kind) => kind.replace("_", "")).join(" or ");
    text(ctx.res, 400, `this route serves the ${names} checkpoint repositories only`);
    return null;
  }
  return repo;
}

/** The checkpoint `id` (or an id prefix) names, or `null` after answering `404`. */
function find(ctx: RouteContext, repo: RepoKind, id: string): Found | null {
  const history = ctx.state.checkpoints[repo] ?? [];
  const at = history.findIndex((c) => c.summary.id.startsWith(id));
  if (id === "" || at === -1) {
    text(ctx.res, 404, `no checkpoint found matching '${id}'`);
    return null;
  }
  return { history, at };
}

function changesAt({ history, at }: Found): ChangedPath[] {
  return changedPaths(history[at - 1]?.files ?? {}, history[at]?.files ?? {});
}

function pathTouches(changes: ChangedPath[], path: string): boolean {
  return changes.some((c) => c.path === path || c.path.startsWith(`${path.replace(/\/$/, "")}/`));
}

function listCheckpoints(ctx: RouteContext): void {
  const { query, res, state } = ctx;
  const repo = repoOf(ctx, query.get("repo"));
  if (repo === null) return;
  const history = state.checkpoints[repo] ?? [];
  const path = query.get("path");
  const turnId = query.get("turn_id");
  const limit = Math.min(
    Math.max(Number(query.get("limit") ?? DEFAULT_PAGE_SIZE) || 1, 1),
    MAX_PAGE_SIZE,
  );

  const matching = history
    .map((checkpoint, at) => ({ checkpoint, at }))
    .filter(
      ({ checkpoint, at }) =>
        (turnId === null || checkpoint.summary.turn_id === turnId) &&
        (path === null || pathTouches(changesAt({ history, at }), path)),
    )
    .reverse();
  const before = query.get("before");
  const after =
    before === null ? -1 : matching.findIndex((m) => m.checkpoint.summary.id === before);
  if (before !== null && after === -1) {
    text(res, 404, "invalid page cursor");
    return;
  }
  const rest = matching.slice(after + 1);
  const items = rest.slice(0, limit).map((m) => m.checkpoint.summary);
  json(res, 200, {
    items,
    next_cursor: rest.length > limit ? (items.at(-1)?.id ?? null) : null,
  } satisfies CheckpointPage);
}

function checkpointStats(ctx: RouteContext): void {
  const repo = repoOf(ctx, ctx.query.get("repo"));
  if (repo !== null) json(ctx.res, 200, repoStats(ctx.state, repo));
}

function showCheckpoint(ctx: RouteContext): void {
  const repo = repoOf(ctx, ctx.query.get("repo"));
  const found = repo === null ? null : find(ctx, repo, decodedParam(ctx, 0));
  const summary = found?.history[found.at]?.summary;
  if (found === null || summary === undefined) return;
  json(ctx.res, 200, { summary, changed_paths: changesAt(found) } satisfies CheckpointDetail);
}

/** The repository, checkpoint and `path` query of a diff or file read. */
function locateFile(ctx: RouteContext): { found: Found; repo: RepoKind; path: string } | null {
  const repo = repoOf(ctx, ctx.query.get("repo"));
  const path = ctx.query.get("path");
  if (repo !== null && path === null) {
    text(ctx.res, 400, "Failed to deserialize query string: missing field `path`");
    return null;
  }
  const found = repo === null ? null : find(ctx, repo, decodedParam(ctx, 0));
  return repo === null || found === null || path === null ? null : { found, repo, path };
}

function checkpointDiff(ctx: RouteContext): void {
  const located = locateFile(ctx);
  if (located === null) return;
  const { found, path } = located;
  const changed = changesAt(found).some((c) => c.path === path);
  const diff = changed
    ? unifiedDiff(
        path,
        found.history[found.at - 1]?.files[path],
        found.history[found.at]?.files[path],
      )
    : null;
  json(ctx.res, 200, { diff });
}

function checkpointFile(ctx: RouteContext): void {
  const located = locateFile(ctx);
  if (located === null) return;
  const { found, path } = located;
  const content = found.history[found.at]?.files[path];
  if (content === undefined) {
    text(ctx.res, 404, `${path} is not a file at this checkpoint`);
    return;
  }
  ctx.res.writeHead(200, { "Content-Type": "text/plain; charset=utf-8" });
  ctx.res.end(content);
}

/** The repository named by a JSON body, and the body, or `null` after answering. */
async function bodyRepo(ctx: RouteContext): Promise<{ repo: RepoKind; body: JsonObject } | null> {
  const body = await readJsonObject(ctx.req);
  const repo = repoOf(ctx, stringField(body, "repo") ?? null);
  return repo === null ? null : { repo, body };
}

async function restoreCheckpoint(ctx: RouteContext): Promise<void> {
  const parsed = await bodyRepo(ctx);
  if (parsed === null) return;
  const { repo, body } = parsed;
  const path = stringField(body, "path");
  if (path === undefined) {
    text(ctx.res, 422, "Failed to deserialize the JSON body: missing string field `path`");
    return;
  }
  const id = decodedParam(ctx, 0);
  const found = find(ctx, repo, id);
  if (found === null) return;
  const { state } = ctx;
  const target = found.history[found.at];
  const prefix = `${path.replace(/\/$/, "")}/`;
  const under = (p: string): boolean => p === path || p.startsWith(prefix);
  const restored = Object.keys(target?.files ?? {})
    .filter(under)
    .sort();
  if (restored.length === 0) {
    text(ctx.res, 404, `path '${path}' not found at checkpoint '${id}'`);
    return;
  }
  const now = liveFiles(state, repo);
  for (const gone of Object.keys(now).filter((p) => under(p) && !restored.includes(p))) {
    writeLive(state, repo, gone, null);
  }
  for (const p of restored) writeLive(state, repo, p, target?.files[p] ?? "");
  const made = record(state, repo, liveFiles(state, repo), {
    trigger: "restore",
    summary: `restored ${path} from checkpoint ${id}`,
  });
  json(ctx.res, 200, {
    checkpoint_id: made.summary.id,
    restored_paths: restored,
  } satisfies RestoreOutcome);
}

async function undoCheckpoint(ctx: RouteContext): Promise<void> {
  const parsed = await bodyRepo(ctx);
  if (parsed === null) return;
  const { repo } = parsed;
  const id = decodedParam(ctx, 0);
  const found = find(ctx, repo, id);
  if (found === null) return;
  const { state } = ctx;
  const { history, at } = found;
  const live = liveFiles(state, repo);
  const reverted: string[] = [];
  const skipped: string[] = [];
  for (const { path } of changesAt(found)) {
    // A path that changed again since would lose that later edit.
    if (live[path] !== history[at]?.files[path]) {
      skipped.push(path);
      continue;
    }
    writeLive(state, repo, path, history[at - 1]?.files[path] ?? null);
    reverted.push(path);
  }
  const made = record(state, repo, liveFiles(state, repo), {
    trigger: "undo",
    summary: `undid checkpoint ${id}`,
  });
  json(ctx.res, 200, {
    checkpoint_id: made.summary.id,
    reverted_paths: reverted,
    skipped_paths: skipped,
  } satisfies UndoOutcome);
}

/** The checkpoint routes, in the unscoped `/api/...` spelling that both the agent and the hub scopes use. */
export const checkpointRoutes: readonly Route[] = [
  { method: "GET", pattern: "/api/checkpoints", handler: listCheckpoints },
  { method: "GET", pattern: "/api/checkpoints/stats", handler: checkpointStats },
  { method: "GET", pattern: /^\/api\/checkpoints\/([^/]+)$/, handler: showCheckpoint },
  { method: "GET", pattern: /^\/api\/checkpoints\/([^/]+)\/diff$/, handler: checkpointDiff },
  { method: "GET", pattern: /^\/api\/checkpoints\/([^/]+)\/file$/, handler: checkpointFile },
  { method: "POST", pattern: /^\/api\/checkpoints\/([^/]+)\/restore$/, handler: restoreCheckpoint },
  { method: "POST", pattern: /^\/api\/checkpoints\/([^/]+)\/undo$/, handler: undoCheckpoint },
];
