import type {
  WorkspaceEntry,
  WorkspaceMoveResponse,
  WorkspaceValidateResponse,
  WorkspaceWriteResponse,
} from "../src/lib/types";
import { json, readJsonObject, stringField, text, type JsonObject } from "./http";
import type { Route, RouteContext } from "./routes";
import type { MockState } from "./state";
import {
  fileVersion,
  listDirectory,
  movePath,
  pathKind,
  removePath,
  writeFile,
} from "./workspace-tree";

/**
 * The workspace file API comes in two scopes. The agent's own
 * (`/api/agents/{name}/workspace/...`) addresses its workspace, where the
 * shared team tree shows up under `team/`. The team's (`/api/team/workspace/...`)
 * addresses that tree directly, with paths relative to `team/`. The mock keeps
 * the team tree in the hub's state, under `team/`.
 */
type WorkspaceScope = "agent" | "team";

/** The team tree's place in a workspace path. */
const TEAM_PREFIX = "team";

/** A path in a workspace namespace, placed in the state that holds it. */
interface Located {
  state: MockState;
  /** The path within that state's workspace. */
  key: string;
}

/** Whether `key` is in the shared team tree. */
function isTeamKey(key: string): boolean {
  return key === TEAM_PREFIX || key.startsWith(`${TEAM_PREFIX}/`);
}

/** Place a path as a client sent it. A path that leaves the workspace is refused with `403`, and gives `null`. */
function locate(ctx: RouteContext, scope: WorkspaceScope, path: string): Located | null {
  if (path.startsWith("/") || path.split("/").includes("..")) {
    text(ctx.res, 403, `path traversal rejected: ${path}`);
    return null;
  }
  const relative = path.replace(/\/+$/, "");
  if (scope === "team") {
    const key = relative === "" ? TEAM_PREFIX : `${TEAM_PREFIX}/${relative}`;
    return { state: ctx.hub.hubState, key };
  }
  return { state: isTeamKey(relative) ? ctx.hub.hubState : ctx.state, key: relative };
}

/** The `path` query parameter every file route needs, or `null` after answering `400` for its absence. */
function requirePathQuery(ctx: RouteContext): string | null {
  const path = ctx.query.get("path");
  if (path === null) text(ctx.res, 400, "Failed to deserialize query string: missing `path`");
  return path;
}

/** The string fields a JSON body has to carry, or `undefined` after answering `422` for one that is missing. */
function requireStrings(
  ctx: RouteContext,
  body: JsonObject,
  fields: string[],
): string[] | undefined {
  const values: string[] = [];
  for (const field of fields) {
    const value = stringField(body, field);
    if (value === undefined) {
      text(ctx.res, 422, `Failed to deserialize the JSON body: missing string field \`${field}\``);
      return undefined;
    }
    values.push(value);
  }
  return values;
}

function notFound(ctx: RouteContext, path: string): void {
  text(ctx.res, 404, `path not found: ${path}`);
}

/** Refuse, with `400`, what would replace, move or delete the team folder itself. */
function refusesTeamRoot(ctx: RouteContext, key: string): boolean {
  if (key !== TEAM_PREFIX) return false;
  text(ctx.res, 400, "team is the shared team folder; it can't be replaced, moved, or deleted");
  return true;
}

/** The version of the file at `at`, or `null` when there is none. */
function versionOf(at: Located): string | null {
  const content = at.state.workspaceFileContents[at.key];
  return content === undefined ? null : fileVersion(content);
}

/**
 * Answer `412` when the request's `If-Match` or `If-None-Match: *` doesn't
 * hold for the file at `at`, and report whether it did. A request with
 * neither is unconditional.
 */
function preconditionFailed(ctx: RouteContext, at: Located): boolean {
  const ifMatch = ctx.req.headers["if-match"];
  const createOnly = ctx.req.headers["if-none-match"] === "*";
  if (ifMatch === undefined && !createOnly) return false;
  const current = versionOf(at);
  if (createOnly && current !== null) {
    json(ctx.res, 412, { error: "file already exists", current_version: current });
    return true;
  }
  if (ifMatch !== undefined && ifMatch !== current) {
    json(ctx.res, 412, {
      error: "file has changed since it was last read",
      current_version: current,
    });
    return true;
  }
  return false;
}

function listFiles(ctx: RouteContext, scope: WorkspaceScope): void {
  const path = ctx.query.get("path") ?? "";
  const at = locate(ctx, scope, path);
  if (at === null) return;
  const kind = pathKind(at.state, at.key);
  if (kind === null) {
    notFound(ctx, path);
    return;
  }
  const entries = listDirectory(at.state, at.key);
  if (kind === "file" || entries === undefined) {
    text(ctx.res, 500, "failed to read directory: Not a directory (os error 20)");
    return;
  }
  // The team folder in an agent's root is the shared one, whatever the agent's
  // own copy of the root says about it.
  const team = ctx.hub.hubState.workspaceFiles[""]?.find((entry) => entry.name === TEAM_PREFIX);
  const listed: WorkspaceEntry[] =
    scope === "agent" && at.key === "" && team !== undefined
      ? entries.map((entry) => (entry.name === TEAM_PREFIX ? team : entry))
      : entries;
  json(ctx.res, 200, listed);
}

