// ── Meeting rooms (Svelte 5 runes) ──────────────────────────────────
//
// Multi-agent meeting rooms via `/api/hub/rooms/*` and live hub WebSocket
// frames (`room_updated`, `room_message`, `room_turn_state`, `room_deleted`).
// `hub.svelte.ts` refreshes the list on socket connect/reconnect and routes
// room frames into `roomsStore.handleFrame`.

import {
  archiveRoom as apiArchiveRoom,
  continueRoom as apiContinueRoom,
  createRoom as apiCreateRoom,
  deleteRoom as apiDeleteRoom,
  fetchRoom,
  fetchRoomCandidates,
  fetchRooms,
  markRoomRead as apiMarkRoomRead,
  patchRoom as apiPatchRoom,
  postRoomMessage as apiPostRoomMessage,
  reopenRoom as apiReopenRoom,
  stopRoom as apiStopRoom,
  uploadFile,
  type UploadedFile,
} from "./api";
import { userErrorMessage } from "./errors";
import type {
  CreateRoomRequest,
  HubServerMessage,
  PatchRoomRequest,
  RoomActiveTurn,
  RoomDetail,
  RoomDetailResponse,
  RoomMessage,
  RoomRosterCandidate,
  RoomSummary,
} from "./hub-types";

const DEFAULT_PAGE_SIZE = 200;

export interface RoomTurnStateSnapshot {
  activeTurn: RoomActiveTurn | null;
  queuedSeatIds: string[];
  pausedAtCap: boolean;
  windowUsed: number;
  windowCap: number;
}

function byUpdatedDesc(a: RoomSummary, b: RoomSummary): number {
  if (a.updated_at === b.updated_at) {
    if (a.id === b.id) return 0;
    return a.id < b.id ? -1 : 1;
  }
  return a.updated_at > b.updated_at ? -1 : 1;
}

