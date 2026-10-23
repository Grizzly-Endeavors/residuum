import type { UserInboxAttachment, UserInboxItem } from "../src/lib/types";
import { json, text } from "./http";
import { decodedParam, type Route, type RouteContext } from "./routes";
import type { MockState } from "./state";
import { writeFile } from "./workspace-tree";

/**
 * The user inbox routes. An agent's inbox items are in `state.inboxItems` and
 * its archive in `state.inboxArchive`. Both are empty for an agent that has
 * never run, which the routes list as `[]`, the way the backend does for an
 * inbox directory that doesn't exist yet.
 */

/** A 1x1 PNG: what an image attachment serves. */
export const PNG_PIXEL = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==",
  "base64",
);

/** The timestamp the backend serializes: local time to the minute, `YYYY-MM-DDTHH:MM`. */
function minuteTimestamp(timestamp: string): string {
  return timestamp.slice(0, 16);
}

/** Where an item's attachment is served. */
export function attachmentUrl(agent: string, id: string, index: number): string {
  return `/api/agents/${agent}/inbox/${id}/attachments/${index}`;
}

/**
 * An item the way the API lists it (`ApiInboxItem`): the timestamp to the
 * minute, and each attachment pointing at its route under the agent.
 */
export function toApiInboxItem(state: MockState, item: UserInboxItem): UserInboxItem {
  return {
    ...item,
    timestamp: minuteTimestamp(item.timestamp),
    attachments: item.attachments.map((attachment, index) => ({
      ...attachment,
      url: attachmentUrl(state.agentName, item.id, index),
    })),
  };
}

/** The content types the backend names a file's type by, from its extension. */
const MIME_TYPES: Readonly<Record<string, string>> = {
  md: "text/markdown",
  txt: "text/plain",
  csv: "text/csv",
  html: "text/html",
  json: "application/json",
  pdf: "application/pdf",
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  gif: "image/gif",
  svg: "image/svg+xml",
  zip: "application/zip",
};

/** The type of a file by its extension, as the backend detects it. */
export function mimeTypeOf(filename: string): string {
  const extension = filename.slice(filename.lastIndexOf(".") + 1).toLowerCase();
  return MIME_TYPES[extension] ?? "application/octet-stream";
}

/** Whether a file of this type is kept as text in the workspace, where the file panel can open it. */
function isText(mimeType: string): boolean {
  return mimeType.startsWith("text/") || mimeType === "application/json";
}

/** Which of an agent's two inbox directories an attachment is in: `user` holds what the agent sent the user, `agent` what was sent to the agent. */
export type InboxSide = "user" | "agent";

/** Where an item's attachment is in the agent's workspace. */
export function attachmentPath(side: InboxSide, id: string, filename: string): string {
  return `inbox/${side}/attachments/${id}/${filename}`;
}

/**
 * Keep a file as an item's attachment: a text file is a workspace file, so the
 * file panel opens it by `attachmentPath`; any other kind is held by its bytes.
 * The attachment's `url` is filled in when the item is listed.
 */
export function storeAttachment(
  state: MockState,
  side: InboxSide,
  id: string,
  filename: string,
  bytes: Buffer,
): UserInboxAttachment {
  const mimeType = mimeTypeOf(filename);
  const path = attachmentPath(side, id, filename);
  if (isText(mimeType)) writeFile(state, path, bytes.toString("utf8"));
  else state.attachmentBytes.set(path, bytes);
  return { filename, mime_type: mimeType, size: bytes.length, url: "" };
}

/** What an attachment nobody stored serves: a stand-in of the attachment's type. */
function standInBody(attachment: Pick<UserInboxAttachment, "filename" | "mime_type">): Buffer {
  if (attachment.mime_type === "image/png") return PNG_PIXEL;
  return Buffer.from(`Mock attachment: ${attachment.filename}\n`);
}

/** What an attachment serves: the file as it was stored, else a stand-in. */
function attachmentBody(
  state: MockState,
  side: InboxSide,
  id: string,
  attachment: UserInboxAttachment,
): Buffer {
  const path = attachmentPath(side, id, attachment.filename);
  const stored = state.workspaceFileContents[path];
  if (stored !== undefined) return Buffer.from(stored);
  return state.attachmentBytes.get(path) ?? standInBody(attachment);
}

/**
 * An attachment of a new item that nobody stored a file for, sized as the
 * stand-in it serves. Its `url` is filled in when the item is listed.
 */
export function mockAttachment(filename: string, mimeType: string): UserInboxAttachment {
  const size = standInBody({ filename, mime_type: mimeType }).length;
  return { filename, mime_type: mimeType, size, url: "" };
}

/** Put an attachment made by `mockAttachment` in the workspace, where the file panel opens a text one by its path. */
export function keepStandIn(
  state: MockState,
  side: InboxSide,
  id: string,
  attachment: UserInboxAttachment,
): void {
  if (!isText(attachment.mime_type)) return;
  writeFile(
    state,
    attachmentPath(side, id, attachment.filename),
    standInBody(attachment).toString("utf8"),
  );
}

/**
 * The attachments of the item `id` and the side it is on: an item the agent
 * sent the user, active or archived, else a message sent to the agent.
 */