function readFile(ctx: RouteContext, scope: WorkspaceScope): void {
  const path = requirePathQuery(ctx);
  if (path === null) return;
  const at = locate(ctx, scope, path);
  if (at === null) return;
  const content = at.state.workspaceFileContents[at.key];
  if (content === undefined) {
    if (pathKind(at.state, at.key) === "directory") {
      text(ctx.res, 500, "failed to read file: Is a directory (os error 21)");
    } else {
      notFound(ctx, path);
    }
    return;
  }
  ctx.res.writeHead(200, {
    "Content-Type": "text/plain; charset=utf-8",
    ETag: fileVersion(content),
  });
  ctx.res.end(content);
}

async function putFile(ctx: RouteContext, scope: WorkspaceScope): Promise<void> {
  const fields = requireStrings(ctx, await readJsonObject(ctx.req), ["path", "content"]);
  if (fields === undefined) return;
  const [path = "", content = ""] = fields;
  const at = locate(ctx, scope, path);
  if (at === null || refusesTeamRoot(ctx, at.key) || preconditionFailed(ctx, at)) return;
  const version = writeFile(at.state, at.key, content);
  json(ctx.res, 200, { saved: true, version } satisfies WorkspaceWriteResponse);
}

function deleteFile(ctx: RouteContext, scope: WorkspaceScope): void {
  const path = requirePathQuery(ctx);
  if (path === null) return;
  const at = locate(ctx, scope, path);
  if (at === null) return;
  const kind = pathKind(at.state, at.key);
  if (kind === null) {
    notFound(ctx, path);
    return;
  }
  if (at.key === "" || at.key === TEAM_PREFIX) {
    const what = at.key === "" ? "the workspace root" : "the team folder";
    text(ctx.res, 400, `${what} cannot be deleted`);
    return;
  }
  if (kind === "directory") {
    if (ctx.query.get("recursive") !== "true") {
      text(ctx.res, 409, `${path} is a directory; pass recursive=true to delete it`);
      return;
    }
  } else if (preconditionFailed(ctx, at)) {
    return;
  }
  removePath(at.state, at.key);
  // The mock keeps no checkpoints, so there is nothing for Undo to restore.
  json(ctx.res, 200, { deleted: true, checkpoint_id: null });
}

async function moveFile(ctx: RouteContext, scope: WorkspaceScope): Promise<void> {
  const body = await readJsonObject(ctx.req);
  const fields = requireStrings(ctx, body, ["from", "to"]);
  if (fields === undefined) return;
  const [fromPath = "", toPath = ""] = fields;
  const from = locate(ctx, scope, fromPath);
  const to = from === null ? null : locate(ctx, scope, toPath);
  if (from === null || to === null) return;
  if (refusesTeamRoot(ctx, from.key) || refusesTeamRoot(ctx, to.key)) return;
  const kind = pathKind(from.state, from.key);
  if (kind === null) {
    notFound(ctx, fromPath);
    return;
  }
  if (kind === "file" && preconditionFailed(ctx, from)) return;

  const moved = { moved: true, version: null } satisfies WorkspaceMoveResponse;
  const sameState = from.state === to.state;
  // Moving a path onto itself succeeds and changes nothing.
  if (sameState && from.key === to.key) {
    json(ctx.res, 200, { ...moved, version: versionOf(to) });
    return;
  }
  if (sameState && kind === "directory" && to.key.startsWith(`${from.key}/`)) {
    text(ctx.res, 400, `can't move ${fromPath} into itself (${toPath})`);
    return;
  }
  if (pathKind(to.state, to.key) !== null && body.overwrite !== true) {
    text(ctx.res, 409, `${toPath} already exists; pass overwrite: true to replace it`);
    return;
  }
  movePath(from.state, from.key, to.state, to.key);
  json(ctx.res, 200, { ...moved, version: versionOf(to) });
}

async function validateFile(ctx: RouteContext): Promise<void> {
  if (requireStrings(ctx, await readJsonObject(ctx.req), ["path", "content"]) === undefined) return;
  // The mock has no parsers for the strictly-parsed files, so it finds nothing to report.
  json(ctx.res, 200, { diagnostics: [] } satisfies WorkspaceValidateResponse);
}

function routesFor(scope: WorkspaceScope, prefix: string): readonly Route[] {
  return [
    {
      method: "GET",
      pattern: `${prefix}/files`,
      handler: (ctx) => {
        listFiles(ctx, scope);
      },
    },
    {
      method: "GET",
      pattern: `${prefix}/file`,
      handler: (ctx) => {
        readFile(ctx, scope);
      },
    },
    { method: "PUT", pattern: `${prefix}/file`, handler: (ctx) => putFile(ctx, scope) },
    {
      method: "DELETE",
      pattern: `${prefix}/file`,
      handler: (ctx) => {
        deleteFile(ctx, scope);
      },
    },
    { method: "POST", pattern: `${prefix}/move`, handler: (ctx) => moveFile(ctx, scope) },
    { method: "POST", pattern: `${prefix}/validate`, handler: validateFile },
  ];
}

/** The workspace file routes of both scopes, in the spelling scoped routing leaves them in. */
export const workspaceRoutes: readonly Route[] = [
  ...routesFor("agent", "/api/workspace"),
  ...routesFor("team", "/api/team/workspace"),
];
