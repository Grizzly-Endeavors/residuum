// ── Reactive WebSocket connection (Svelte 5 runes) ──────────────────

import { SvelteMap } from "svelte/reactivity";
import type {
  ClientMessage,
  ServerMessage,
  RecentMessage,
  FeedItem,
  ToolGroupFeedItem,
  ToolCallState,
  ConnectionStatus,
  ImageAttachment,
} from "./types";

let feedIdCounter = 0;
function nextId(): number {
  return ++feedIdCounter;
}

class WsConnection {
  status = $state<ConnectionStatus>("disconnected");
  feed = $state<FeedItem[]>([]);
  isProcessing = $state(false);
  verbose = $state(false);

  private ws: WebSocket | null = null;
  private msgCounter = 0;
  private reconnectDelay = 1000;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private pingTimer: ReturnType<typeof setInterval> | null = null;
  private pendingToolCalls = new SvelteMap<string, ToolCallState>();

  constructor() {
    try {
      this.verbose = localStorage.getItem("residuum-verbose") === "true";
    } catch {
      // localStorage unavailable
    }
  }

  connect(): void {
    const proto = location.protocol === "https:" ? "wss:" : "ws:";
    const url = `${proto}//${location.host}/ws`;

    this.status = "connecting";
    this.ws = new WebSocket(url);

    this.ws.onopen = () => {
      this.status = "connected";
      this.reconnectDelay = 1000;
      if (this.verbose) {
        this.send({ type: "set_verbose", enabled: true });
      }
      this.startPing();
    };

    this.ws.onmessage = (e) => {
      try {
        const msg: ServerMessage = JSON.parse(e.data);
        this.handleMessage(msg);
      } catch {
        // ignore unparseable frames
      }
    };

    this.ws.onclose = () => {
      this.status = "disconnected";
      this.stopPing();
      this.scheduleReconnect();
    };

    this.ws.onerror = () => {
      this.status = "disconnected";
    };
  }

  disconnect(): void {
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    this.stopPing();
    if (this.ws) {
      this.ws.onclose = null;
      this.ws.close();
      this.ws = null;
    }
    this.status = "disconnected";
  }

