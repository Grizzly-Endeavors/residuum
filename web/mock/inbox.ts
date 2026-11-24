import { json } from "./http";
import { decodedParam, type Route, type RouteContext } from "./routes";
import type { MockState, UserInboxAttachment, UserInboxItem } from "./state";
import { removePath, writeFile } from "./workspace-tree";

export const PNG_PIXEL = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==",
  "base64",
);

function pathSegment(value: string): string {
  return encodeURIComponent(value).replace(
    /[!'()*]/g,
    (char) => `%${char.charCodeAt(0).toString(16).toUpperCase()}`,
  );
}

export function attachmentUrl(agent: string, side: InboxSide, id: string, index: number): string {
  const owner = pathSegment(agent);
  const item = pathSegment(id);
  return side === "user"
    ? `/api/agents/${owner}/inbox/${item}/attachments/${String(index)}`
    : `/api/agents/${owner}/inbox/agent/${item}/attachments/${String(index)}`;
}

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
  webp: "image/webp",
  svg: "image/svg+xml",
  zip: "application/zip",
};

export function mimeTypeOf(filename: string): string {
  const extension = filename.slice(filename.lastIndexOf(".") + 1).toLowerCase();
  return MIME_TYPES[extension] ?? "application/octet-stream";
}

function isText(mimeType: string): boolean {
  return mimeType.startsWith("text/") || mimeType === "application/json";
}

export type InboxSide = "user" | "agent";

function attachmentDirectory(side: InboxSide, id: string, archived: boolean): string {
  return `${archived ? "archive/" : ""}inbox/${side}/attachments/${id}`;
}

export function attachmentPath(
  side: InboxSide,
  id: string,
  filename: string,
  archived: boolean,
): string {
  return `${attachmentDirectory(side, id, archived)}/${filename}`;
}

export function storeAttachment(
  state: MockState,
  side: InboxSide,
  id: string,
  filename: string,
  bytes: Buffer,
  archived = false,
): UserInboxAttachment {
  const mimeType = mimeTypeOf(filename);
  const path = attachmentPath(side, id, filename, archived);
  if (isText(mimeType)) writeFile(state, path, bytes.toString("utf8"));
  else state.attachmentBytes.set(path, bytes);
  return { filename, mime_type: mimeType, size: bytes.length, url: "" };
}

function standInBody(attachment: Pick<UserInboxAttachment, "filename" | "mime_type">): Buffer {
  if (attachment.mime_type === "image/png") return PNG_PIXEL;
  return Buffer.from(`Mock attachment: ${attachment.filename}\n`);
}

function attachmentBody(
  state: MockState,
  side: InboxSide,
  id: string,
  attachment: UserInboxAttachment,
  archived: boolean,
): Buffer {
  const path = attachmentPath(side, id, attachment.filename, archived);
  const stored = state.workspaceFileContents[path];
  if (stored !== undefined) return Buffer.from(stored);
  return state.attachmentBytes.get(path) ?? standInBody(attachment);
}

export function mockAttachment(filename: string, mimeType: string): UserInboxAttachment {
  const size = standInBody({ filename, mime_type: mimeType }).length;
  return { filename, mime_type: mimeType, size, url: "" };
}

export function keepStandIn(
  state: MockState,
  side: InboxSide,
  id: string,
  attachment: UserInboxAttachment,
): void {
  if (!isText(attachment.mime_type)) return;
  writeFile(
    state,
    attachmentPath(side, id, attachment.filename, false),
    standInBody(attachment).toString("utf8"),
  );
}

export function moveAttachments(state: MockState, item: UserInboxItem, toArchive: boolean): void {
  let moved = false;
  for (const { filename } of item.attachments) {
    const from = attachmentPath("user", item.id, filename, !toArchive);
    const to = attachmentPath("user", item.id, filename, toArchive);
    const content = state.workspaceFileContents[from];
    if (content !== undefined) {
      writeFile(state, to, content);
      moved = true;
    }
    const bytes = state.attachmentBytes.get(from);
    if (bytes !== undefined) {
      state.attachmentBytes.set(to, bytes);
      state.attachmentBytes.delete(from);
      moved = true;
    }
  }
  if (moved) removePath(state, attachmentDirectory("user", item.id, !toArchive));
}

interface FoundAttachments {
  attachments: readonly UserInboxAttachment[];
  archived: boolean;
}

function userItemAttachments(state: MockState, id: string): FoundAttachments | undefined {
  const active = state.inboxItems.find((item) => item.id === id);
  if (active !== undefined) return { attachments: active.attachments, archived: false };
  const archived = state.inboxArchive.find((item) => item.id === id);
  return archived === undefined ? undefined : { attachments: archived.attachments, archived: true };
}

function agentItemAttachments(state: MockState, id: string): FoundAttachments | undefined {
  const received = state.agentInbox.find((item) => item.id === id);
  return received?.attachments === undefined
    ? undefined
    : { attachments: received.attachments, archived: false };
}

function itemId(ctx: RouteContext): string {
  const id = decodedParam(ctx, 0);
  return id.endsWith(".json") ? id.slice(0, -".json".length) : id;
}

function serveAttachment(
  ctx: RouteContext,
  find: (state: MockState, id: string) => FoundAttachments | undefined,
  side: InboxSide,
): void {
  const id = itemId(ctx);
  const indexText = decodedParam(ctx, 1);
  if (!/^\d+$/.test(indexText)) {
    ctx.res.writeHead(400, { "Content-Type": "text/plain" });
    ctx.res.end(`Invalid URL: Cannot parse \`${indexText}\` to a \`usize\``);
    return;
  }
  const found = find(ctx.state, id);
  const attachment = found?.attachments[Number(indexText)];
  if (found === undefined || attachment === undefined) {
    json(ctx.res, 404, {
      error:
        "This attachment is no longer available. It may have been removed from its inbox item.",
    });
    return;
  }
  const body = attachmentBody(ctx.state, side, id, attachment, found.archived);
  const filename = attachment.filename.replace(/["\\]/g, "");
  ctx.res.writeHead(200, {
    "Content-Type": attachment.mime_type,
    "Content-Disposition": `inline; filename="${filename}"`,
    "Content-Length": body.length,
  });
  ctx.res.end(body);
}

export const inboxRoutes: readonly Route[] = [
  {
    method: "GET",
    pattern: /^\/api\/inbox\/([^/]+)\/attachments\/([^/]+)$/,
    handler: (ctx) => {
      serveAttachment(ctx, userItemAttachments, "user");
    },
  },
  {
    method: "GET",
    pattern: /^\/api\/inbox\/agent\/([^/]+)\/attachments\/([^/]+)$/,
    handler: (ctx) => {
      serveAttachment(ctx, agentItemAttachments, "agent");
    },
  },
];
