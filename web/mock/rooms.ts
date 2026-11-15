import type {
  CreateRoomRequest,
  MarkRoomReadRequest,
  PatchRoomRequest,
  PostRoomMessageRequest,
  RoomDetail,
  RoomDetailResponse,
  RoomListResponse,
  RoomMessage,
  RoomRosterCandidate,
  RoomSeat,
  RoomSummary,
} from "../src/lib/hub-types";
import { json, readJsonObject } from "./http";
import { decodedParam, type Route, type RouteContext } from "./routes";
import type { MockHub } from "./state";

interface MockRoom {
  detail: RoomDetail;
  messages: RoomMessage[];
  minutesMarkdown: string | null;
}

const roomsByHub = new WeakMap<MockHub, Map<string, MockRoom>>();

function storeFor(hub: MockHub): Map<string, MockRoom> {
  let map = roomsByHub.get(hub);
  if (map === undefined) {
    map = new Map();
    roomsByHub.set(hub, map);
  }
  return map;
}

function summaryOf(detail: RoomDetail): RoomSummary {
  return {
    id: detail.id,
    name: detail.name,
    topic: detail.topic,
    status: detail.status,
    pinned: detail.pinned,
    created_at: detail.created_at,
    updated_at: detail.updated_at,
    archived_at: detail.archived_at,
    roster: detail.roster,
    last_seq: detail.last_seq,
    unread_count: detail.unread_count,
    paused_at_cap: detail.window.paused_at_cap,
    minutes_shared_path: detail.minutes.shared_path,
  };
}

function candidatesOf(hub: MockHub): RoomRosterCandidate[] {
  return [...hub.agents.values()].map((agent) => ({
    seat_id: `agent:${agent.name}`,
    kind: "local",
    display_name: agent.displayName,
    role: agent.role,
    instance: null,
    online: agent.runState === "running",
  }));
}

function seatFromId(hub: MockHub, seatId: string): RoomSeat {
  const candidate = candidatesOf(hub).find((c) => c.seat_id === seatId);
  if (candidate !== undefined) {
    return {
      seat_id: candidate.seat_id,
      kind: candidate.kind,
      display_name: candidate.display_name,
      role: candidate.role,
      instance: candidate.instance,
    };
  }
  return {
    seat_id: seatId,
    kind: "local",
    display_name: seatId.replace(/^agent:/, ""),
    role: null,
    instance: null,
  };
}

function listRooms({ res, hub }: RouteContext): void {
  const rooms = [...storeFor(hub).values()]
    .map((entry) => summaryOf(entry.detail))
    .sort((a, b) => b.updated_at.localeCompare(a.updated_at));
  json(res, 200, { rooms, decision_model_active: true } satisfies RoomListResponse);
}

function listCandidates({ res, hub }: RouteContext): void {
  json(res, 200, { candidates: candidatesOf(hub) });
}

async function createRoom(ctx: RouteContext): Promise<void> {
  const { req, res, hub } = ctx;
  const body = (await readJsonObject(req)) as unknown as CreateRoomRequest | null;
  if (body === null || typeof body.name !== "string" || body.name.trim() === "") {
    json(res, 400, { error: "room name is required" });
    return;
  }
  const id = `room_${String(storeFor(hub).size + 1).padStart(8, "0")}`;
  const now = hub.env.clock.iso();
  const roster = (Array.isArray(body.seat_ids) ? body.seat_ids : []).map((seatId) =>
    seatFromId(hub, seatId),
  );
  const detail: RoomDetail = {
    id,
    name: body.name.trim(),
    topic: body.topic ?? null,
    status: "active",
    pinned: body.pinned,
    created_at: now,
    updated_at: now,
    archived_at: null,
    roster,
    historical_participants: roster.map((s) => s.seat_id),
    last_seq: 0,
    last_read_seq: 0,
    unread_count: 0,
    window: {
      roster_size_at_start: roster.length,
      window_cap: Math.ceil(1.5 * Math.max(1, roster.length)),
      window_used: 0,
      per_seat_used: {},
      queued_seats: [],
      paused_at_cap: false,
      decision_model_fallback: false,
    },
    minutes: { shared_path: null, archived_at_seq: 0, inbox_ref: null, updated_at: null },
    active_turn: null,
  };
  const entry: MockRoom = { detail, messages: [], minutesMarkdown: null };
  storeFor(hub).set(id, entry);
  json(res, 201, {
    room: detail,
    messages: [],
    minutes_markdown: null,
    decision_model_active: true,
  } satisfies RoomDetailResponse);
}

