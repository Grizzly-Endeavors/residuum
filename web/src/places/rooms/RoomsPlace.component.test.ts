import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { tick } from "svelte";
import userEvent from "@testing-library/user-event";
import {
  jsonResponse,
  mockFetch,
  render,
  screen,
  settle,
  stubPointer,
  stubWebSocket,
} from "../../test/component";
import { waitFor } from "../../test/wait";
import { hub } from "../../lib/hub.svelte";
import { roomsStore } from "../../lib/rooms.svelte";
import { router } from "../../lib/router.svelte";
import type {
  AgentSummary,
  RoomDetail,
  RoomDetailResponse,
  RoomMessage,
  RoomRosterCandidate,
  RoomSeat,
  RoomSummary,
} from "../../lib/hub-types";
import { snapshot } from "../../test/hub-frames";
import RoomsPlace from "./RoomsPlace.svelte";

const ROOM_ID = "room_01jcv8k9m2p4q7";

const LOCAL_SEAT: RoomSeat = {
  seat_id: "agent:scout",
  kind: "local",
  display_name: "Scout",
  role: "Research specialist",
  instance: null,
};

const SIBLING_SEAT: RoomSeat = {
  seat_id: "a2a:ops/planner",
  kind: "sibling_a2a",
  display_name: "Planner",
  role: "Operations planner",
  instance: "ops",
};

const EXT_SEAT: RoomSeat = {
  seat_id: "a2a:partner-bot",
  kind: "external_a2a",
  display_name: "Partner Bot",
  role: "External advisor",
  instance: null,
};

const CANDIDATES: RoomRosterCandidate[] = [
  {
    seat_id: "agent:scout",
    kind: "local",
    display_name: "Scout",
    role: "Research specialist",
    instance: null,
    online: true,
  },
  {
    seat_id: "a2a:ops/planner",
    kind: "sibling_a2a",
    display_name: "Planner",
    role: "Operations planner",
    instance: "ops",
    online: true,
  },
  {
    seat_id: "a2a:partner-bot",
    kind: "external_a2a",
    display_name: "Partner Bot",
    role: "External advisor",
    instance: null,
    online: true,
  },
];

function agentSummary(name: string): AgentSummary {
  return {
    name,
    display_name: name,
    state: "running",
    last_error: null,
    autostart: true,
    role: null,
    a2a_visibility: "private",
    teams_configured: false,
  };
}

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
    roster: [LOCAL_SEAT, EXT_SEAT],
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
    roster: [LOCAL_SEAT, EXT_SEAT],
    historical_participants: ["agent:scout", "a2a:partner-bot"],
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
    room_id: ROOM_ID,
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

function resetRoomsStore(): void {
  roomsStore.setViewingRoom(null);
  roomsStore.rooms = [];
  roomsStore.candidates = [];
  roomsStore.decisionModelActive = true;
  roomsStore.currentRoom = null;
  roomsStore.messages = [];
  roomsStore.minutesMarkdown = null;
  roomsStore.loaded = false;
  roomsStore.loading = false;
  roomsStore.loadingOlder = false;
  roomsStore.error = null;
  roomsStore.hasMoreMessages = false;
  roomsStore.turnStates = {};
}

beforeEach(() => {
  stubPointer();
  stubWebSocket();
  Element.prototype.scrollIntoView = vi.fn();
  vi.spyOn(console, "error").mockImplementation(() => {});
  resetRoomsStore();
  hub.handleFrame(snapshot([agentSummary("scout")]));
  mockFetch((url) => {
    if (url === "/api/hub/rooms") {
      return jsonResponse({ rooms: [], decision_model_active: true });
    }
    if (url === "/api/hub/rooms/candidates") {
      return jsonResponse({ candidates: CANDIDATES });
    }
    return jsonResponse({}, 404);
  });
});

afterEach(() => {
  resetRoomsStore();
  vi.restoreAllMocks();
});

