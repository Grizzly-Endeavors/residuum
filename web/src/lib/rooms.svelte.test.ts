import { afterEach, describe, expect, it, vi } from "vitest";
import { RoomsStore } from "./rooms.svelte";
import type {
  RoomDetail,
  RoomDetailResponse,
  RoomMessage,
  RoomRosterCandidate,
  RoomSeat,
  RoomSummary,
} from "./hub-types";

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

const SEAT_SCOUT: RoomSeat = {
  seat_id: "agent:scout",
  kind: "local",
  display_name: "Scout",
  role: "Researcher",
  instance: null,
};

const SEAT_ANALYST: RoomSeat = {
  seat_id: "agent:analyst",
  kind: "local",
  display_name: "Analyst",
  role: "Data analyst",
  instance: null,
};

function makeSummary(id: string, overrides: Partial<RoomSummary> = {}): RoomSummary {
  return {
    id,
    name: "Architecture Sync",
    topic: "API review",
    status: "active",
    pinned: false,
    created_at: "2026-11-14T10:00:00Z",
    updated_at: "2026-11-14T10:05:00Z",
    archived_at: null,
    roster: [SEAT_SCOUT, SEAT_ANALYST],
    last_seq: 2,
    unread_count: 0,
    paused_at_cap: false,
    minutes_shared_path: null,
    ...overrides,
  };
}

function makeDetail(id: string, overrides: Partial<RoomDetail> = {}): RoomDetail {
  return {
    id,
    name: "Architecture Sync",
    topic: "API review",
    status: "active",
    pinned: false,
    created_at: "2026-11-14T10:00:00Z",
    updated_at: "2026-11-14T10:05:00Z",
    archived_at: null,
    roster: [SEAT_SCOUT, SEAT_ANALYST],
    historical_participants: ["agent:scout", "agent:analyst"],
    last_seq: 2,
    last_read_seq: 2,
    unread_count: 0,
    window: {
      roster_size_at_start: 2,
      window_cap: 3,
      window_used: 1,
      per_seat_used: { "agent:scout": 1 },
      queued_seats: [],
      paused_at_cap: false,
      decision_model_fallback: false,
      selector_error: null,
    },
    minutes: {
      shared_path: null,
      archived_at_seq: 0,
      inbox_ref: null,
      updated_at: null,
    },
    meetings: [],
    last_captured_seq: 0,
    active_turn: null,
    ...overrides,
  };
}

