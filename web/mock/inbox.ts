import type { UserInboxAttachment, UserInboxItem } from "../src/lib/types";
import { json, text } from "./http";
import { decodedParam, type Route, type RouteContext } from "./routes";
import type { MockState } from "./state";

/**
 * The user inbox routes. An agent's inbox items are in `state.inboxItems` and
 * its archive in `state.inboxArchive`. Both are empty for an agent that has
 * never run, which the routes list as `[]`, the way the backend does for an
 * inbox directory that doesn't exist yet.
 */

/** A 1x1 PNG: what an image attachment serves. */
const PNG_PIXEL = Buffer.from(
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

/** What an attachment serves: the mock keeps no files, so it is a stand-in of the attachment's type. */
function attachmentBody(attachment: Pick<UserInboxAttachment, "filename" | "mime_type">): Buffer {
  if (attachment.mime_type === "image/png") return PNG_PIXEL;
  return Buffer.from(`Mock attachment: ${attachment.filename}\n`);
}

/**
 * An attachment of a new item, sized as what it serves. Its `url` is filled in
 * when the item is listed.
 */
export function mockAttachment(filename: string, mimeType: string): UserInboxAttachment {
  const size = attachmentBody({ filename, mime_type: mimeType }).length;
  return { filename, mime_type: mimeType, size, url: "" };
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

/** Serve one attachment of an item in the inbox, or else in the archive, so a link outlives archiving. */
function serveAttachment(ctx: RouteContext): void {
  const id = itemId(ctx);
  const indexText = decodedParam(ctx, 1);
  if (!/^\d+$/.test(indexText)) {
    ctx.res.writeHead(400, { "Content-Type": "text/plain" });
    ctx.res.end(`Invalid URL: Cannot parse \`${indexText}\` to a \`usize\``);
    return;
  }
  const { inboxItems, inboxArchive } = ctx.state;
  const item = [...inboxItems, ...inboxArchive].find((candidate) => candidate.id === id);
  const attachment = item?.attachments[Number(indexText)];
  if (attachment === undefined) {
    ctx.res.writeHead(404);
    ctx.res.end();
    return;
  }
  const body = attachmentBody(attachment);
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