  send(msg: ClientMessage): void {
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(msg));
    }
  }

  sendChat(content: string, images?: ImageAttachment[]): void {
    this.msgCounter++;
    const id = `web-${this.msgCounter}`;
    const msg: ClientMessage = images?.length
      ? { type: "send_message", id, content, images }
      : { type: "send_message", id, content };
    this.send(msg);
    this.feed.push({ id: nextId(), kind: "user", content, images });
    this.isProcessing = true;
  }

  setVerbose(enabled: boolean): void {
    this.verbose = enabled;
    try {
      localStorage.setItem("residuum-verbose", String(enabled));
    } catch {
      // localStorage unavailable
    }
    this.send({ type: "set_verbose", enabled });
  }

  loadHistory(messages: RecentMessage[]): void {
    this.feed.length = 0;
    this.pendingToolCalls.clear();
    if (!messages.length) return;

    // eslint-disable-next-line svelte/prefer-svelte-reactivity -- local non-reactive scratch map
    const toolCallItems = new Map<string, ToolCallState>();

    for (const msg of messages) {
      const content = msg.content || "";
      switch (msg.role) {
        case "user":
          this.feed.push({ id: nextId(), kind: "user", content });
          break;
        case "assistant": {
          if (content.trim()) {
            this.feed.push({ id: nextId(), kind: "assistant", content });
          }
          if (msg.tool_calls?.length) {
            const calls: ToolCallState[] = msg.tool_calls.map((tc) => {
              const call: ToolCallState = {
                id: tc.id,
                name: tc.name,
                arguments: tc.arguments || "",
                status: "done",
              };
              toolCallItems.set(tc.id, call);
              return call;
            });
            this.feed.push({ id: nextId(), kind: "tool-group", calls });
          }
          break;
        }
        case "tool": {
          if (msg.tool_call_id) {
            const call = toolCallItems.get(msg.tool_call_id);
            if (call && content) {
              call.result =
                (call.result ? call.result + "\n" : "") +
                "\u2500\u2500\u2500 result \u2500\u2500\u2500\n" +
                content;
            }
            toolCallItems.delete(msg.tool_call_id);
          }
          break;
        }
        case "system":
          break;
      }
    }

    this.feed.push({
      id: nextId(),
      kind: "divider",
      label: "\u2014 session resumed \u2014",
    });
  }

  appendFeedItem(item: FeedItem): void {
    this.feed.push(item);
  }

  // ── Private ──────────────────────────────────────────────────────────

  private handleMessage(msg: ServerMessage): void {
    switch (msg.type) {
      case "turn_started":
        this.isProcessing = true;
        break;

      case "tool_call":
        this.handleToolCall(msg);
        break;

      case "tool_result":
        this.handleToolResult(msg);
        break;

      case "response":
        this.isProcessing = false;
        if (msg.content) {
          this.feed.push({
            id: nextId(),
            kind: "assistant",
            content: msg.content,
          });
        }
        break;

      case "broadcast_response":
        if (msg.content) {
          this.feed.push({
            id: nextId(),
            kind: "assistant",
            content: msg.content,
          });
        }
        break;

      case "system_event":
        this.feed.push({
          id: nextId(),
          kind: "system",
          content: `[${msg.source}] ${msg.content}`,
        });
        break;

      case "error":
        this.isProcessing = false;
        this.feed.push({
          id: nextId(),
          kind: "error",
          content: msg.message,
        });
        break;

      case "notice":
        this.feed.push({
          id: nextId(),
          kind: "notice",
          content: msg.message,
        });
        break;

      case "reloading":
        this.feed.push({
          id: nextId(),
          kind: "system",
          content: "Gateway is reloading...",
        });
        break;

      case "pong":
        break;
    }
  }

  private handleToolCall(msg: Extract<ServerMessage, { type: "tool_call" }>): void {
    const argsText =
      typeof msg.arguments === "string" ? msg.arguments : JSON.stringify(msg.arguments, null, 2);

    const call: ToolCallState = {
      id: msg.id,
      name: msg.name,
      arguments: argsText,
      status: "running",
    };

    // Find or create a tool group at the end of the feed
    const last = this.feed[this.feed.length - 1];
    if (last?.kind === "tool-group") {
      last.calls.push(call);
    } else {
      this.feed.push({
        id: nextId(),
        kind: "tool-group",
        calls: [call],
      });
    }

    // Store the proxied reference from the $state feed so mutations
    // in handleToolResult go through Svelte's reactivity system
    const group = this.feed[this.feed.length - 1] as ToolGroupFeedItem;
    const lastCall = group.calls[group.calls.length - 1];
    if (lastCall) this.pendingToolCalls.set(msg.id, lastCall);
  }

  private handleToolResult(msg: Extract<ServerMessage, { type: "tool_result" }>): void {
    const call = this.pendingToolCalls.get(msg.tool_call_id);
    if (call) {
      call.status = msg.is_error ? "error" : "done";
      if (msg.output) {
        call.result =
          (call.result ? call.result + "\n" : "") +
          "\u2500\u2500\u2500 result \u2500\u2500\u2500\n" +
          msg.output;
      }
      this.pendingToolCalls.delete(msg.tool_call_id);
    }
  }

  private scheduleReconnect(): void {
    if (this.reconnectTimer) return;
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      this.connect();
    }, this.reconnectDelay);
    this.reconnectDelay = Math.min(this.reconnectDelay * 1.5, 15000);
  }

  private startPing(): void {
    this.stopPing();
    this.pingTimer = setInterval(() => {
      this.send({ type: "ping" });
    }, 30000);
  }

  private stopPing(): void {
    if (this.pingTimer) {
      clearInterval(this.pingTimer);
      this.pingTimer = null;
    }
  }
}

export const ws = new WsConnection();