function makeMessage(seq: number, overrides: Partial<RoomMessage> = {}): RoomMessage {
  return {
    seq,
    id: `rmsg_01jcv8m100000${String(seq)}`,
    room_id: "room_01jcv8k9m2p4q7",
    kind: "agent",
    seat_id: "agent:scout",
    author_name: "Scout",
    seat_kind: "local",
    content: `Message ${String(seq)}`,
    attachments: [],
    timestamp: `2026-11-14T10:0${String(seq)}:00Z`,
    ...overrides,
  };
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("RoomsStore", () => {
  it("loads the room list and derives active, archived, and pinned active rooms", async () => {
    const activePinned = makeSummary("room_01jcv8k9m2p4q7", {
      name: "Architecture Sync",
      pinned: true,
      updated_at: "2026-11-14T12:00:00Z",
    });
    const activeUnpinned = makeSummary("room_01jcv8k9m2p4q8", {
      name: "Daily Standup",
      pinned: false,
      updated_at: "2026-11-14T11:00:00Z",
    });
    const archivedPinned = makeSummary("room_01jcv8k9m2p4q9", {
      name: "Retrospective",
      status: "archived",
      pinned: true,
      archived_at: "2026-11-14T09:00:00Z",
      updated_at: "2026-11-14T09:00:00Z",
    });

    vi.stubGlobal(
      "fetch",
      vi.fn((url: string) => {
        if (url === "/api/hub/rooms") {
          return Promise.resolve(
            jsonResponse({
              rooms: [activeUnpinned, archivedPinned, activePinned],
              decision_model_active: true,
            }),
          );
        }
        return Promise.resolve(new Response("not found", { status: 404 }));
      }),
    );

    const store = new RoomsStore();
    await store.refreshList();

    expect(store.rooms.map((r) => r.id)).toEqual([
      "room_01jcv8k9m2p4q7",
      "room_01jcv8k9m2p4q8",
      "room_01jcv8k9m2p4q9",
    ]);
    expect(store.activeRooms.map((r) => r.id)).toEqual([
      "room_01jcv8k9m2p4q7",
      "room_01jcv8k9m2p4q8",
    ]);
    expect(store.archivedRooms.map((r) => r.id)).toEqual(["room_01jcv8k9m2p4q9"]);
    expect(store.pinnedRooms.map((r) => r.id)).toEqual(["room_01jcv8k9m2p4q7"]);
    expect(store.decisionModelActive).toBe(true);
    expect(store.error).toBeNull();
  });

  it("loads eligible roster candidates", async () => {
    const candidates: RoomRosterCandidate[] = [
      {
        seat_id: "agent:scout",
        kind: "local",
        display_name: "Scout",
        role: "Researcher",
        instance: null,
        online: true,
      },
    ];
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string) => {
        if (url === "/api/hub/rooms/candidates") {
          return Promise.resolve(jsonResponse({ candidates }));
        }
        return Promise.resolve(new Response("not found", { status: 404 }));
      }),
    );

    const store = new RoomsStore();
    await store.refreshCandidates();
    expect(store.candidates).toEqual(candidates);
  });

  it("opens a room and loads older messages", async () => {
    const id = "room_01jcv8k9m2p4q7";
    const initialResp: RoomDetailResponse = {
      room: makeDetail(id, { last_seq: 5 }),
      messages: [makeMessage(3), makeMessage(4), makeMessage(5)],
      minutes_markdown: null,
      decision_model_active: true,
    };
    const olderResp: RoomDetailResponse = {
      room: makeDetail(id, { last_seq: 5 }),
      messages: [makeMessage(1), makeMessage(2)],
      minutes_markdown: null,
      decision_model_active: true,
    };

    const fetchMock = vi.fn((url: string) => {
      if (url === `/api/hub/rooms/${id}?limit=200`) {
        return Promise.resolve(jsonResponse(initialResp));
      }
      if (url === `/api/hub/rooms/${id}?limit=200&before_seq=3`) {
        return Promise.resolve(jsonResponse(olderResp));
      }
      return Promise.resolve(new Response("not found", { status: 404 }));
    });
    vi.stubGlobal("fetch", fetchMock);

    const store = new RoomsStore();
    await store.openRoom(id);
    expect(store.currentRoom?.id).toBe(id);
    expect(store.messages.map((m) => m.seq)).toEqual([3, 4, 5]);
    expect(store.hasMoreMessages).toBe(true);

    await store.loadOlderMessages();
    expect(store.messages.map((m) => m.seq)).toEqual([1, 2, 3, 4, 5]);
    expect(store.hasMoreMessages).toBe(false);
  });

  it("handles room_updated frame by upserting summary and updating currentRoom", () => {
    const store = new RoomsStore();
    const id = "room_01jcv8k9m2p4q7";
    store.rooms = [makeSummary(id, { name: "Old Name", pinned: false })];
    store.currentRoom = makeDetail(id, { name: "Old Name", pinned: false });

    store.handleFrame({
      type: "room_updated",
      room: makeSummary(id, {
        name: "Updated Name",
        pinned: true,
        paused_at_cap: true,
        unread_count: 2,
        last_seq: 4,
        updated_at: "2026-11-14T12:30:00Z",
        minutes_shared_path: "team/minutes/2026-11-14-sync.md",
      }),
    });

    expect(store.rooms[0]?.name).toBe("Updated Name");
    expect(store.pinnedRooms.map((r) => r.id)).toEqual([id]);
    expect(store.currentRoom.name).toBe("Updated Name");
    expect(store.currentRoom.pinned).toBe(true);
    expect(store.currentRoom.window.paused_at_cap).toBe(true);
    expect(store.currentRoom.minutes.shared_path).toBe("team/minutes/2026-11-14-sync.md");
  });

  it("handles room_message frame: increments unread when not viewing and auto-marks read when viewing", async () => {
    const fetchMock = vi.fn((_url: string, _init?: RequestInit) =>
      Promise.resolve(new Response(null, { status: 204 })),
    );
    vi.stubGlobal("fetch", fetchMock);

    const store = new RoomsStore();
    const roomA = "room_01jcv8k9m2p4q7";
    const roomB = "room_01jcv8k9m2p4q8";
    store.rooms = [
      makeSummary(roomA, { last_seq: 1, unread_count: 0 }),
      makeSummary(roomB, { last_seq: 1, unread_count: 0 }),
    ];
    store.currentRoom = makeDetail(roomA, { last_seq: 1, last_read_seq: 1, unread_count: 0 });
    store.messages = [makeMessage(1, { room_id: roomA })];
    store.setViewingRoom(roomA);

    // Message arrives in roomB (not viewing): unread_count becomes 1, no POST /read.
    store.handleFrame({
      type: "room_message",
      room_id: roomB,
      message: makeMessage(2, { room_id: roomB }),
      unread_count: 1,
    });
    expect(store.rooms.find((r) => r.id === roomB)?.unread_count).toBe(1);
    expect(fetchMock).not.toHaveBeenCalled();

    // Message arrives in roomA (currently viewing): auto-marks read and keeps unread_count 0.
    store.handleFrame({
      type: "room_message",
      room_id: roomA,
      message: makeMessage(2, { room_id: roomA }),
      unread_count: 1,
    });
    expect(store.messages.map((m) => m.seq)).toEqual([1, 2]);
    expect(store.rooms.find((r) => r.id === roomA)?.unread_count).toBe(0);
    expect(store.currentRoom.unread_count).toBe(0);
    expect(store.currentRoom.last_read_seq).toBe(2);

    await Promise.resolve();
    expect(fetchMock).toHaveBeenCalledWith(
      `/api/hub/rooms/${roomA}/read`,
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({ seq: 2 }),
      }),
    );
  });

  it("handles room_turn_state frame across currentRoom and rooms list", () => {
    const store = new RoomsStore();
    const id = "room_01jcv8k9m2p4q7";
    store.rooms = [makeSummary(id, { paused_at_cap: false })];
    store.currentRoom = makeDetail(id);

    store.handleFrame({
      type: "room_turn_state",
      room_id: id,
      active_turn: {
        seat_id: "agent:analyst",
        display_name: "Analyst",
        seat_kind: "local",
        started_at: "2026-11-14T10:10:00Z",
        tool_status: "Reading workspace",
      },
      queued_seat_ids: ["agent:scout"],
      paused_at_cap: true,
      window_used: 3,
      window_cap: 3,
      selector_error: "provider rejected speaker selection: Input blocked.",
    });

    expect(store.currentRoom.active_turn?.display_name).toBe("Analyst");
    expect(store.currentRoom.active_turn?.tool_status).toBe("Reading workspace");
    expect(store.currentRoom.window.queued_seats).toEqual(["agent:scout"]);
    expect(store.currentRoom.window.paused_at_cap).toBe(true);
    expect(store.currentRoom.window.window_used).toBe(3);
    expect(store.currentRoom.window.selector_error).toBe(
      "provider rejected speaker selection: Input blocked.",
    );
    expect(store.turnStates[id]?.selectorError).toBe(
      "provider rejected speaker selection: Input blocked.",
    );
    expect(store.rooms[0]?.paused_at_cap).toBe(true);

    store.dismissSelectorError(id);
    expect(store.currentRoom.window.selector_error).toBeNull();
    expect(store.turnStates[id]?.selectorError).toBeNull();
  });

  it("handles room_deleted frame by removing from list and clearing currentRoom", () => {
    const store = new RoomsStore();
    const id = "room_01jcv8k9m2p4q7";
    store.rooms = [makeSummary(id), makeSummary("room_01jcv8k9m2p4q8")];
    store.currentRoom = makeDetail(id);
    store.messages = [makeMessage(1)];
    store.minutesMarkdown = "# Minutes";
    store.setViewingRoom(id);

    store.handleFrame({ type: "room_deleted", room_id: id });

    expect(store.rooms.map((r) => r.id)).toEqual(["room_01jcv8k9m2p4q8"]);
    expect(store.currentRoom).toBeNull();
    expect(store.messages).toEqual([]);
    expect(store.minutesMarkdown).toBeNull();
    expect(store.viewingRoomId).toBeNull();
  });

  it("supports create, patch, postMessage, continue, stop, archive, reopen, and delete actions", async () => {
    const id = "room_01jcv8k9m2p4q7";
    const detail = makeDetail(id);
    const detailResp: RoomDetailResponse = {
      room: detail,
      messages: [makeMessage(1)],
      minutes_markdown: null,
      decision_model_active: true,
    };

    const fetchMock = vi.fn((url: string, init?: RequestInit) => {
      if (url === `/api/hub/rooms/${id}?limit=200` && (!init?.method || init.method === "GET")) {
        return Promise.resolve(jsonResponse(detailResp));
      }
      if (url === "/api/hub/rooms" && init?.method === "POST") {
        return Promise.resolve(jsonResponse(detailResp, 201));
      }
      if (url === `/api/hub/rooms/${id}` && init?.method === "PATCH") {
        return Promise.resolve(jsonResponse({ ...detail, pinned: true }));
      }
      if (url === `/api/hub/rooms/${id}/messages` && init?.method === "POST") {
        return Promise.resolve(
          jsonResponse(makeMessage(2, { kind: "owner", seat_id: null, author_name: "Owner" }), 202),
        );
      }
      if (url === `/api/hub/rooms/${id}/continue` && init?.method === "POST") {
        return Promise.resolve(jsonResponse(detail, 202));
      }
      if (url === `/api/hub/rooms/${id}/stop` && init?.method === "POST") {
        return Promise.resolve(jsonResponse({ ...detail, active_turn: null }));
      }
      if (url === `/api/hub/rooms/${id}/archive` && init?.method === "POST") {
        return Promise.resolve(
          jsonResponse({
            ...detailResp,
            room: { ...detail, status: "archived" },
            minutes_markdown: "# Meeting Minutes",
          }),
        );
      }
      if (url === `/api/hub/rooms/${id}/reopen` && init?.method === "POST") {
        return Promise.resolve(jsonResponse({ ...detail, status: "active" }));
      }
      if (url === `/api/hub/rooms/${id}` && init?.method === "DELETE") {
        return Promise.resolve(new Response(null, { status: 204 }));
      }
      return Promise.resolve(new Response("unexpected", { status: 500 }));
    });
    vi.stubGlobal("fetch", fetchMock);

    const store = new RoomsStore();
    const created = await store.createRoom({
      name: "Architecture Sync",
      topic: "API review",
      seat_ids: ["agent:scout", "agent:analyst"],
      pinned: false,
    });
    expect(created?.room.id).toBe(id);
    expect(store.currentRoom).toBeNull();
    expect(store.rooms.map((r) => r.id)).toEqual([id]);

    await store.openRoom(id);
    expect(store.currentRoom?.id).toBe(id);

    const patched = await store.patchRoom(id, { pinned: true });
    expect(patched?.pinned).toBe(true);

    const posted = await store.postMessage(id, "Hello team", []);
    expect(posted?.seq).toBe(2);
    expect(store.messages.map((m) => m.seq)).toEqual([1, 2]);

    expect(await store.continueRoom(id)).not.toBeNull();
    expect(await store.stopRoom(id)).not.toBeNull();

    const archived = await store.archiveRoom(id);
    expect(archived?.room.status).toBe("archived");
    expect(store.minutesMarkdown).toBe("# Meeting Minutes");

    const reopened = await store.reopenRoom(id);
    expect(reopened?.status).toBe("active");

    expect(await store.deleteRoom(id)).toBe(true);
    expect(store.rooms).toEqual([]);
    expect(store.currentRoom).toBeNull();
  });

  it("keeps currentRoom null on list-view patchRoom/archiveRoom so subsequent openRoom loads messages", async () => {
    const id = "room_01jcv8k9m2p4q7";
    const detail = makeDetail(id);
    const detailResp: RoomDetailResponse = {
      room: detail,
      messages: [makeMessage(1), makeMessage(2)],
      minutes_markdown: null,
      decision_model_active: true,
    };

    vi.stubGlobal(
      "fetch",
      vi.fn((url: string, init?: RequestInit) => {
        if (url === `/api/hub/rooms/${id}` && init?.method === "PATCH") {
          return Promise.resolve(jsonResponse({ ...detail, pinned: true }));
        }
        if (url === `/api/hub/rooms/${id}/archive` && init?.method === "POST") {
          return Promise.resolve(
            jsonResponse({
              ...detailResp,
              room: { ...detail, status: "archived", pinned: true },
              minutes_markdown: "# Minutes",
            }),
          );
        }
        if (url === `/api/hub/rooms/${id}?limit=200`) {
          return Promise.resolve(
            jsonResponse({
              ...detailResp,
              room: { ...detail, status: "archived", pinned: true },
              minutes_markdown: "# Minutes",
            }),
          );
        }
        return Promise.resolve(new Response("unexpected", { status: 500 }));
      }),
    );

    const store = new RoomsStore();
    store.rooms = [makeSummary(id)];

    await store.patchRoom(id, { pinned: true });
    expect(store.currentRoom).toBeNull();
    expect(store.rooms[0]?.pinned).toBe(true);

    await store.archiveRoom(id);
    expect(store.currentRoom).toBeNull();
    expect(store.messages).toEqual([]);
    expect(store.minutesMarkdown).toBeNull();
    expect(store.rooms[0]?.status).toBe("archived");

    await store.openRoom(id);
    expect(store.currentRoom?.id).toBe(id);
    expect(store.messages.map((m) => m.seq)).toEqual([1, 2]);
    expect(store.minutesMarkdown).toBe("# Minutes");
  });

  it("discards stale B response on rapid openRoom(A) -> openRoom(B) -> openRoom(A)", async () => {
    const roomA = "room_01jcv8k9m2p4q7";
    const roomB = "room_01jcv8k9m2p4q8";
    const deferred: Record<string, Array<(res: Response) => void>> = {
      [roomA]: [],
      [roomB]: [],
    };

    vi.stubGlobal(
      "fetch",
      vi.fn(
        (url: string) =>
          new Promise<Response>((resolve) => {
            if (url === `/api/hub/rooms/${roomA}?limit=200`) deferred[roomA]?.push(resolve);
            else if (url === `/api/hub/rooms/${roomB}?limit=200`) deferred[roomB]?.push(resolve);
            else resolve(new Response(null, { status: 204 }));
          }),
      ),
    );

    const store = new RoomsStore();
    const firstA = store.openRoom(roomA);
    const secondB = store.openRoom(roomB);
    const thirdA = store.openRoom(roomA);

    deferred[roomA]?.[1]?.(
      jsonResponse({
        room: makeDetail(roomA, { name: "Room A" }),
        messages: [makeMessage(1, { room_id: roomA, content: "A1" })],
        minutes_markdown: null,
        decision_model_active: true,
      }),
    );
    await thirdA;

    deferred[roomB]?.[0]?.(
      jsonResponse({
        room: makeDetail(roomB, { name: "Room B" }),
        messages: [makeMessage(1, { room_id: roomB, content: "B1" })],
        minutes_markdown: "# B Minutes",
        decision_model_active: true,
      }),
    );
    deferred[roomA]?.[0]?.(
      jsonResponse({
        room: makeDetail(roomA, { name: "Stale A" }),
        messages: [],
        minutes_markdown: null,
        decision_model_active: true,
      }),
    );
    expect(await secondB).toBeNull();
    expect(await firstA).toBeNull();

    expect(store.viewingRoomId).toBe(roomA);
    expect(store.currentRoom?.id).toBe(roomA);
    expect(store.currentRoom?.name).toBe("Room A");
    expect(store.messages.map((m) => m.content)).toEqual(["A1"]);
    expect(store.minutesMarkdown).toBeNull();
  });

  it("preserves a live WS room_message arriving while openRoom is in flight", async () => {
    const id = "room_01jcv8k9m2p4q7";
    let resolveFetch!: (res: Response) => void;

    vi.stubGlobal(
      "fetch",
      vi.fn((url: string, init?: RequestInit) => {
        if (url === `/api/hub/rooms/${id}?limit=200`) {
          return new Promise<Response>((resolve) => {
            resolveFetch = resolve;
          });
        }
        if (url === `/api/hub/rooms/${id}/read` && init?.method === "POST") {
          return Promise.resolve(new Response(null, { status: 204 }));
        }
        return Promise.resolve(new Response("unexpected", { status: 500 }));
      }),
    );

    const store = new RoomsStore();
    store.rooms = [makeSummary(id, { last_seq: 2, unread_count: 1 })];

    const opening = store.openRoom(id);
    expect(store.viewingRoomId).toBe(id);
    expect(store.messages).toEqual([]);

    // Live WS message seq=3 arrives before fetchRoom (seq 1..2) resolves.
    store.handleFrame({
      type: "room_message",
      room_id: id,
      message: makeMessage(3, { room_id: id, content: "Live msg" }),
      unread_count: 1,
    });
    expect(store.messages.map((m) => m.seq)).toEqual([3]);

    resolveFetch(
      jsonResponse({
        room: makeDetail(id, { last_seq: 2, last_read_seq: 1, unread_count: 1 }),
        messages: [makeMessage(1, { room_id: id }), makeMessage(2, { room_id: id })],
        minutes_markdown: null,
        decision_model_active: true,
      }),
    );
    await opening;

    expect(store.messages.map((m) => m.seq)).toEqual([1, 2, 3]);
    expect(store.currentRoom?.last_seq).toBe(3);
    expect(store.currentRoom?.unread_count).toBe(0);
    expect(store.rooms[0]?.unread_count).toBe(0);
  });

  it("keeps unread_count monotonic when out-of-order markRead arrives after a newer message", async () => {
    const id = "room_01jcv8k9m2p4q7";
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.resolve(new Response(null, { status: 204 }))),
    );

    const store = new RoomsStore();
    store.rooms = [makeSummary(id, { last_seq: 5, unread_count: 1 })];
    store.currentRoom = makeDetail(id, { last_seq: 5, last_read_seq: 4, unread_count: 1 });
    store.setViewingRoom(id);

    // Newer WS message seq=6 arrives while viewing, keeping unread_count=0 and advancing last_seq=6.
    store.handleFrame({
      type: "room_message",
      room_id: id,
      message: makeMessage(6, { room_id: id }),
      unread_count: 1,
    });
    expect(store.rooms[0]?.unread_count).toBe(0);
    expect(store.currentRoom.last_read_seq).toBe(6);

    // Out-of-order markRead for older seq=5 must never bump unread_count back to 1.
    await store.markRead(id, 5);
    expect(store.rooms[0]?.unread_count).toBe(0);
    expect(store.currentRoom.unread_count).toBe(0);
    expect(store.currentRoom.last_read_seq).toBe(6);
  });

  it("updates decisionModelActive on system_one_status frames and resyncs open room on refreshList", async () => {
    const id = "room_01jcv8k9m2p4q7";
    const fetchMock = vi.fn((url: string) => {
      if (url === "/api/hub/rooms") {
        return Promise.resolve(
          jsonResponse({
            rooms: [makeSummary(id, { unread_count: 2 })],
            decision_model_active: false,
          }),
        );
      }
      if (url === `/api/hub/rooms/${id}?limit=200`) {
        return Promise.resolve(
          jsonResponse({
            room: makeDetail(id, { last_seq: 3, last_read_seq: 3, unread_count: 0 }),
            messages: [makeMessage(1), makeMessage(2), makeMessage(3)],
            minutes_markdown: null,
            decision_model_active: false,
          }),
        );
      }
      return Promise.resolve(new Response("unexpected", { status: 500 }));
    });
    vi.stubGlobal("fetch", fetchMock);

    const store = new RoomsStore();
    store.handleFrame({
      type: "system_one_status",
      status: { configured: true, provider: "System One", model: "fast-model", outage: null },
    });
    expect(store.decisionModelActive).toBe(true);

    store.handleFrame({
      type: "system_one_status",
      status: {
        configured: true,
        provider: "System One",
        model: "fast-model",
        outage: {
          kind: "unreachable",
          message: "System 1 timed out",
          since: "2026-11-14T10:00:00Z",
        },
      },
    });
    expect(store.decisionModelActive).toBe(false);

    store.setViewingRoom(id);
    await store.refreshList();
    await Promise.resolve();
    expect(fetchMock).toHaveBeenCalledWith(`/api/hub/rooms/${id}?limit=200`, undefined);
    expect(store.messages.map((m) => m.seq)).toEqual([1, 2, 3]);
    expect(store.rooms[0]?.unread_count).toBe(0);
  });

  it("clears room state and discards in-flight openRoom when setViewingRoom(null) is called", async () => {
    const id = "room_01jcv8k9m2p4q7";
    let resolveFetch!: (res: Response) => void;
    vi.stubGlobal(
      "fetch",
      vi.fn(
        () =>
          new Promise<Response>((resolve) => {
            resolveFetch = resolve;
          }),
      ),
    );

    const store = new RoomsStore();
    const opening = store.openRoom(id);
    expect(store.loading).toBe(true);

    store.setViewingRoom(null);
    expect(store.viewingRoomId).toBeNull();
    expect(store.currentRoom).toBeNull();
    expect(store.messages).toEqual([]);
    expect(store.minutesMarkdown).toBeNull();
    expect(store.loading).toBe(false);

    resolveFetch(
      jsonResponse({
        room: makeDetail(id),
        messages: [makeMessage(1, { room_id: id })],
        minutes_markdown: "# Minutes",
        decision_model_active: true,
      }),
    );
    expect(await opening).toBeNull();
    expect(store.currentRoom).toBeNull();
    expect(store.messages).toEqual([]);
  });

  it("captureNotes posts mode='new', updates room detail/messages/minutes, and handles errors", async () => {
    const id = "room_01jcv8k9m2p4q7";
    const initialDetail = makeDetail(id, { last_seq: 2, last_read_seq: 2 });
    const capturedMeeting = {
      id: "rmtg_01jcv8m9000001",
      start_seq: 1,
      end_seq: 2,
      divider_seq: 3,
      shared_path: "team/minutes/2026-11-14-architecture-sync.md",
      inbox_ref: "scout/user/min-1",
      created_at: "2026-11-14T10:06:00Z",
      updated_at: "2026-11-14T10:06:00Z",
    };
    const dividerMsg = makeMessage(3, {
      room_id: id,
      kind: "system",
      seat_id: null,
      author_name: "System",
      seat_kind: null,
      content: "Notes captured — saved to `team/minutes/2026-11-14-architecture-sync.md`.",
    });

    let failNext = false;
    const fetchMock = vi.fn((url: string, init?: RequestInit) => {
      if (url === `/api/hub/rooms/${id}?limit=200` && (!init?.method || init.method === "GET")) {
        return Promise.resolve(
          jsonResponse({
            room: initialDetail,
            messages: [makeMessage(1, { room_id: id }), makeMessage(2, { room_id: id })],
            minutes_markdown: null,
            decision_model_active: true,
          } satisfies RoomDetailResponse),
        );
      }
      if (url === `/api/hub/rooms/${id}/capture-notes` && init?.method === "POST") {
        if (failNext) {
          return Promise.resolve(jsonResponse({ error: "no new messages" }, 409));
        }
        return Promise.resolve(
          jsonResponse({
            room: makeDetail(id, {
              last_seq: 3,
              last_read_seq: 3,
              last_captured_seq: 3,
              meetings: [capturedMeeting],
              minutes: {
                shared_path: capturedMeeting.shared_path,
                archived_at_seq: 3,
                inbox_ref: capturedMeeting.inbox_ref,
                updated_at: "2026-11-14T10:06:00Z",
              },
            }),
            messages: [
              makeMessage(1, { room_id: id }),
              makeMessage(2, { room_id: id }),
              dividerMsg,
            ],
            minutes_markdown: "# Meeting Minutes\n\n## Summary\nCaptured slice 1.",
            decision_model_active: true,
          } satisfies RoomDetailResponse),
        );
      }
      return Promise.resolve(new Response("unexpected", { status: 500 }));
    });
    vi.stubGlobal("fetch", fetchMock);

    const store = new RoomsStore();
    await store.openRoom(id);

    const ok = await store.captureNotes(id);
    expect(ok).toBe(true);
    expect(fetchMock).toHaveBeenCalledWith(
      `/api/hub/rooms/${id}/capture-notes`,
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({ mode: "new" }),
      }),
    );
    expect(store.currentRoom?.meetings).toEqual([capturedMeeting]);
    expect(store.currentRoom?.last_captured_seq).toBe(3);
    expect(store.messages.map((m) => m.seq)).toEqual([1, 2, 3]);
    expect(store.minutesMarkdown).toContain("Captured slice 1.");
    expect(store.rooms[0]?.minutes_shared_path).toBe(capturedMeeting.shared_path);

    failNext = true;
    const failed = await store.captureNotes(id);
    expect(failed).toBe(false);
    expect(store.error).toContain("No new messages");
  });
});