function summaryFromDetail(detail: RoomDetail): RoomSummary {
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

function mergeSortedMessages(
  existing: readonly RoomMessage[],
  incoming: readonly RoomMessage[],
): RoomMessage[] {
  const bySeq: Record<number, RoomMessage> = {};
  for (const msg of existing) bySeq[msg.seq] = msg;
  for (const msg of incoming) bySeq[msg.seq] = msg;
  return Object.values(bySeq).sort((a, b) => a.seq - b.seq);
}

function turnStateFromDetail(detail: RoomDetail): RoomTurnStateSnapshot {
  return {
    activeTurn: detail.active_turn,
    queuedSeatIds: detail.window.queued_seats,
    pausedAtCap: detail.window.paused_at_cap,
    windowUsed: detail.window.window_used,
    windowCap: detail.window.window_cap,
  };
}

export class RoomsStore {
  /** All rooms (active + archived), newest update first. */
  rooms = $state<RoomSummary[]>([]);
  /** Eligible local and online A2A agents for room rosters. */
  candidates = $state<RoomRosterCandidate[]>([]);
  /** Whether the hub's System 1 decision model is currently configured and healthy. */
  decisionModelActive = $state(false);
  /** The currently opened room detail, or null. */
  currentRoom = $state<RoomDetail | null>(null);
  /** Transcript slice for `currentRoom`, oldest to newest. */
  messages = $state<RoomMessage[]>([]);
  /** Attached meeting minutes markdown for `currentRoom`, or null. */
  minutesMarkdown = $state<string | null>(null);
  /** Whether the initial list has completed at least once. */
  loaded = $state(false);
  /** Whether a room detail load is currently in flight. */
  loading = $state(false);
  /** Whether older messages are currently being fetched. */
  loadingOlder = $state(false);
  /** Plain-language error from the last failed room operation, or null. */
  error = $state<string | null>(null);
  /** Whether older transcript messages exist before `messages[0]`. */
  hasMoreMessages = $state(false);
  /** Room ID the user is currently viewing on screen (drives auto-read). */
  viewingRoomId = $state<string | null>(null);
  /** Live turn-window state per room id. */
  turnStates = $state<Record<string, RoomTurnStateSnapshot>>({});

  readonly activeRooms = $derived(this.rooms.filter((room) => room.status === "active"));
  readonly archivedRooms = $derived(this.rooms.filter((room) => room.status === "archived"));
  readonly pinnedRooms = $derived(
    this.rooms.filter((room) => room.status === "active" && room.pinned),
  );

  private listRequest = 0;
  private openRequest = 0;

  /** Fetch all rooms and System 1 availability. Never rejects so background connect callers stay safe. */
  async refreshList(): Promise<void> {
    const request = ++this.listRequest;
    if (this.viewingRoomId !== null) {
      void this.openRoom(this.viewingRoomId);
    }
    try {
      const response = await fetchRooms();
      if (request !== this.listRequest) return;
      this.rooms = response.rooms
        .map((room) => (room.id === this.viewingRoomId ? { ...room, unread_count: 0 } : room))
        .sort(byUpdatedDesc);
      this.decisionModelActive = response.decision_model_active;
      this.loaded = true;
      this.error = null;
    } catch (err) {
      if (request !== this.listRequest) return;
      this.error = userErrorMessage(err, { action: "Couldn't load meeting rooms." });
    }
  }

  /** Fetch eligible local, sibling, and external A2A seats. */
  async refreshCandidates(): Promise<void> {
    try {
      const response = await fetchRoomCandidates();
      this.candidates = response.candidates;
      this.error = null;
    } catch (err) {
      this.error = userErrorMessage(err, { action: "Couldn't load eligible room participants." });
    }
  }

  /** Load a room's detail, latest transcript slice, and any attached minutes. */
  async openRoom(id: string, limit = DEFAULT_PAGE_SIZE): Promise<RoomDetailResponse | null> {
    if (this.viewingRoomId !== id) {
      this.setViewingRoom(id);
    }
    const request = ++this.openRequest;
    this.loading = true;
    this.error = null;
    try {
      const prevSummary = this.rooms.find((room) => room.id === id);
      const response = await fetchRoom(id, { limit });
      if (request !== this.openRequest) return null;
      this.applyDetailResponse(response);
      this.hasMoreMessages = this.messages.length > 0 && (this.messages[0]?.seq ?? 1) > 1;
      const maxSeq = this.currentRoom?.last_seq ?? response.room.last_seq;
      if (
        this.viewingRoomId === id &&
        maxSeq > 0 &&
        (maxSeq > response.room.last_read_seq ||
          response.room.unread_count > 0 ||
          (prevSummary?.unread_count ?? 0) > 0)
      ) {
        void this.markRead(id, maxSeq);
      }
      return response;
    } catch (err) {
      if (request !== this.openRequest) return null;
      this.error = userErrorMessage(err, {
        action: "Couldn't open the meeting room.",
        notFound: "That meeting room no longer exists.",
      });
      return null;
    } finally {
      if (request === this.openRequest) this.loading = false;
    }
  }

  /** Page older messages strictly before the earliest currently loaded message. */
  async loadOlderMessages(limit = DEFAULT_PAGE_SIZE): Promise<RoomMessage[]> {
    const room = this.currentRoom;
    const oldestSeq = this.messages[0]?.seq;
    if (room === null || !this.hasMoreMessages || this.loadingOlder || oldestSeq === undefined) {
      return [];
    }
    if (oldestSeq <= 1) {
      this.hasMoreMessages = false;
      return [];
    }
    this.loadingOlder = true;
    this.error = null;
    try {
      const response = await fetchRoom(room.id, { limit, before_seq: oldestSeq });
      if (this.currentRoom?.id !== room.id) return [];
      const older = response.messages.filter((msg) => msg.seq < oldestSeq);
      this.messages = mergeSortedMessages(older, this.messages);
      const newOldest = this.messages[0]?.seq ?? 1;
      this.hasMoreMessages = older.length > 0 && newOldest > 1;
      return older;
    } catch (err) {
      this.error = userErrorMessage(err, { action: "Couldn't load earlier messages." });
      return [];
    } finally {
      this.loadingOlder = false;
    }
  }

  async createRoom(request: CreateRoomRequest): Promise<RoomDetailResponse | null> {
    this.error = null;
    try {
      const response = await apiCreateRoom(request);
      this.applyDetailResponse(response);
      if (this.viewingRoomId === response.room.id) {
        this.hasMoreMessages = false;
      }
      return response;
    } catch (err) {
      this.error = userErrorMessage(err, { action: "Couldn't create the meeting room." });
      return null;
    }
  }

  async patchRoom(id: string, request: PatchRoomRequest): Promise<RoomDetail | null> {
    this.error = null;
    try {
      const detail = await apiPatchRoom(id, request);
      this.applyDetail(detail);
      return detail;
    } catch (err) {
      this.error = userErrorMessage(err, { action: "Couldn't update the meeting room." });
      return null;
    }
  }

  async postMessage(
    id: string,
    content: string,
    uploadIds: string[] = [],
  ): Promise<RoomMessage | null> {
    this.error = null;
    try {
      const message = await apiPostRoomMessage(id, { content, upload_ids: uploadIds });
      this.foldMessage(id, message, 0);
      return message;
    } catch (err) {
      this.error = userErrorMessage(err, { action: "Couldn't send the message." });
      return null;
    }
  }

  async uploadAttachment(
    file: File | Blob,
    filename: string,
    options?: { signal?: AbortSignal },
  ): Promise<UploadedFile> {
    return uploadFile(file, filename, options);
  }

  async continueRoom(id: string): Promise<RoomDetail | null> {
    this.error = null;
    try {
      const detail = await apiContinueRoom(id);
      this.applyDetail(detail);
      return detail;
    } catch (err) {
      this.error = userErrorMessage(err, { action: "Couldn't continue the room discussion." });
      return null;
    }
  }

  async stopRoom(id: string): Promise<RoomDetail | null> {
    this.error = null;
    try {
      const detail = await apiStopRoom(id);
      this.applyDetail(detail);
      return detail;
    } catch (err) {
      this.error = userErrorMessage(err, { action: "Couldn't stop the room turn." });
      return null;
    }
  }

  async archiveRoom(id: string): Promise<RoomDetailResponse | null> {
    this.error = null;
    try {
      const response = await apiArchiveRoom(id);
      this.applyDetailResponse(response);
      return response;
    } catch (err) {
      this.error = userErrorMessage(err, { action: "Couldn't archive the meeting room." });
      return null;
    }
  }

  async reopenRoom(id: string): Promise<RoomDetail | null> {
    this.error = null;
    try {
      const detail = await apiReopenRoom(id);
      this.applyDetail(detail);
      return detail;
    } catch (err) {
      this.error = userErrorMessage(err, { action: "Couldn't reopen the meeting room." });
      return null;
    }
  }

  async deleteRoom(id: string): Promise<boolean> {
    this.error = null;
    try {
      await apiDeleteRoom(id);
      this.removeRoomState(id);
      return true;
    } catch (err) {
      this.error = userErrorMessage(err, { action: "Couldn't delete the meeting room." });
      return false;
    }
  }

  async markRead(id: string, seq: number): Promise<void> {
    this.applyLocalRead(id, seq);
    try {
      await apiMarkRoomRead(id, seq);
    } catch {
      // Marking read is best-effort; the next refresh or WS frame reconciles state.
    }
  }

  /** Track which room is open in the viewport and discard stale fetches on room switch. */
  setViewingRoom(id: string | null): void {
    if (this.viewingRoomId === id) {
      if (id === null && this.currentRoom !== null) {
        this.currentRoom = null;
        this.messages = [];
        this.minutesMarkdown = null;
        this.hasMoreMessages = false;
      }
      return;
    }
    this.viewingRoomId = id;
    this.openRequest += 1;
    this.loading = false;
    if (id === null || this.currentRoom?.id !== id) {
      this.currentRoom = null;
      this.messages = [];
      this.minutesMarkdown = null;
      this.hasMoreMessages = false;
    }
  }

  /** Fold live hub WebSocket frames touching meeting rooms or System 1 status. */
  handleFrame(msg: HubServerMessage): void {
    switch (msg.type) {
      case "room_updated":
        this.upsertSummary(msg.room);
        if (this.currentRoom?.id === msg.room.id) {
          const unread = this.viewingRoomId === msg.room.id ? 0 : msg.room.unread_count;
          this.currentRoom = {
            ...this.currentRoom,
            name: msg.room.name,
            topic: msg.room.topic,
            status: msg.room.status,
            pinned: msg.room.pinned,
            created_at: msg.room.created_at,
            updated_at: msg.room.updated_at,
            archived_at: msg.room.archived_at,
            roster: msg.room.roster,
            last_seq: msg.room.last_seq,
            unread_count: unread,
            last_read_seq:
              unread === 0 ? msg.room.last_seq : Math.max(0, msg.room.last_seq - unread),
            window: {
              ...this.currentRoom.window,
              paused_at_cap: msg.room.paused_at_cap,
            },
            minutes: {
              ...this.currentRoom.minutes,
              shared_path: msg.room.minutes_shared_path,
            },
          };
        }
        break;
      case "room_message":
        this.foldMessage(msg.room_id, msg.message, msg.unread_count);
        break;
      case "room_turn_state":
        this.turnStates = {
          ...this.turnStates,
          [msg.room_id]: {
            activeTurn: msg.active_turn,
            queuedSeatIds: msg.queued_seat_ids,
            pausedAtCap: msg.paused_at_cap,
            windowUsed: msg.window_used,
            windowCap: msg.window_cap,
          },
        };
        this.rooms = this.rooms.map((room) =>
          room.id === msg.room_id ? { ...room, paused_at_cap: msg.paused_at_cap } : room,
        );
        if (this.currentRoom?.id === msg.room_id) {
          this.currentRoom = {
            ...this.currentRoom,
            active_turn: msg.active_turn,
            window: {
              ...this.currentRoom.window,
              queued_seats: [...msg.queued_seat_ids],
              paused_at_cap: msg.paused_at_cap,
              window_used: msg.window_used,
              window_cap: msg.window_cap,
            },
          };
        }
        break;
      case "room_deleted":
        this.removeRoomState(msg.room_id);
        break;
      case "system_one_status":
        this.decisionModelActive = msg.status.configured && msg.status.outage === null;
        break;
      case "hub_boot":
      case "pong":
      case "agents_snapshot":
      case "agent_state":
      case "agent_stopping":
      case "agent_created":
      case "agent_restored":
      case "agent_deleted":
      case "agent_activity":
      case "notice":
      case "hub_config_reloaded":
      case "team_event":
      case "agent_overview":
      case "artifact_updated":
      case "artifact_removed":
      case "subscribed":
      case "session_frame":
      case "session_relay_lagged":
      case "workspace_changed":
      case "workspace_resync":
      case "workspace_watch_unavailable":
        break;
    }
  }

  private applyDetailResponse(response: RoomDetailResponse): void {
    this.decisionModelActive = response.decision_model_active;
    this.turnStates = {
      ...this.turnStates,
      [response.room.id]: turnStateFromDetail(response.room),
    };
    if (this.viewingRoomId !== response.room.id) {
      this.upsertSummary(summaryFromDetail(response.room));
      return;
    }
    const mergedMessages = mergeSortedMessages(this.messages, response.messages);
    const newestMsg = mergedMessages.at(-1);
    const maxSeq = Math.max(response.room.last_seq, newestMsg?.seq ?? 0);
    const updatedAt =
      newestMsg !== undefined && newestMsg.timestamp > response.room.updated_at
        ? newestMsg.timestamp
        : response.room.updated_at;
    const detail: RoomDetail = {
      ...response.room,
      last_seq: maxSeq,
      updated_at: updatedAt,
    };
    this.currentRoom = detail;
    this.messages = mergedMessages;
    this.minutesMarkdown = response.minutes_markdown;
    this.upsertSummary(summaryFromDetail(detail));
  }

  private applyDetail(detail: RoomDetail): void {
    if (this.viewingRoomId === detail.id && this.currentRoom?.id === detail.id) {
      this.currentRoom = detail;
    }
    this.turnStates = {
      ...this.turnStates,
      [detail.id]: turnStateFromDetail(detail),
    };
    this.upsertSummary(summaryFromDetail(detail));
  }

  private upsertSummary(summary: RoomSummary): void {
    const viewing = this.viewingRoomId === summary.id;
    const normalized = viewing ? { ...summary, unread_count: 0 } : summary;
    this.rooms = [...this.rooms.filter((r) => r.id !== summary.id), normalized].sort(byUpdatedDesc);
  }

  private foldMessage(roomId: string, message: RoomMessage, serverUnreadCount: number): void {
    const viewing = this.viewingRoomId === roomId;
    const effectiveUnread = viewing ? 0 : serverUnreadCount;

    if (viewing || this.currentRoom?.id === roomId) {
      this.messages = mergeSortedMessages(this.messages, [message]);
    }
    if (this.currentRoom?.id === roomId) {
      const nextLastSeq = Math.max(this.currentRoom.last_seq, message.seq);
      const nextReadSeq = viewing
        ? Math.max(this.currentRoom.last_read_seq, message.seq)
        : Math.max(0, nextLastSeq - effectiveUnread);
      this.currentRoom = {
        ...this.currentRoom,
        last_seq: nextLastSeq,
        last_read_seq: nextReadSeq,
        unread_count: effectiveUnread,
        updated_at: message.timestamp,
      };
    }

    this.rooms = this.rooms
      .map((room) => {
        if (room.id !== roomId) return room;
        return {
          ...room,
          last_seq: Math.max(room.last_seq, message.seq),
          unread_count: effectiveUnread,
          updated_at: message.timestamp,
        };
      })
      .sort(byUpdatedDesc);

    if (viewing && serverUnreadCount > 0) {
      void this.markRead(roomId, message.seq);
    }
  }

  private applyLocalRead(roomId: string, seq: number): void {
    this.rooms = this.rooms.map((room) => {
      if (room.id !== roomId) return room;
      const clamped = Math.min(seq, room.last_seq);
      return {
        ...room,
        unread_count: Math.min(room.unread_count, Math.max(0, room.last_seq - clamped)),
      };
    });
    if (this.currentRoom?.id === roomId) {
      const clamped = Math.min(seq, this.currentRoom.last_seq);
      const nextRead = Math.max(this.currentRoom.last_read_seq, clamped);
      this.currentRoom = {
        ...this.currentRoom,
        last_read_seq: nextRead,
        unread_count: Math.min(
          this.currentRoom.unread_count,
          Math.max(0, this.currentRoom.last_seq - nextRead),
        ),
      };
    }
  }

  private removeRoomState(roomId: string): void {
    this.rooms = this.rooms.filter((room) => room.id !== roomId);
    if (roomId in this.turnStates) {
      this.turnStates = Object.fromEntries(
        Object.entries(this.turnStates).filter(([id]) => id !== roomId),
      );
    }
    if (this.currentRoom?.id === roomId) {
      this.currentRoom = null;
      this.messages = [];
      this.minutesMarkdown = null;
      this.hasMoreMessages = false;
    }
    if (this.viewingRoomId === roomId) {
      this.viewingRoomId = null;
      this.openRequest += 1;
      this.loading = false;
      this.messages = [];
      this.minutesMarkdown = null;
      this.hasMoreMessages = false;
    }
  }
}

export const roomsStore = new RoomsStore();
