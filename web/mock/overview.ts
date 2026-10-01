import type {
  AgentOverview,
  HubServerMessage,
  LastMessage,
  OverviewResponse,
} from "../src/lib/hub-types";
import { chatHistorySegment } from "./chat";
import type { MockEnv } from "./env";
import { json } from "./http";
import type { Route } from "./routes";
import type { MockAgent } from "./state";
import { byName } from "./util";

/**
 * The team overview of the hub, `src/hub/overview/`: what Home shows about
 * each agent beyond its state, activity and summary. The mock reads it from
 * the same data its routes serve (the conversation, the live sessions, the
 * inbox) whenever it is asked for or sent, and tells hub clients with
 * `agent_overview` frames, gathered the way the backend gathers them.
 */

/** How long an agent's changes are gathered before its overview is sent, in simulated milliseconds: `COALESCE_WINDOW` in `src/hub/overview/service.rs`. */
export const COALESCE_WINDOW_MS = 1000;

/** The most characters a preview holds, the ellipsis that marks a cut included. */
export const PREVIEW_CHARS = 200;

export interface MockOverview {
  /** Every agent's overview by name, like `GET /api/hub/overview`. */
  response: () => OverviewResponse;
  /**
   * Something about the agent's overview may have changed. Its frame goes out
   * when the window that this starts ends, and shows the agent as it is then.
   */
  changed: (agent: MockAgent) => void;
  /** Read a frame the hub sends to its clients: an agent starting or stopping, appearing, or going away. */
  observeHub: (frame: HubServerMessage) => void;
  /** Read the session frames an agent sends to its clients. */
  watchAgent: (agent: MockAgent) => void;
  /** Start over as a hub that has just started: nothing told, nothing waiting. */
  begin: () => void;
}

// ─── Previews ─────────────────────────────────────────────────────────────────

