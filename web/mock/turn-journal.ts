import type { QueuedMessage, ServerMessage, TurnInProgress } from "../src/lib/generated/protocol";

/**
 * The main conversation's turn in flight, as the backend's turn journal keeps
 * it, so a page that asks (`resync_turn`) is shown what the turn did before
 * it connected: every frame of the turn so far, each model call's streamed
 * pieces joined, and only the newest usage. It also keeps the people's
 * messages still waiting for the agent to read them.
 */
export class MockTurnJournal {
  private turn: TurnInProgress | null = null;
  private waiting: QueuedMessage[] = [];

  constructor(private readonly now: () => string) {}

  /** Note a frame the agent's socket broadcast. Frames of no main-agent turn are ignored. */
  record(frame: ServerMessage): void {
    if (frame.type === "message_queued") {
      this.waiting.push(structuredClone(frame.message));
      return;
    }
    if (frame.type === "message_withdrawn" || frame.type === "user_message") {
      this.waiting = this.waiting.filter((message) => message.id !== frame.id);
    }
    const turnId = turnOf(frame);
    if (turnId === null) return;
    if (frame.type === "turn_ended") {
      if (this.turn?.reply_to === turnId) this.turn = null;
      return;
    }
    const at = this.now();
    if (this.turn?.reply_to !== turnId) {
      this.turn = { reply_to: turnId, started_at: at, now: at, frames: [] };
    }
    const { frames } = this.turn;
    if (frame.type === "turn_usage") {
      this.turn.frames = frames.filter((entry) => entry.frame.type !== "turn_usage");
    }
    const last = this.turn.frames.at(-1)?.frame;
    if (
      last !== undefined &&
      (last.type === "text_delta" || last.type === "thinking_delta") &&
      last.type === frame.type &&
      last.call === frame.call
    ) {
      last.text += frame.text;
      return;
    }
    this.turn.frames.push({ at, frame: structuredClone(frame) });
  }

  /** The turn in flight so far, or `null` when none runs. */
  snapshot(): TurnInProgress | null {
    return this.turn === null ? null : { ...structuredClone(this.turn), now: this.now() };
  }

  /** The messages waiting for the agent to read them, oldest first. */
  queued(): QueuedMessage[] {
    return structuredClone(this.waiting);
  }

  /** Forget the turn and what waited for it, as a restarted agent would. */
  clear(): void {
    this.turn = null;
    this.waiting = [];
  }
}

/** The main-agent turn `frame` belongs to, or `null` for a frame of none. */
function turnOf(frame: ServerMessage): string | null {
  switch (frame.type) {
    case "user_message":
      return frame.turn_id;
    case "turn_started":
    case "turn_ended":
    case "turn_usage":
    case "tool_call":
    case "tool_result":
    case "text_delta":
    case "thinking_delta":
    case "thinking":
    case "stream_restart":
    case "broadcast_response":
      return frame.reply_to;
    case "response":
      // A message posted outside any turn names none.
      return frame.reply_to === "" ? null : frame.reply_to;
    case "post_turn_activity":
    case "file_attachment":
    case "error":
    case "pong":
    case "turn_snapshot":
    case "reloading":
    case "notice":
    case "inline_output":
    case "session_started":
    case "session_state_changed":
    case "session_completed":
    case "session_turn_started":
    case "session_turn_ended":
    case "session_tool_call":
    case "session_tool_result":
    case "session_turn_usage":
    case "session_broadcast_response":
    case "session_response":
    case "session_error":
    case "session_message_to_main":
    case "session_message_delivered":
    case "session_stop_requested":
    case "session_command_failed":
    case "session_outbound_a2a_task":
    case "artifact_updated":
    case "artifact_removed":
    case "workspace_changed":
    case "workspace_resync":
    case "workspace_watch_unavailable":
    case "message_queued":
    case "message_withdrawn":
    case "queued_message_unavailable":
      // The backend sends these on other channels than the main conversation.
      return null;
  }
}