function getRoom(ctx: RouteContext): void {
  const id = decodedParam(ctx, 0);
  const entry = storeFor(ctx.hub).get(id);
  if (entry === undefined) {
    json(ctx.res, 404, { error: `room '${id}' not found` });
    return;
  }
  const beforeSeqParam = ctx.query.get("before_seq");
  const beforeSeq = beforeSeqParam === null ? null : Number(beforeSeqParam);
  const limit = Math.min(200, Math.max(1, Number(ctx.query.get("limit") ?? "200") || 200));
  const filtered =
    beforeSeq === null ? entry.messages : entry.messages.filter((m) => m.seq < beforeSeq);
  const slice = filtered.slice(-limit);
  json(ctx.res, 200, {
    room: entry.detail,
    messages: slice,
    minutes_markdown: entry.minutesMarkdown,
    decision_model_active: true,
  } satisfies RoomDetailResponse);
}

async function patchRoom(ctx: RouteContext): Promise<void> {
  const id = decodedParam(ctx, 0);
  const entry = storeFor(ctx.hub).get(id);
  if (entry === undefined) {
    json(ctx.res, 404, { error: `room '${id}' not found` });
    return;
  }
  const body = (await readJsonObject(ctx.req)) as unknown as PatchRoomRequest | null;
  if (body !== null) {
    if (body.name !== undefined) entry.detail.name = body.name;
    if (body.topic !== undefined) {
      entry.detail.topic = body.topic.trim() === "" ? null : body.topic;
    }
    if (body.pinned !== undefined) entry.detail.pinned = body.pinned;
    if (Array.isArray(body.remove_seats)) {
      const removed = new Set(body.remove_seats);
      const next = entry.detail.roster.filter((s) => !removed.has(s.seat_id));
      if (next.length === 0) {
        json(ctx.res, 400, { error: "an active room must keep at least 1 participant" });
        return;
      }
      entry.detail.roster = next;
    }
    if (Array.isArray(body.add_seats)) {
      const existing = new Set(entry.detail.roster.map((s) => s.seat_id));
      for (const seatId of body.add_seats) {
        if (!existing.has(seatId)) {
          entry.detail.roster.push(seatFromId(ctx.hub, seatId));
          existing.add(seatId);
          if (!entry.detail.historical_participants.includes(seatId)) {
            entry.detail.historical_participants.push(seatId);
          }
        }
      }
    }
    entry.detail.updated_at = ctx.hub.env.clock.iso();
  }
  json(ctx.res, 200, entry.detail);
}

async function markRead(ctx: RouteContext): Promise<void> {
  const id = decodedParam(ctx, 0);
  const entry = storeFor(ctx.hub).get(id);
  if (entry === undefined) {
    json(ctx.res, 404, { error: `room '${id}' not found` });
    return;
  }
  const body = (await readJsonObject(ctx.req)) as unknown as MarkRoomReadRequest | null;
  if (body !== null && typeof body.seq === "number") {
    entry.detail.last_read_seq = Math.max(
      entry.detail.last_read_seq,
      Math.min(body.seq, entry.detail.last_seq),
    );
    entry.detail.unread_count = Math.max(0, entry.detail.last_seq - entry.detail.last_read_seq);
  }
  ctx.res.writeHead(204).end();
}

async function postMessage(ctx: RouteContext): Promise<void> {
  const id = decodedParam(ctx, 0);
  const entry = storeFor(ctx.hub).get(id);
  if (entry === undefined) {
    json(ctx.res, 404, { error: `room '${id}' not found` });
    return;
  }
  const body = (await readJsonObject(ctx.req)) as unknown as PostRoomMessageRequest | null;
  const seq = entry.detail.last_seq + 1;
  const now = ctx.hub.env.clock.iso();
  const message: RoomMessage = {
    seq,
    id: `rmsg_${String(seq).padStart(8, "0")}`,
    room_id: id,
    kind: "owner",
    seat_id: null,
    author_name: "Owner",
    seat_kind: null,
    content: body?.content ?? "",
    attachments: [],
    timestamp: now,
  };
  entry.messages.push(message);
  entry.detail.last_seq = seq;
  entry.detail.last_read_seq = seq;
  entry.detail.unread_count = 0;
  entry.detail.updated_at = now;
  json(ctx.res, 202, message);
}

