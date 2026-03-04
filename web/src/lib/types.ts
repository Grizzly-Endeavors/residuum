// ── Protocol + UI types ──────────────────────────────────────────────

// ── Client -> Server ─────────────────────────────────────────────────

export type ClientMessage =
  | { type: "send_message"; id: string; content: string }
  | { type: "set_verbose"; enabled: boolean }
  | { type: "ping" }
  | { type: "reload" }
  | { type: "server_command"; name: string; args: string | null }
  | { type: "inbox_add"; body: string };

// ── Server -> Client ─────────────────────────────────────────────────

export type ServerMessage =
  | { type: "turn_started"; reply_to: string }
  | { type: "tool_call"; id: string; name: string; arguments: unknown }
  | {
      type: "tool_result";
      tool_call_id: string;
      name: string;
      output: string;
      is_error: boolean;
    }
  | { type: "response"; reply_to: string; content: string }
  | { type: "system_event"; source: string; content: string }
  | { type: "broadcast_response"; content: string }
  | { type: "error"; reply_to: string | null; message: string }
  | { type: "pong" }
  | { type: "reloading" }
  | { type: "notice"; message: string };

// ── Chat history ─────────────────────────────────────────────────────

export interface ToolCallRecord {
  id: string;
  name: string;
  arguments: string;
}

export interface RecentMessage {
  role: "user" | "assistant" | "tool" | "system";
  content: string;
  tool_calls?: ToolCallRecord[];
  tool_call_id?: string;
  timestamp: string;
  project_context: string;
  visibility: "user" | "background";
}

// ── Status API ───────────────────────────────────────────────────────

export interface StatusResponse {
  mode: "setup" | "running";
}

// ── Feed items (UI rendering) ────────────────────────────────────────

export type ConnectionStatus =
  | "connecting"
  | "connected"
  | "disconnected";

export interface ToolCallState {
  id: string;
  name: string;
  arguments: string;
  status: "running" | "done" | "error";
  result?: string;
}

interface FeedItemBase {
  id: number;
}

export interface UserFeedItem extends FeedItemBase {
  kind: "user";
  content: string;
}

export interface AssistantFeedItem extends FeedItemBase {
  kind: "assistant";
  content: string;
}

export interface SystemFeedItem extends FeedItemBase {
  kind: "system";
  content: string;
}

export interface ErrorFeedItem extends FeedItemBase {
  kind: "error";
  content: string;
}

export interface NoticeFeedItem extends FeedItemBase {
  kind: "notice";
  content: string;
}

export interface DividerFeedItem extends FeedItemBase {
  kind: "divider";
  label: string;
}

export interface ToolGroupFeedItem extends FeedItemBase {
  kind: "tool-group";
  calls: ToolCallState[];
}

export interface CommandOutputFeedItem extends FeedItemBase {
  kind: "command-output";
  content: string;
}

export type FeedItem =
  | UserFeedItem
  | AssistantFeedItem
  | SystemFeedItem
  | ErrorFeedItem
  | NoticeFeedItem
  | DividerFeedItem
  | ToolGroupFeedItem
  | CommandOutputFeedItem;