/** Each rule of `plainPreview`, applied in order. */
const MARKDOWN_RULES: ReadonlyArray<readonly [RegExp, string]> = [
  [/^[ \t]*```.*$/gm, " "],
  [/<!--[\s\S]*?-->/g, " "],
  [/<\/?[a-z][^>\n]*>/gi, ""],
  [/!\[([^\]\n]*)\]\([^)\n]*\)/g, "$1"],
  [/\[([^\]\n]*)\]\([^)\n]*\)/g, "$1"],
  [/`([^`\n]*)`/g, "$1"],
  [/^[ \t]{0,3}(?:[-*_][ \t]*){3,}$/gm, " "],
  [/^[ \t]*\|?[ \t:-]+(?:\|[ \t:-]+)+\|?[ \t]*$/gm, " "],
  [/^[ \t]{0,3}(?:#{1,6}|>|[-*+]|\d+[.)])[ \t]+(?:\[[ xX]\][ \t]+)?/gm, ""],
  [/\|/g, " "],
  [/(\*{1,2}|~~)(?=\S)([^*~\n]*?\S)\1/g, "$2"],
  [/(?<!\w)(_{1,2})(?=\S)([^_\n]*?\S)\1(?!\w)/g, "$2"],
];

/**
 * `markdown` as plain text on one line, cut to `PREVIEW_CHARS` characters
 * with a trailing `…`. A close match of the backend's `plain_preview`
 * (`src/hub/overview/preview.rs`), which parses the Markdown rather than
 * matching its marks.
 */
export function plainPreview(markdown: string): string {
  let text = markdown;
  for (const [pattern, replacement] of MARKDOWN_RULES) text = text.replace(pattern, replacement);
  const line = text.split(/\s+/).filter(Boolean).join(" ");
  const characters = Array.from(line);
  if (characters.length <= PREVIEW_CHARS) return line;
  return `${characters
    .slice(0, PREVIEW_CHARS - 1)
    .join("")
    .trimEnd()}…`;
}

// ─── Reading an agent ─────────────────────────────────────────────────────────

/** A stored time as RFC 3339 with whole seconds, the way the backend reports it. */
function rfc3339(iso: string): string {
  return new Date(Date.parse(iso)).toISOString().replace(/\.\d{3}Z$/, "Z");
}

/**
 * The newest message of the agent's main conversation that the user saw: the
 * newest text from the user or the agent in its recent history, else in the
 * newest episode that has one, dated to the day.
 */
function lastMessage(agent: MockAgent): LastMessage | null {
  let cursor: string | null = null;
  for (;;) {
    const segment = chatHistorySegment(agent.state, cursor);
    if (segment === null) return null;
    for (const message of [...segment.messages].reverse()) {
      if (message.role !== "user" && message.role !== "assistant") continue;
      if (segment.kind === "recent" && message.visibility !== "user") continue;
      const preview = plainPreview(message.content);
      if (preview === "") continue;
      return segment.kind === "recent"
        ? {
            role: message.role,
            preview,
            at: rfc3339(message.timestamp),
            at_precision: "minute",
          }
        : {
            role: message.role,
            preview,
            at: `${segment.date}T00:00:00Z`,
            at_precision: "day",
          };
    }
    if (segment.next_cursor === null) return null;
    cursor = segment.next_cursor;
  }
}

/** The agent's overview now, read from the data its routes serve. */
function overviewOf(agent: MockAgent): AgentOverview {
  const live =
    agent.runState === "running"
      ? [...agent.state.sessions.live].sort((a, b) => byName(a.started_at, b.started_at))
      : [];
  return {
    name: agent.name,
    last_message: lastMessage(agent),
    live_sessions: live.map((session) => ({
      address: session.address,
      run_id: session.run_id,
      category: session.category,
      source_label: session.source_label,
      purpose: session.purpose,
      state: session.state,
      started_at: session.started_at,
    })),
    upcoming: [],
    inbox_unread: agent.state.inboxItems.filter((item) => !item.read).length,
    outbound_problems: [],
  };
}

// ─── The overview ─────────────────────────────────────────────────────────────

/**
 * An overview of the hub's `agents` (the map the hub keeps current), whose
 * frames `send` delivers to every connected hub client.
 */
export function createOverview(
  env: MockEnv,
  bootId: string,
  agents: ReadonlyMap<string, MockAgent>,
  send: (frame: HubServerMessage) => void,
): MockOverview {
  /** What clients were last told of each agent: the last frame sent, or the answer to the request that first found it. */
  const told = new Map<string, string>();
  /** The cancel of the wait that ends with each agent's next frame. */
  const waiting = new Map<string, () => void>();

  /** Send the agent's overview if clients don't have it. */
  function flush(name: string): void {
    const agent = agents.get(name);
    if (agent === undefined) return;
    const overview = overviewOf(agent);
    const text = JSON.stringify(overview);
    if (told.get(name) === text) return;
    told.set(name, text);
    send({ type: "agent_overview", overview });
  }

  function changed(agent: MockAgent): void {
    if (waiting.has(agent.name)) return;
    waiting.set(
      agent.name,
      env.after(COALESCE_WINDOW_MS, () => {
        waiting.delete(agent.name);
        flush(agent.name);
      }),
    );
  }

  /** Stop waiting to send the agent's frame. */
  function stopWaiting(name: string): void {
    waiting.get(name)?.();
    waiting.delete(name);
  }

  return {
    response: () => {
      const sorted = [...agents.values()].sort((a, b) => byName(a.name, b.name));
      const overviews = sorted.map((agent) => {
        const overview = overviewOf(agent);
        const known = told.get(agent.name);
        if (known === undefined) told.set(agent.name, JSON.stringify(overview));
        else if (known !== JSON.stringify(overview)) changed(agent);
        return overview;
      });
      return { boot_id: bootId, agents: overviews };
    },
    changed,
    observeHub: (frame) => {
      if (frame.type === "agent_state") {
        const agent = agents.get(frame.agent.name);
        if (agent !== undefined) changed(agent);
      } else if (frame.type === "agent_created" || frame.type === "agent_restored") {
        stopWaiting(frame.agent.name);
        told.delete(frame.agent.name);
        flush(frame.agent.name);
      } else if (frame.type === "agent_deleted") {
        stopWaiting(frame.name);
        told.delete(frame.name);
      }
    },
    watchAgent: (agent) => {
      const sendToPages = agent.state.broadcast;
      agent.state.broadcast = (frame) => {
        sendToPages(frame);
        if (
          frame.type === "session_started" ||
          frame.type === "session_state_changed" ||
          frame.type === "session_completed"
        ) {
          changed(agent);
        }
      };
    },
    begin: () => {
      for (const name of [...waiting.keys()]) stopWaiting(name);
      told.clear();
    },
  };
}

// ─── Routes ───────────────────────────────────────────────────────────────────

/** The overview route, in the `/api/hub/...` spelling the hub keeps. */
export const overviewRoutes: readonly Route[] = [
  {
    method: "GET",
    pattern: "/api/hub/overview",
    handler: ({ res, hub }) => {
      json(res, 200, hub.overview.response());
    },
  },
];