function attachmentsOf(
  state: MockState,
  id: string,
): { side: InboxSide; attachments: readonly UserInboxAttachment[] } | undefined {
  const sent = [...state.inboxItems, ...state.inboxArchive].find((item) => item.id === id);
  if (sent !== undefined) return { side: "user", attachments: sent.attachments };
  const received = state.agentInbox.find(
    (item) => item.id === id && item.attachments !== undefined,
  );
  if (received?.attachments !== undefined) {
    return { side: "agent", attachments: received.attachments };
  }
  return undefined;
}

function newestFirst(items: readonly UserInboxItem[]): UserInboxItem[] {
  return [...items].sort((a, b) => b.timestamp.localeCompare(a.timestamp));
}

/** The item id a route names: the backend takes it with or without `.json`. */
function itemId(ctx: RouteContext): string {
  const id = decodedParam(ctx, 0);
  return id.endsWith(".json") ? id.slice(0, -".json".length) : id;
}

function listItems(ctx: RouteContext, items: readonly UserInboxItem[]): void {
  json(
    ctx.res,
    200,
    newestFirst(items).map((item) => toApiInboxItem(ctx.state, item)),
  );
}

/**
 * A running agent's file watcher sees a change to its inbox files and the hub
 * counts them again. Nothing watches a stopped agent's, so its count is read
 * the next time it is asked for.
 */
function noticeInboxChange(ctx: RouteContext): void {
  const agent = ctx.hub.agents.get(ctx.state.agentName);
  if (agent?.runState === "running") ctx.hub.overview.changed(agent);
}

function markRead(ctx: RouteContext): void {
  const id = itemId(ctx);
  const item = ctx.state.inboxItems.find((candidate) => candidate.id === id);
  if (item === undefined) {
    text(
      ctx.res,
      500,
      `failed to mark inbox item as read: failed to load inbox item ${id}.json for mark_read`,
    );
    return;
  }
  item.read = true;
  noticeInboxChange(ctx);
  json(ctx.res, 200, toApiInboxItem(ctx.state, item));
}

/** Move an item from one list to the other, or answer `500` with `missing`'s message, the way the backend does for a file that isn't there. */
function moveItem(
  ctx: RouteContext,
  from: UserInboxItem[],
  to: UserInboxItem[],
  missing: (id: string) => string,
): void {
  const id = itemId(ctx);
  const at = from.findIndex((candidate) => candidate.id === id);
  const [item] = at === -1 ? [] : from.splice(at, 1);
  if (item === undefined) {
    text(ctx.res, 500, missing(id));
    return;
  }
  to.push(item);
  noticeInboxChange(ctx);
  json(ctx.res, 200, null);
}

/**
 * Serve one attachment of an item in the inbox, or else in the archive, so a
 * link outlives archiving, or else of a message the user sent the agent.
 */
function serveAttachment(ctx: RouteContext): void {
  const id = itemId(ctx);
  const indexText = decodedParam(ctx, 1);
  if (!/^\d+$/.test(indexText)) {
    ctx.res.writeHead(400, { "Content-Type": "text/plain" });
    ctx.res.end(`Invalid URL: Cannot parse \`${indexText}\` to a \`usize\``);
    return;
  }
  const found = attachmentsOf(ctx.state, id);
  const attachment = found?.attachments[Number(indexText)];
  if (found === undefined || attachment === undefined) {
    ctx.res.writeHead(404);
    ctx.res.end();
    return;
  }
  const body = attachmentBody(ctx.state, found.side, id, attachment);
  // A quote or backslash would end the header's quoted filename early.
  const filename = attachment.filename.replace(/["\\]/g, "");
  ctx.res.writeHead(200, {
    "Content-Type": attachment.mime_type,
    "Content-Disposition": `inline; filename="${filename}"`,
    "Content-Length": body.length,
  });
  ctx.res.end(body);
}

/** The user inbox routes, in the unscoped `/api/...` spelling. */
export const inboxRoutes: readonly Route[] = [
  {
    method: "GET",
    pattern: "/api/inbox",
    handler: (ctx) => {
      listItems(ctx, ctx.state.inboxItems);
    },
  },
  {
    method: "GET",
    pattern: "/api/inbox/archive",
    handler: (ctx) => {
      listItems(ctx, ctx.state.inboxArchive);
    },
  },
  {
    method: "PUT",
    pattern: /^\/api\/inbox\/([^/]+)\/read$/,
    handler: markRead,
  },
  {
    method: "POST",
    pattern: /^\/api\/inbox\/([^/]+)\/archive$/,
    handler: (ctx) => {
      moveItem(
        ctx,
        ctx.state.inboxItems,
        ctx.state.inboxArchive,
        (id) =>
          `failed to archive inbox item: inbox item '${id}.json' not found or could not be archived`,
      );
    },
  },
  {
    method: "POST",
    pattern: /^\/api\/inbox\/([^/]+)\/restore$/,
    handler: (ctx) => {
      moveItem(
        ctx,
        ctx.state.inboxArchive,
        ctx.state.inboxItems,
        (id) =>
          `failed to restore inbox item: inbox item '${id}.json' not found in archive or could not be restored`,
      );
    },
  },
  {
    method: "GET",
    pattern: /^\/api\/inbox\/([^/]+)\/attachments\/([^/]+)$/,
    handler: serveAttachment,
  },
];