function continueRoom(ctx: RouteContext): void {
  const id = decodedParam(ctx, 0);
  const entry = storeFor(ctx.hub).get(id);
  if (entry === undefined) {
    json(ctx.res, 404, { error: `room '${id}' not found` });
    return;
  }
  entry.detail.window.paused_at_cap = false;
  json(ctx.res, 202, entry.detail);
}

function stopRoom(ctx: RouteContext): void {
  const id = decodedParam(ctx, 0);
  const entry = storeFor(ctx.hub).get(id);
  if (entry === undefined) {
    json(ctx.res, 404, { error: `room '${id}' not found` });
    return;
  }
  entry.detail.active_turn = null;
  entry.detail.window.queued_seats = [];
  json(ctx.res, 200, entry.detail);
}

function archiveRoom(ctx: RouteContext): void {
  const id = decodedParam(ctx, 0);
  const entry = storeFor(ctx.hub).get(id);
  if (entry === undefined) {
    json(ctx.res, 404, { error: `room '${id}' not found` });
    return;
  }
  const now = ctx.hub.env.clock.iso();
  entry.detail.status = "archived";
  entry.detail.archived_at = now;
  if (entry.detail.last_seq > 0 && entry.detail.last_seq > entry.detail.minutes.archived_at_seq) {
    const slug = entry.detail.name
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-|-$/g, "");
    const sharedPath = `team/minutes/${now.slice(0, 10)}-${slug || id}.md`;
    entry.minutesMarkdown = `# Meeting Minutes: ${entry.detail.name}\n\n## Summary\n\nMock minutes generated from ${String(entry.messages.length)} messages.\n\n## Decisions\n\n- None recorded.\n\n## Action Items\n\n- Follow up on open items.\n`;
    entry.detail.minutes = {
      shared_path: sharedPath,
      archived_at_seq: entry.detail.last_seq,
      inbox_ref: null,
      updated_at: now,
    };
  }
  json(ctx.res, 200, {
    room: entry.detail,
    messages: entry.messages,
    minutes_markdown: entry.minutesMarkdown,
    decision_model_active: true,
  } satisfies RoomDetailResponse);
}

function reopenRoom(ctx: RouteContext): void {
  const id = decodedParam(ctx, 0);
  const entry = storeFor(ctx.hub).get(id);
  if (entry === undefined) {
    json(ctx.res, 404, { error: `room '${id}' not found` });
    return;
  }
  entry.detail.status = "active";
  entry.detail.archived_at = null;
  json(ctx.res, 200, entry.detail);
}

function deleteRoom(ctx: RouteContext): void {
  const id = decodedParam(ctx, 0);
  if (!storeFor(ctx.hub).delete(id)) {
    json(ctx.res, 404, { error: `room '${id}' not found` });
    return;
  }
  ctx.res.writeHead(204).end();
}

export const roomsRoutes: readonly Route[] = [
  { method: "GET", pattern: "/api/hub/rooms", handler: listRooms },
  { method: "POST", pattern: "/api/hub/rooms", handler: createRoom },
  { method: "GET", pattern: "/api/hub/rooms/candidates", handler: listCandidates },
  { method: "GET", pattern: /^\/api\/hub\/rooms\/([^/]+)$/, handler: getRoom },
  { method: "PATCH", pattern: /^\/api\/hub\/rooms\/([^/]+)$/, handler: patchRoom },
  { method: "DELETE", pattern: /^\/api\/hub\/rooms\/([^/]+)$/, handler: deleteRoom },
  { method: "POST", pattern: /^\/api\/hub\/rooms\/([^/]+)\/read$/, handler: markRead },
  { method: "POST", pattern: /^\/api\/hub\/rooms\/([^/]+)\/messages$/, handler: postMessage },
  { method: "POST", pattern: /^\/api\/hub\/rooms\/([^/]+)\/continue$/, handler: continueRoom },
  { method: "POST", pattern: /^\/api\/hub\/rooms\/([^/]+)\/stop$/, handler: stopRoom },
  { method: "POST", pattern: /^\/api\/hub\/rooms\/([^/]+)\/archive$/, handler: archiveRoom },
  { method: "POST", pattern: /^\/api\/hub\/rooms\/([^/]+)\/reopen$/, handler: reopenRoom },
];
