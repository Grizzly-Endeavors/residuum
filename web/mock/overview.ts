import type {
  AgentOverview,
  HubServerMessage,
  LastMessage,
  OutboundProblem,
  OverviewResponse,
  UpcomingKind,
  UpcomingRun,
} from "../src/lib/hub-types";
import { chatHistorySegment } from "./chat";
import type { MockEnv } from "./env";
import { json } from "./http";
import type { Route } from "./routes";
import { nextPulseRun } from "./scheduled";
import type { MockAgent } from "./state";
import { byName } from "./util";
import { rfc3339InZone } from "./zone";

export const COALESCE_WINDOW_MS = 1000;

export const PREVIEW_CHARS = 200;

export const MOST_UPCOMING = 3;

/** How long an agent must stay unreachable before its task is a problem, in milliseconds: `UNREACHABLE_NOTICE_AFTER` in `crates/a2a/src/client/tracker.rs`. */
export const OUTBOUND_NOTICE_MS = 600_000;

const NOTICE_SLACK_MS = 5;

export interface MockOverview {
  response: () => OverviewResponse;
  changed: (agent: MockAgent) => void;
  observeHub: (frame: HubServerMessage) => void;
  watchAgent: (agent: MockAgent) => void;
  begin: () => void;
}

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

function rfc3339(iso: string): string {
  return new Date(Date.parse(iso)).toISOString().replace(/\.\d{3}Z$/, "Z");
}

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

const KIND_ORDER: Record<UpcomingKind, number> = { pulse: 0, action: 1 };

function upcomingOf(agent: MockAgent, nowMs: number): UpcomingRun[] {
  const { pulses, actions } = agent.state.scheduled;
  const runs: Array<{ at: number; kind: UpcomingKind; name: string }> = [
    ...pulses.flatMap((pulse) => {
      const at = nextPulseRun(pulse, nowMs);
      return at === null ? [] : [{ at, kind: "pulse" as const, name: pulse.name }];
    }),
    ...actions.map((action) => ({
      at: Date.parse(action.run_at),
      kind: "action" as const,
      name: action.name,
    })),
  ];
  runs.sort(
    (a, b) => a.at - b.at || KIND_ORDER[a.kind] - KIND_ORDER[b.kind] || byName(a.name, b.name),
  );
  return runs
    .slice(0, MOST_UPCOMING)
    .map(({ at, kind, name }) => ({ kind, name, at: rfc3339InZone(at) }));
}

function noticeAt(task: MockAgent["state"]["outboundTasks"][number]): number | null {
  if (!task.open || task.unreachable_since === null) return null;
  return Date.parse(task.unreachable_since) + OUTBOUND_NOTICE_MS;
}

function outboundProblemsOf(agent: MockAgent, nowMs: number): OutboundProblem[] {
  if (agent.runState !== "running") return [];
  return agent.state.outboundTasks
    .filter((task) => (noticeAt(task) ?? Infinity) <= nowMs)
    .sort(
      (a, b) =>
        byName(a.unreachable_since ?? "", b.unreachable_since ?? "") ||
        byName(a.task_id, b.task_id),
    )
    .map((task) => ({
      task_id: task.task_id,
      remote_agent: task.agent,
      status_text: task.status_text,
      unreachable_since: task.unreachable_since ?? "",
    }));
}

function nextNoticeAt(agent: MockAgent, nowMs: number): number | null {
  if (agent.runState !== "running") return null;
  const coming = agent.state.outboundTasks
    .map(noticeAt)
    .filter((at): at is number => at !== null && at > nowMs);
  return coming.length === 0 ? null : Math.min(...coming);
}

function overviewOf(agent: MockAgent, nowMs: number): AgentOverview {
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
    upcoming: upcomingOf(agent, nowMs),
    inbox_unread: agent.state.inboxItems.filter((item) => !item.read).length,
    outbound_problems: outboundProblemsOf(agent, nowMs),
  };
}

export function createOverview(
  env: MockEnv,
  bootId: string,
  agents: ReadonlyMap<string, MockAgent>,
  send: (frame: HubServerMessage) => void,
): MockOverview {
  const told = new Map<string, string>();
  const waiting = new Map<string, () => void>();
  const noticeWaits = new Map<string, () => void>();

  function read(agent: MockAgent): AgentOverview {
    const now = env.clock.now();
    noticeWaits.get(agent.name)?.();
    noticeWaits.delete(agent.name);
    const at = nextNoticeAt(agent, now);
    if (at !== null) {
      noticeWaits.set(
        agent.name,
        env.after(at - now + NOTICE_SLACK_MS, () => {
          noticeWaits.delete(agent.name);
          if (env.clock.now() >= at) changed(agent);
        }),
      );
    }
    return overviewOf(agent, now);
  }

  function flush(name: string): void {
    const agent = agents.get(name);
    if (agent === undefined) return;
    const overview = read(agent);
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

  function stopWaiting(name: string): void {
    waiting.get(name)?.();
    waiting.delete(name);
    noticeWaits.get(name)?.();
    noticeWaits.delete(name);
  }

  return {
    response: () => {
      const sorted = [...agents.values()].sort((a, b) => byName(a.name, b.name));
      const overviews = sorted.map((agent) => {
        const overview = read(agent);
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
          frame.type === "session_completed" ||
          frame.type === "session_outbound_a2a_task"
        ) {
          changed(agent);
        }
      };
    },
    begin: () => {
      for (const name of [...waiting.keys(), ...noticeWaits.keys()]) stopWaiting(name);
      told.clear();
    },
  };
}

export const overviewRoutes: readonly Route[] = [
  {
    method: "GET",
    pattern: "/api/hub/overview",
    handler: ({ res, hub }) => {
      json(res, 200, hub.overview.response());
    },
  },
];