describe("RoomsPlace", () => {
  it("shows empty state and creates a room via NewRoomDialog with EXT A2A badge visibility", async () => {
    const user = userEvent.setup();
    const createdRoom: RoomDetailResponse = {
      room: makeDetail(ROOM_ID, { name: "Release Review", topic: "Check readiness", pinned: true }),
      messages: [],
      minutes_markdown: null,
      decision_model_active: true,
    };

    mockFetch((url, init) => {
      if (url === "/api/hub/rooms" && (!init?.method || init.method === "GET")) {
        return jsonResponse({ rooms: [], decision_model_active: true });
      }
      if (url === "/api/hub/rooms/candidates") {
        return jsonResponse({ candidates: CANDIDATES });
      }
      if (url === "/api/hub/rooms" && init?.method === "POST") {
        return jsonResponse(createdRoom, 201);
      }
      return jsonResponse({}, 404);
    });

    const openPlace = vi.spyOn(router, "openPlace").mockResolvedValue(true);

    render(RoomsPlace, { room: null, tab: "active" });

    expect(await screen.findByText("No active meeting rooms")).toBeInTheDocument();
    const newButtons = screen.getAllByRole("button", { name: "New room" });
    await user.click(newButtons[0] as HTMLElement);

    const dialog = await screen.findByRole("dialog", { name: "New room" });
    expect(dialog).toHaveTextContent("EXT A2A");
    expect(dialog).toHaveTextContent("@partner-bot");
    expect(dialog).not.toHaveTextContent("@scout");
    expect(dialog).not.toHaveTextContent("@planner");

    await user.type(screen.getByLabelText("Room name"), "Release Review");
    await user.type(screen.getByLabelText("Topic (optional)"), "Check readiness");
    await user.click(screen.getByRole("checkbox", { name: /Scout/ }));
    await user.click(screen.getByRole("checkbox", { name: /Partner Bot/ }));
    await user.click(screen.getByRole("switch", { name: /Pin to sidebar/ }));

    await user.click(screen.getByRole("button", { name: "Create room" }));

    await waitFor(() => {
      expect(openPlace).toHaveBeenCalledWith({
        kind: "rooms",
        room: ROOM_ID,
        tab: "active",
      });
    });
  });

  it("shows @mention-only banner when decisionModelActive is false and opens ?settings=_all/system_one", async () => {
    const user = userEvent.setup();
    const detail = makeDetail(ROOM_ID);
    mockFetch((url) => {
      if (url.startsWith(`/api/hub/rooms/${ROOM_ID}`)) {
        return jsonResponse({
          room: detail,
          messages: [makeMessage(1)],
          minutes_markdown: null,
          decision_model_active: false,
        } satisfies RoomDetailResponse);
      }
      return jsonResponse({}, 404);
    });

    const openSettings = vi.spyOn(router, "openSettings").mockResolvedValue(true);
    render(RoomsPlace, { room: ROOM_ID, tab: "active" });

    expect(await screen.findByText(/@mention-only mode/i)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /Configure Decision model/i }));
    expect(openSettings).toHaveBeenCalledWith({ scope: "_all", section: "system_one" });
  });

  it("displays EXT A2A badges in header, messages, and RoomRosterDrawer while enforcing >=1 active seat", async () => {
    const user = userEvent.setup();
    let detail = makeDetail(ROOM_ID, { roster: [LOCAL_SEAT, EXT_SEAT] });
    const extMessage = makeMessage(2, {
      seat_id: EXT_SEAT.seat_id,
      author_name: "Partner Bot",
      seat_kind: "external_a2a",
      content: "External note",
    });

    mockFetch((url, init) => {
      if (url.startsWith(`/api/hub/rooms/${ROOM_ID}`) && (!init?.method || init.method === "GET")) {
        return jsonResponse({
          room: detail,
          messages: [makeMessage(1), extMessage],
          minutes_markdown: null,
          decision_model_active: true,
        } satisfies RoomDetailResponse);
      }
      if (url === "/api/hub/rooms/candidates") {
        return jsonResponse({ candidates: CANDIDATES });
      }
      if (url === `/api/hub/rooms/${ROOM_ID}` && init?.method === "PATCH") {
        const raw = typeof init.body === "string" ? init.body : "{}";
        const body = JSON.parse(raw) as {
          add_seats?: string[];
          remove_seats?: string[];
        };
        if (body.remove_seats?.includes(EXT_SEAT.seat_id)) {
          detail = { ...detail, roster: [LOCAL_SEAT] };
        }
        if (body.add_seats?.includes(SIBLING_SEAT.seat_id)) {
          detail = { ...detail, roster: [...detail.roster, SIBLING_SEAT] };
        }
        return jsonResponse(detail);
      }
      return jsonResponse({}, 404);
    });

    render(RoomsPlace, { room: ROOM_ID, tab: "active" });

    await screen.findByText("External note");
    const extBadges = screen.getAllByText("EXT A2A");
    expect(extBadges.length).toBeGreaterThanOrEqual(2);

    await user.click(screen.getByRole("button", { name: /Roster/i }));
    const drawer = await screen.findByRole("dialog", { name: /Room roster/i });
    expect(drawer).toHaveTextContent("EXT A2A");
    expect(drawer).toHaveTextContent("@partner-bot");
    expect(drawer).not.toHaveTextContent("@scout");
    expect(drawer).not.toHaveTextContent("@planner");

    // Add Planner from available candidates
    await user.click(screen.getByRole("button", { name: "Add Planner" }));
    await waitFor(() => {
      expect(drawer).toHaveTextContent("ops");
    });

    // Remove Partner Bot, leaving 2 seats
    await user.click(screen.getByRole("button", { name: "Remove Partner Bot" }));
    await waitFor(() => {
      expect(screen.queryByRole("button", { name: "Remove Partner Bot" })).toBeNull();
    });

    // When only 1 seat remains, Remove must be disabled with >=1 explanation
    detail = { ...detail, roster: [LOCAL_SEAT] };
    roomsStore.currentRoom = detail;
    await tick();

    const removeScout = screen.getByRole("button", { name: "Remove Scout" });
    expect(removeScout).toBeDisabled();
    expect(drawer).toHaveTextContent(/at least 1 active participant/i);
  });

  it("supports @ autocomplete, file upload, live typing + Stop, and paused + Continue controls", async () => {
    const user = userEvent.setup();
    const detail = makeDetail(ROOM_ID, {
      roster: [LOCAL_SEAT, SIBLING_SEAT],
    });
    mockFetch((url) => {
      if (url.startsWith(`/api/hub/rooms/${ROOM_ID}`)) {
        return jsonResponse({
          room: detail,
          messages: [
            makeMessage(1),
            makeMessage(2, {
              kind: "system",
              seat_id: null,
              author_name: "System",
              seat_kind: null,
              content: "Planner went offline.",
            }),
          ],
          minutes_markdown: null,
          decision_model_active: true,
        } satisfies RoomDetailResponse);
      }
      return jsonResponse({}, 404);
    });

    const uploadSpy = vi.spyOn(roomsStore, "uploadAttachment").mockResolvedValue({
      id: "0123456789abcdef0123456789abcdef",
      filename: "notes.txt",
      size: 12,
    });
    const postSpy = vi.spyOn(roomsStore, "postMessage").mockResolvedValue(
      makeMessage(3, {
        kind: "owner",
        seat_id: null,
        author_name: "Owner",
        seat_kind: null,
        content: "@everyone Check notes",
      }),
    );
    const stopSpy = vi.spyOn(roomsStore, "stopRoom").mockResolvedValue(detail);
    const continueSpy = vi.spyOn(roomsStore, "continueRoom").mockResolvedValue(detail);

    render(RoomsPlace, { room: ROOM_ID, tab: "active" });

    await screen.findByText("Planner went offline.");

    const box = screen.getByRole("combobox", { name: /Message/i });
    await user.type(box, "@");
    const listbox = await screen.findByRole("listbox", { name: /Mention participant/i });
    expect(listbox).toHaveTextContent("everyone");
    expect(listbox).toHaveTextContent("Scout");
    expect(listbox).toHaveTextContent("Planner");

    // Enter selects @everyone
    await user.keyboard("{Enter}");
    expect(box).toHaveValue("@everyone ");

    // Also test selecting @scout by typing @sc + Arrow/Enter
    await user.type(box, "@sc");
    expect(screen.getByRole("listbox")).toHaveTextContent("Scout");
    await user.keyboard("{Enter}");
    expect(box).toHaveValue("@everyone @scout ");

    // Upload a file attachment
    const input = document.querySelector<HTMLInputElement>('input[type="file"]');
    expect(input).not.toBeNull();
    const file = new File(["hello world!"], "notes.txt", { type: "text/plain" });
    await user.upload(input as HTMLInputElement, file);
    await waitFor(() => {
      expect(uploadSpy).toHaveBeenCalledOnce();
      expect(screen.queryByRole("progressbar")).toBeNull();
    });

    // Shift+Enter adds newline, Enter submits
    await user.type(box, "line1{Shift>}{Enter}{/Shift}line2{Enter}");
    await waitFor(() => {
      expect(postSpy).toHaveBeenCalledWith(ROOM_ID, "@everyone @scout line1\nline2", [
        "0123456789abcdef0123456789abcdef",
      ]);
    });

    // Live typing state + Stop button
    roomsStore.handleFrame({
      type: "room_turn_state",
      room_id: ROOM_ID,
      active_turn: {
        seat_id: "agent:scout",
        display_name: "Scout",
        seat_kind: "local",
        started_at: "2026-11-14T10:10:00Z",
        tool_status: "Searching notes",
      },
      queued_seat_ids: ["a2a:ops/planner"],
      paused_at_cap: false,
      window_used: 1,
      window_cap: 3,
      selector_error: null,
    });
    await tick();

    expect(screen.getByText(/Scout is typing…/)).toBeInTheDocument();
    expect(screen.getByText("Searching notes")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Stop" }));
    expect(stopSpy).toHaveBeenCalledWith(ROOM_ID);

    // Between speakers (active_turn null, queued_seat_ids non-empty): Stop stays visible and composer stays held
    roomsStore.handleFrame({
      type: "room_turn_state",
      room_id: ROOM_ID,
      active_turn: null,
      queued_seat_ids: ["agent:scout"],
      paused_at_cap: false,
      window_used: 1,
      window_cap: 3,
      selector_error: null,
    });
    await tick();

    await user.type(box, "follow-up");
    expect(screen.getByText("Next participant starting…")).toBeInTheDocument();
    expect(screen.getByText("(1 queued)")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Stop" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Send" })).toBeDisabled();
    expect(screen.getByText("Wait for the current reply or press Stop")).toBeInTheDocument();

    // Paused at cap + Continue button
    roomsStore.handleFrame({
      type: "room_turn_state",
      room_id: ROOM_ID,
      active_turn: null,
      queued_seat_ids: [],
      paused_at_cap: true,
      window_used: 3,
      window_cap: 3,
      selector_error: null,
    });
    await tick();

    expect(screen.getByText("Paused after 3 agent replies")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Continue" }));
    expect(continueSpy).toHaveBeenCalledWith(ROOM_ID);
  });

  it("renders archived minutes, strips team/ prefix when opening in panel, and exposes Reopen room CTA", async () => {
    const user = userEvent.setup();
    const archivedDetail = makeDetail(ROOM_ID, {
      status: "archived",
      archived_at: "2026-11-14T11:00:00Z",
      minutes: {
        shared_path: "team/minutes/2026-11-14-architecture-sync.md",
        archived_at_seq: 2,
        inbox_ref: "scout/user/min-1",
        updated_at: "2026-11-14T11:00:00Z",
      },
    });

    mockFetch((url) => {
      if (url.startsWith(`/api/hub/rooms/${ROOM_ID}`)) {
        return jsonResponse({
          room: archivedDetail,
          messages: [makeMessage(1)],
          minutes_markdown: "# Meeting Minutes: Architecture Sync\n\n## Summary\n\nAligned on API.",
          decision_model_active: true,
        } satisfies RoomDetailResponse);
      }
      return jsonResponse({}, 404);
    });

    const openPanel = vi.spyOn(router, "openPanel").mockResolvedValue(true);
    const reopenSpy = vi.spyOn(roomsStore, "reopenRoom").mockResolvedValue({
      ...archivedDetail,
      status: "active",
      archived_at: null,
    });

    render(RoomsPlace, { room: ROOM_ID, tab: "archived" });
    await settle();

    expect(await screen.findByText(/Aligned on API\./)).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: /Message/i })).toBeDisabled();

    await user.click(screen.getByRole("button", { name: /Open minutes file/i }));
    expect(openPanel).toHaveBeenCalledWith({
      kind: "file",
      path: "minutes/2026-11-14-architecture-sync.md",
      agent: null,
    });

    const reopenButtons = screen.getAllByRole("button", { name: "Reopen room" });
    await user.click(reopenButtons[0] as HTMLElement);
    expect(reopenSpy).toHaveBeenCalledWith(ROOM_ID);
  });

  it("renders room list tabs, toggles pin, renders attachments, and confirms delete", async () => {
    const user = userEvent.setup();
    const activeSummary = makeSummary(ROOM_ID, {
      name: "Active Sync",
      pinned: true,
      unread_count: 3,
    });
    const archivedSummary = makeSummary("room_01jcv8k9m2p4q8", {
      name: "Old Retro",
      status: "archived",
      archived_at: "2026-11-14T08:00:00Z",
    });

    mockFetch((url) => {
      if (url === "/api/hub/rooms") {
        return jsonResponse({
          rooms: [activeSummary, archivedSummary],
          decision_model_active: true,
        });
      }
      return jsonResponse({}, 404);
    });

    const replacePlace = vi.spyOn(router, "replacePlace").mockResolvedValue(true);
    const { rerender } = render(RoomsPlace, { room: null, tab: "active" });

    expect(await screen.findByRole("link", { name: /Active Sync/ })).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: /Old Retro/ })).toBeNull();

    await user.click(screen.getByRole("tab", { name: /Archived/ }));
    expect(replacePlace).toHaveBeenCalledWith({
      kind: "rooms",
      room: null,
      tab: "archived",
    });

    const detail = makeDetail(ROOM_ID, { pinned: false });
    const msgWithAtt = makeMessage(1, {
      attachments: [
        {
          id: "0123456789abcdef0123456789abcdef",
          filename: "diagram.png",
          mime_type: "image/png",
          size: 2048,
          url: `/api/hub/rooms/${ROOM_ID}/attachments/0123456789abcdef0123456789abcdef`,
        },
      ],
    });
    mockFetch((url) => {
      if (url.startsWith(`/api/hub/rooms/${ROOM_ID}`)) {
        return jsonResponse({
          room: detail,
          messages: [msgWithAtt],
          minutes_markdown: null,
          decision_model_active: true,
        } satisfies RoomDetailResponse);
      }
      return jsonResponse({}, 404);
    });

    const patchSpy = vi.spyOn(roomsStore, "patchRoom").mockResolvedValue({
      ...detail,
      pinned: true,
    });
    const deleteSpy = vi.spyOn(roomsStore, "deleteRoom").mockResolvedValue(true);
    const openPlace = vi.spyOn(router, "openPlace").mockResolvedValue(true);

    await rerender({ room: ROOM_ID, tab: "active" });
    const img = await screen.findByRole("img", { name: "diagram.png" });
    expect(img).toHaveAttribute(
      "src",
      `/api/hub/rooms/${ROOM_ID}/attachments/0123456789abcdef0123456789abcdef`,
    );

    await user.click(screen.getByRole("button", { name: "Pin" }));
    expect(patchSpy).toHaveBeenCalledWith(ROOM_ID, { pinned: true });

    await user.click(screen.getByRole("button", { name: "Delete" }));
    const confirmDialog = await screen.findByRole("alertdialog");
    expect(confirmDialog).toHaveTextContent("Architecture Sync");
    await user.click(screen.getByRole("button", { name: "Delete room" }));

    await waitFor(() => {
      expect(deleteSpy).toHaveBeenCalledWith(ROOM_ID);
      expect(openPlace).toHaveBeenCalledWith({ kind: "rooms", room: null, tab: "active" });
    });
  });

  it("avoids duplicate error banners, supports dismissing list errors, and suppresses error banner on 404", async () => {
    const user = userEvent.setup();
    mockFetch((url, init) => {
      if (url === "/api/hub/rooms" && (!init?.method || init.method === "GET")) {
        return jsonResponse({ error: "hub unreachable" }, 503);
      }
      if (url === "/api/hub/rooms/candidates") {
        return jsonResponse({ candidates: CANDIDATES });
      }
      if (url === "/api/hub/rooms" && init?.method === "POST") {
        return jsonResponse({ error: "room quota reached" }, 400);
      }
      if (url.startsWith(`/api/hub/rooms/${ROOM_ID}`)) {
        return jsonResponse({ error: "room not found" }, 404);
      }
      return jsonResponse({}, 404);
    });

    const { rerender } = render(RoomsPlace, { room: null, tab: "active" });

    // List error banner is dismissable
    expect(await screen.findByRole("alert")).toHaveTextContent(/Couldn't load meeting rooms/i);
    await user.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(screen.queryByRole("alert")).toBeNull();

    // Failed createRoom shows error once inside NewRoomDialog (not duplicated on list page)
    await user.click(screen.getAllByRole("button", { name: "New room" })[0] as HTMLElement);
    await screen.findByRole("dialog", { name: "New room" });
    await user.type(screen.getByLabelText("Room name"), "Quota Test");
    await user.click(screen.getByRole("checkbox", { name: /Scout/ }));
    await user.click(screen.getByRole("button", { name: "Create room" }));

    await waitFor(() => {
      expect(screen.getAllByRole("alert")).toHaveLength(1);
    });
    expect(roomsStore.error).toBeNull();
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("alert")).toBeNull();

    // 404 openRoom shows Room not found EmptyState without duplicate top error banner
    await rerender({ room: ROOM_ID, tab: "active" });
    expect(await screen.findByText("Room not found")).toBeInTheDocument();
    expect(screen.queryByRole("alert")).toBeNull();
    expect(roomsStore.error).not.toBeNull();

    // Navigating back from 404 (<id> -> null) clears stale roomsStore.error
    roomsStore.loaded = true;
    await rerender({ room: null, tab: "active" });
    expect(roomsStore.error).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("resets open modals on room switch, keeps historical sibling badge, aborts in-flight uploads on unmount, and toggles minutes", async () => {
    const user = userEvent.setup();
    const roomB = "room_01jcv8k9m2p4q8";
    const historicalSiblingMsg = makeMessage(1, {
      seat_id: "a2a:ops/planner",
      author_name: "Planner",
      seat_kind: "sibling_a2a",
      content: "Historical sibling message",
    });

    let uploadSignal: AbortSignal | undefined;
    vi.spyOn(roomsStore, "uploadAttachment").mockImplementation((_file, _name, options) => {
      uploadSignal = options?.signal;
      return new Promise(() => {});
    });

    mockFetch((url) => {
      if (url.startsWith(`/api/hub/rooms/${ROOM_ID}`)) {
        return jsonResponse({
          room: makeDetail(ROOM_ID, {
            status: "archived",
            roster: [LOCAL_SEAT],
            historical_participants: [LOCAL_SEAT.seat_id, "a2a:ops/planner"],
            minutes: {
              shared_path: "team/minutes/2026-11-14-sync.md",
              archived_at_seq: 1,
              inbox_ref: null,
              updated_at: "2026-11-14T11:00:00Z",
            },
          }),
          messages: [historicalSiblingMsg],
          minutes_markdown: "## Summary\n\nDetailed minutes body.",
          decision_model_active: true,
        } satisfies RoomDetailResponse);
      }
      if (url.startsWith(`/api/hub/rooms/${roomB}`)) {
        return jsonResponse({
          room: makeDetail(roomB, { name: "Room B", roster: [LOCAL_SEAT] }),
          messages: [makeMessage(1, { room_id: roomB })],
          minutes_markdown: null,
          decision_model_active: true,
        } satisfies RoomDetailResponse);
      }
      if (url === "/api/hub/rooms/candidates") {
        return jsonResponse({ candidates: CANDIDATES });
      }
      return jsonResponse({}, 404);
    });

    const { rerender, unmount } = render(RoomsPlace, { room: ROOM_ID, tab: "archived" });

    // Historical sibling badge still renders even though a2a:ops/planner was removed from roster
    await screen.findByText("Historical sibling message");
    expect(screen.getByText("ops")).toBeInTheDocument();

    // Minutes header keeps Open minutes file button visible even when collapsed
    const minutesToggle = screen.getByRole("button", { name: "Meeting minutes" });
    expect(minutesToggle).toHaveAttribute("aria-expanded", "true");
    await user.click(minutesToggle);
    expect(minutesToggle).toHaveAttribute("aria-expanded", "false");
    expect(screen.getByRole("button", { name: "Open minutes file" })).toBeVisible();

    // Open roster drawer, then switch rooms: drawer must close on room change
    await user.click(screen.getByRole("button", { name: /Roster/i }));
    expect(await screen.findByRole("dialog", { name: /Room roster/i })).toBeInTheDocument();

    await rerender({ room: roomB, tab: "active" });
    await screen.findByRole("heading", { level: 1, name: "Room B" });
    expect(screen.queryByRole("dialog", { name: /Room roster/i })).toBeNull();

    // Start an upload in Room B, then unmount: in-flight upload must be aborted
    const fileInput = document.querySelector<HTMLInputElement>('input[type="file"]');
    expect(fileInput).not.toBeNull();
    await user.upload(
      fileInput as HTMLInputElement,
      new File(["data"], "pending.txt", { type: "text/plain" }),
    );
    expect(uploadSignal?.aborted).toBe(false);

    unmount();
    expect(uploadSignal?.aborted).toBe(true);
  });

  it("renders per-room speaker-selection error banner when selector rejects a message and dismisses cleanly", async () => {
    const user = userEvent.setup();
    const openSettingsSpy = vi.spyOn(router, "openSettings");
    const detail = makeDetail(ROOM_ID);

    mockFetch((url) => {
      if (url === "/api/hub/rooms") {
        return jsonResponse({ rooms: [makeSummary(ROOM_ID)], decision_model_active: true });
      }
      if (url.startsWith(`/api/hub/rooms/${ROOM_ID}?`)) {
        return jsonResponse({
          room: detail,
          messages: [makeMessage(1)],
          minutes_markdown: null,
          decision_model_active: true,
        } satisfies RoomDetailResponse);
      }
      if (url.endsWith("/read")) return new Response(null, { status: 204 });
      return jsonResponse({}, 404);
    });

    render(RoomsPlace, { room: ROOM_ID, tab: "active" });
    await screen.findByText("Message 1");
    expect(screen.queryByRole("alert")).toBeNull();

    roomsStore.handleFrame({
      type: "room_turn_state",
      room_id: ROOM_ID,
      active_turn: null,
      queued_seat_ids: [],
      paused_at_cap: false,
      window_used: 0,
      window_cap: 3,
      selector_error:
        "provider rejected speaker selection: Input blocked: prompt contains sensitive words.",
    });
    await tick();

    const alert = screen.getByRole("alert");
    expect(alert).toHaveTextContent("Speaker selection failed");
    expect(alert).toHaveTextContent(
      "provider rejected speaker selection: Input blocked: prompt contains sensitive words.",
    );

    await user.click(screen.getByRole("button", { name: "Decision model settings" }));
    expect(openSettingsSpy).toHaveBeenCalledWith({ scope: "_all", section: "system_one" });

    await user.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("captures notes on active room, disables when caught up, and opens divider and earlier meeting files", async () => {
    const user = userEvent.setup();
    const meeting1 = {
      id: "rmtg_01jcv8m9000001",
      start_seq: 1,
      end_seq: 2,
      divider_seq: 3,
      shared_path: "team/minutes/2026-11-14-architecture-sync.md",
      inbox_ref: "scout/user/min-1",
      created_at: "2026-11-14T10:06:00Z",
      updated_at: "2026-11-14T10:06:00Z",
    };
    const meeting2 = {
      id: "rmtg_01jcv8m9000002",
      start_seq: 4,
      end_seq: 5,
      divider_seq: 6,
      shared_path: "team/minutes/2026-11-14-architecture-sync-2.md",
      inbox_ref: "scout/user/min-2",
      created_at: "2026-11-14T10:20:00Z",
      updated_at: "2026-11-14T10:20:00Z",
    };

    let captureCount = 0;
    mockFetch((url, init) => {
      if (url === "/api/hub/rooms") {
        return jsonResponse({ rooms: [makeSummary(ROOM_ID)], decision_model_active: true });
      }
      if (url.startsWith(`/api/hub/rooms/${ROOM_ID}?`)) {
        return jsonResponse({
          room: makeDetail(ROOM_ID, {
            last_seq: 5,
            last_read_seq: 5,
            last_captured_seq: 3,
            meetings: [meeting1],
            minutes: {
              shared_path: meeting1.shared_path,
              archived_at_seq: 3,
              inbox_ref: meeting1.inbox_ref,
              updated_at: meeting1.updated_at,
            },
          }),
          messages: [
            makeMessage(1),
            makeMessage(2),
            makeMessage(3, {
              kind: "system",
              seat_id: null,
              author_name: "System",
              seat_kind: null,
              content: `Notes captured — saved to \`${meeting1.shared_path}\`.`,
            }),
            makeMessage(4),
            makeMessage(5),
          ],
          minutes_markdown: "# Slice 1",
          decision_model_active: true,
        } satisfies RoomDetailResponse);
      }
      if (url === `/api/hub/rooms/${ROOM_ID}/capture-notes` && init?.method === "POST") {
        captureCount += 1;
        return jsonResponse({
          room: makeDetail(ROOM_ID, {
            last_seq: 6,
            last_read_seq: 6,
            last_captured_seq: 6,
            meetings: [meeting1, meeting2],
            minutes: {
              shared_path: meeting2.shared_path,
              archived_at_seq: 6,
              inbox_ref: meeting2.inbox_ref,
              updated_at: meeting2.updated_at,
            },
          }),
          messages: [
            makeMessage(1),
            makeMessage(2),
            makeMessage(3, {
              kind: "system",
              seat_id: null,
              author_name: "System",
              seat_kind: null,
              content: `Notes captured — saved to \`${meeting1.shared_path}\`.`,
            }),
            makeMessage(4),
            makeMessage(5),
            makeMessage(6, {
              kind: "system",
              seat_id: null,
              author_name: "System",
              seat_kind: null,
              content: `Notes captured — saved to \`${meeting2.shared_path}\`.`,
            }),
          ],
          minutes_markdown: "# Slice 2",
          decision_model_active: true,
        } satisfies RoomDetailResponse);
      }
      if (url.endsWith("/read")) return new Response(null, { status: 204 });
      return jsonResponse({}, 404);
    });

    const openPanel = vi.spyOn(router, "openPanel").mockResolvedValue(true);
    render(RoomsPlace, { room: ROOM_ID, tab: "active" });

    const captureBtn = await screen.findByRole("button", { name: "Capture notes" });
    expect(captureBtn).toBeEnabled();

    // Inline "Open notes" button on divider 1 opens meeting 1 in the panel
    const openNotesBtnsBefore = screen.getAllByRole("button", { name: "Open notes" });
    expect(openNotesBtnsBefore).toHaveLength(1);
    await user.click(openNotesBtnsBefore[0] as HTMLElement);
    expect(openPanel).toHaveBeenCalledWith({
      kind: "file",
      path: "minutes/2026-11-14-architecture-sync.md",
      agent: null,
    });

    // Click Capture notes -> updates room, disables button (caught up), shows earlier notes list
    await user.click(captureBtn);
    await waitFor(() => {
      expect(captureCount).toBe(1);
      expect(screen.getByRole("button", { name: "Capture notes" })).toBeDisabled();
    });

    expect(screen.getAllByRole("button", { name: "Open notes" })).toHaveLength(2);
    const earlierList = screen.getByRole("list", { name: "Earlier captured notes" });
    expect(earlierList).toHaveTextContent("2026-11-14-architecture-sync.md");

    await user.click(screen.getByRole("button", { name: "2026-11-14-architecture-sync.md" }));
    expect(openPanel).toHaveBeenCalledWith({
      kind: "file",
      path: "minutes/2026-11-14-architecture-sync.md",
      agent: null,
    });
  });

  it("shows @leafSlug handle only when display_name differs from leaf slug in NewRoomDialog and RoomRosterDrawer", async () => {
    const user = userEvent.setup();
    const customSiblingCandidate: RoomRosterCandidate = {
      seat_id: "a2a:maven/ivy-assistant",
      kind: "sibling_a2a",
      display_name: "Ivy",
      role: "Vault librarian",
      instance: "maven",
      online: true,
    };
    const customSiblingSeat: RoomSeat = {
      seat_id: "a2a:maven/ivy-assistant",
      kind: "sibling_a2a",
      display_name: "Ivy",
      role: "Vault librarian",
      instance: "maven",
    };

    mockFetch((url) => {
      if (url === "/api/hub/rooms") {
        return jsonResponse({ rooms: [], decision_model_active: true });
      }
      if (url === "/api/hub/rooms/candidates") {
        return jsonResponse({
          candidates: [CANDIDATES[0] as RoomRosterCandidate, customSiblingCandidate],
        });
      }
      if (url.startsWith(`/api/hub/rooms/${ROOM_ID}`)) {
        return jsonResponse({
          room: makeDetail(ROOM_ID, { roster: [LOCAL_SEAT, customSiblingSeat] }),
          messages: [makeMessage(1)],
          minutes_markdown: null,
          decision_model_active: true,
        } satisfies RoomDetailResponse);
      }
      if (url.endsWith("/read")) return new Response(null, { status: 204 });
      return jsonResponse({}, 404);
    });

    const { rerender } = render(RoomsPlace, { room: null, tab: "active" });
    await screen.findByText("No active meeting rooms");
    await user.click(screen.getAllByRole("button", { name: "New room" })[0] as HTMLElement);
    const newDialog = await screen.findByRole("dialog", { name: "New room" });
    expect(newDialog).toHaveTextContent("Ivy");
    expect(newDialog).toHaveTextContent("@ivy-assistant");
    expect(newDialog).not.toHaveTextContent("@scout");
    await user.click(screen.getByRole("button", { name: "Cancel" }));

    await rerender({ room: ROOM_ID, tab: "active" });
    await screen.findByText("Message 1");
    await user.click(screen.getByRole("button", { name: /Roster/i }));
    const drawer = await screen.findByRole("dialog", { name: /Room roster/i });
    expect(drawer).toHaveTextContent("Ivy");
    expect(drawer).toHaveTextContent("@ivy-assistant");
    expect(drawer).not.toHaveTextContent("@scout");
  });
});
