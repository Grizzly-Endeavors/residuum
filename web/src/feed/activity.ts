import { formatElapsed } from "../lib/format-usage";
import type { IconName } from "../lib/icons";
import { isWorkspacePath } from "../lib/markdown";
import type { ObservedTurn, TurnEnding } from "../lib/observed-turns.svelte";
import type { ThinkingFeedItem, ToolCallState } from "../lib/types";

export interface StepCall extends ToolCallState {
  server?: string | null;
}

export type StepStatus = "running" | "done" | "failed" | "stopped";

export interface StepTarget {
  kind: "path" | "session" | "query" | "code" | "name";
  text: string;
}

export interface ActivityStep {
  id: string;
  icon: IconName;
  verb: string;
  target: StepTarget | null;
  status: StepStatus;
  call: StepCall;
}

type Args = Record<string, unknown>;

interface ToolWords {
  icon: IconName;
  running: string;
  done: string;
  by?: string;
  bare?: readonly [running: string, done: string];
  target?: (args: Args, result: string | undefined) => StepTarget | null;
  summary: (n: number) => string;
}

const TARGET_MAX_CHARS = 120;

function argText(args: Args, key: string): string | null {
  const value = args[key];
  if (typeof value === "number") return String(value);
  return typeof value === "string" && value.trim() !== "" ? value.trim() : null;
}

function clip(text: string): string {
  const line = text.replace(/\s+/g, " ");
  return line.length > TARGET_MAX_CHARS ? `${line.slice(0, TARGET_MAX_CHARS - 1)}…` : line;
}

function targetOf(kind: StepTarget["kind"], ...keys: string[]) {
  return (args: Args): StepTarget | null => {
    for (const key of keys) {
      const text = argText(args, key);
      if (text !== null) return { kind, text: clip(text) };
    }
    return null;
  };
}

function pathTarget(key = "path") {
  return (args: Args): StepTarget | null => {
    const text = argText(args, key);
    if (text === null) return null;
    return { kind: isWorkspacePath(text) ? "path" : "code", text: clip(text) };
  };
}

function spawnedSession(_args: Args, result: string | undefined): StepTarget | null {
  const address = /Session (\S+) spawned/.exec(result ?? "")?.[1];
  return address === undefined ? null : { kind: "session", text: address };
}

function stoppedSession(args: Args): StepTarget | null {
  const address = argText(args, "address");
  if (address === null) return null;
  return { kind: address.startsWith("a2a:") ? "code" : "session", text: clip(address) };
}

function counted(verb: string, one: string, many: string) {
  return (n: number): string => `${verb} ${String(n)} ${n === 1 ? one : many}`;
}

function repeated(phrase: string) {
  return (n: number): string => (n === 1 ? phrase : `${phrase} ${String(n)} times`);
}

const TOOL_WORDS: Readonly<Record<string, ToolWords>> = {
  read_file: {
    icon: "file",
    running: "Reading",
    done: "Read",
    bare: ["Reading a file", "Read a file"],
    target: pathTarget(),
    summary: counted("read", "file", "files"),
  },
  write_file: {
    icon: "edit",
    running: "Writing",
    done: "Wrote",
    bare: ["Writing a file", "Wrote a file"],
    target: pathTarget(),
    summary: counted("wrote", "file", "files"),
  },
  edit_file: {
    icon: "edit",
    running: "Editing",
    done: "Edited",
    bare: ["Editing a file", "Edited a file"],
    target: pathTarget(),
    summary: counted("edited", "file", "files"),
  },
  exec: {
    icon: "terminal",
    running: "Running",
    done: "Ran",
    bare: ["Running a command", "Ran a command"],
    target: targetOf("code", "command"),
    summary: counted("ran", "command", "commands"),
  },
  memory_search: {
    icon: "search",
    running: "Searching memory",
    done: "Searched memory",
    by: "for",
    target: targetOf("query", "query"),
    summary: repeated("searched memory"),
  },
  memory_get: {
    icon: "memory",
    running: "Recalling a past conversation",
    done: "Recalled a past conversation",
    target: targetOf("code", "episode_id", "run_id"),
    summary: counted("recalled", "past conversation", "past conversations"),
  },
  ollama_web_search: {
    icon: "search",
    running: "Searching the web",
    done: "Searched the web",
    by: "for",
    target: targetOf("query", "query"),
    summary: repeated("searched the web"),
  },
  web_fetch: {
    icon: "page",
    running: "Reading",
    done: "Read",
    bare: ["Reading a page", "Read a page"],
    target: targetOf("code", "url"),
    summary: counted("read", "page", "pages"),
  },
  send_message: {
    icon: "send",
    running: "Sending a message",
    done: "Sent a message",
    by: "to",
    target: targetOf("name", "endpoint"),
    summary: counted("sent", "message", "messages"),
  },
  list_endpoints: {
    icon: "chat",
    running: "Checking channels",
    done: "Checked channels",
    summary: repeated("checked channels"),
  },
  list_conversations: {
    icon: "chat",
    running: "Listing conversations",
    done: "Listed conversations",
    by: "on",
    target: targetOf("name", "endpoint"),
    summary: repeated("listed conversations"),
  },
  switch_endpoint: {
    icon: "chat",
    running: "Switching channels",
    done: "Switched channels",
    by: "to",
    target: targetOf("name", "endpoint"),
    summary: repeated("switched channels"),
  },
  subagent_spawn: {
    icon: "layers",
    running: "Starting a session",
    done: "Started a session",
    target: spawnedSession,
    summary: counted("started", "session", "sessions"),
  },
  message_agent: {
    icon: "handoff",
    running: "Messaging",
    done: "Messaged",
    bare: ["Messaging another agent", "Messaged another agent"],
    target: targetOf("code", "to"),
    summary: counted("sent", "message to another agent", "messages to other agents"),
  },
  stop_agent: {
    icon: "stop",
    running: "Stopping",
    done: "Stopped",
    bare: ["Stopping a session", "Stopped a session"],
    target: stoppedSession,
    summary: counted("stopped", "session", "sessions"),
  },
  list_agents: {
    icon: "users",
    running: "Checking the team",
    done: "Checked the team",
    summary: repeated("checked the team"),
  },
  agent_create: {
    icon: "users",
    running: "Creating agent",
    done: "Created agent",
    bare: ["Creating an agent", "Created an agent"],
    target: targetOf("name", "name"),
    summary: counted("created", "agent", "agents"),
  },
  agent_delete: {
    icon: "trash",
    running: "Deleting agent",
    done: "Deleted agent",
    bare: ["Deleting an agent", "Deleted an agent"],
    target: targetOf("name", "name"),
    summary: counted("deleted", "agent", "agents"),
  },
  schedule_action: {
    icon: "clock",
    running: "Scheduling",
    done: "Scheduled",
    bare: ["Scheduling an action", "Scheduled an action"],
    target: targetOf("name", "name"),
    summary: counted("scheduled", "action", "actions"),
  },
  list_actions: {
    icon: "clock",
    running: "Checking scheduled actions",
    done: "Checked scheduled actions",
    summary: repeated("checked scheduled actions"),
  },
  cancel_action: {
    icon: "clock",
    running: "Cancelling scheduled action",
    done: "Cancelled scheduled action",
    target: targetOf("code", "id"),
    summary: counted("cancelled", "scheduled action", "scheduled actions"),
  },
  inbox_list: {
    icon: "inbox",
    running: "Checking its inbox",
    done: "Checked its inbox",
    by: "for",
    target: targetOf("query", "query"),
    summary: repeated("checked its inbox"),
  },
  user_inbox_list: {
    icon: "inbox",
    running: "Checking your inbox",
    done: "Checked your inbox",
    by: "for",
    target: targetOf("query", "query"),
    summary: repeated("checked your inbox"),
  },
  inbox_read: {
    icon: "inbox",
    running: "Reading inbox item",
    done: "Read inbox item",
    target: targetOf("code", "id"),
    summary: counted("read", "inbox item", "inbox items"),
  },
  inbox_archive: {
    icon: "archive",
    running: "Archiving inbox items",
    done: "Archived inbox items",
    summary: repeated("archived inbox items"),
  },
  inbox_restore: {
    icon: "restore",
    running: "Restoring inbox items",
    done: "Restored inbox items",
    summary: repeated("restored inbox items"),
  },
  user_inbox_add: {
    icon: "inbox",
    running: "Adding to your inbox",
    done: "Added to your inbox",
    target: targetOf("query", "title"),
    summary: counted("added", "item to your inbox", "items to your inbox"),
  },
  skill_activate: {
    icon: "spark",
    running: "Turning on skill",
    done: "Turned on skill",
    target: targetOf("name", "name"),
    summary: counted("turned on", "skill", "skills"),
  },
  skill_deactivate: {
    icon: "spark",
    running: "Turning off skill",
    done: "Turned off skill",
    target: targetOf("name", "name"),
    summary: counted("turned off", "skill", "skills"),
  },
  agent_keys_list: {
    icon: "key",
    running: "Checking saved keys",
    done: "Checked saved keys",
    summary: repeated("checked saved keys"),
  },
  agent_key_delete: {
    icon: "key",
    running: "Removing saved key",
    done: "Removed saved key",
    target: targetOf("name", "name"),
    summary: counted("removed", "saved key", "saved keys"),
  },
  file_bug_report: {
    icon: "bug",
    running: "Filing a bug report",
    done: "Filed a bug report",
    summary: counted("filed", "bug report", "bug reports"),
  },
  submit_feedback: {
    icon: "send",
    running: "Sending feedback",
    done: "Sent feedback",
    summary: repeated("sent feedback"),
  },
  a2a_task_update: {
    icon: "handoff",
    running: "Updating a task",
    done: "Updated a task",
    summary: repeated("updated a task"),
  },
  workspace_history: {
    icon: "clock",
    running: "Checking file history",
    done: "Checked file history",
    by: "for",
    target: pathTarget(),
    summary: repeated("checked file history"),
  },
  workspace_restore: {
    icon: "restore",
    running: "Restoring",
    done: "Restored",
    bare: ["Restoring earlier versions", "Restored earlier versions"],
    target: pathTarget(),
    summary: repeated("restored earlier versions"),
  },
};

function otherToolWords(call: StepCall): ToolWords {
  const tool = call.server ? `${call.server}: ${call.name}` : call.name;
  return {
    icon: "bolt",
    running: `Using ${tool}`,
    done: `Used ${tool}`,
    summary: repeated(`used ${tool}`),
  };
}

function wordsFor(call: StepCall): ToolWords {
  const builtIn = !call.server && Object.hasOwn(TOOL_WORDS, call.name);
  return (builtIn ? TOOL_WORDS[call.name] : undefined) ?? otherToolWords(call);
}

function statusOf(call: ToolCallState): StepStatus {
  if (call.status === "error") return "failed";
  return call.status;
}

export function activitySteps(calls: readonly StepCall[]): ActivityStep[] {
  return calls.map((call) => {
    const words = wordsFor(call);
    const status = statusOf(call);
    const target = words.target?.(call.arguments, call.result) ?? null;
    const running = status === "running";
    let verb = running ? words.running : words.done;
    if (target === null && words.bare) verb = running ? words.bare[0] : words.bare[1];
    else if (target !== null && words.by) verb = `${verb} ${words.by}`;
    return { id: call.id, icon: words.icon, verb, target, status, call };
  });
}

export function stepText(step: ActivityStep): string {
  if (step.target === null) return step.verb;
  const target = step.target.kind === "query" ? `“${step.target.text}”` : step.target.text;
  return `${step.verb} ${target}`;
}

const SUMMARY_PHRASES = 4;

export function stepsPhrase(calls: readonly StepCall[]): string {
  const groups = new Map<string, { words: ToolWords; count: number }>();
  for (const call of calls) {
    const key = call.server ? `${call.server}\u0000${call.name}` : call.name;
    const group = groups.get(key);
    if (group) group.count++;
    else groups.set(key, { words: wordsFor(call), count: 1 });
  }
  const all = [...groups.values()];
  const shown = all.length > SUMMARY_PHRASES ? all.slice(0, SUMMARY_PHRASES - 1) : all;
  const phrases = shown.map(({ words, count }) => words.summary(count));
  const rest = all.slice(shown.length).reduce((sum, group) => sum + group.count, 0);
  let text = phrases.join(", ");
  if (rest > 0) text += ` and ${String(rest)} more ${rest === 1 ? "step" : "steps"}`;
  return text.charAt(0).toUpperCase() + text.slice(1);
}

export interface ActivitySummary {
  text: string;
  duration: string | null;
  failures: string | null;
}

export function durationWords(ms: number): string | null {
  return ms >= 1000 ? formatElapsed(ms) : null;
}

export type SegmentStep =
  | { kind: "call"; call: StepCall }
  | { kind: "thought"; item: ThinkingFeedItem };

export function callSteps(calls: readonly StepCall[]): SegmentStep[] {
  return calls.map((call) => ({ kind: "call", call }));
}

export function callsOf(steps: readonly SegmentStep[]): StepCall[] {
  return steps.flatMap((step) => (step.kind === "call" ? [step.call] : []));
}

function thoughtsOf(steps: readonly SegmentStep[]): ThinkingFeedItem[] {
  return steps.flatMap((step) => (step.kind === "thought" ? [step.item] : []));
}

function thoughtMs(item: ThinkingFeedItem): number | null {
  return item.startedAt === undefined || item.endedAt === undefined
    ? null
    : item.endedAt - item.startedAt;
}

export function thoughtLabel(item: ThinkingFeedItem): string {
  if (item.streaming === true) return "Thinking";
  const ms = thoughtMs(item);
  const took = ms === null ? null : durationWords(ms);
  return took === null ? "Thought" : `Thought for ${took}`;
}

function thoughtsPhrase(thoughts: readonly ThinkingFeedItem[], alone: boolean): string {
  const times = thoughts.map(thoughtMs);
  const total = times.every((ms): ms is number => ms !== null)
    ? times.reduce((sum, ms) => sum + ms, 0)
    : null;
  const took = total === null ? null : durationWords(total);
  if (alone) return took === null ? "Thought" : `Thought for ${took}`;
  return took === null ? "thought" : `thought ${took}`;
}

export function segmentDuration(steps: readonly SegmentStep[]): string | null {
  if (steps.length === 0) return null;
  let started = Infinity;
  let ended = -Infinity;
  for (const step of steps) {
    const timed = step.kind === "call" ? step.call : step.item;
    if (timed.startedAt === undefined || timed.endedAt === undefined) return null;
    started = Math.min(started, timed.startedAt);
    ended = Math.max(ended, timed.endedAt);
  }
  return durationWords(ended - started);
}

function segmentText(calls: readonly StepCall[], thoughts: readonly ThinkingFeedItem[]): string {
  if (calls.length === 0) return thoughtsPhrase(thoughts, true);
  if (thoughts.length === 0) return stepsPhrase(calls);
  return `${stepsPhrase(calls)}, ${thoughtsPhrase(thoughts, false)}`;
}

export function summarizeSegment(
  steps: readonly SegmentStep[],
  missed: boolean,
): ActivitySummary | null {
  const calls = callsOf(steps);
  const thoughts = thoughtsOf(steps);
  const failed = calls.filter((call) => call.status === "error").length;
  const failures =
    failed === 0 ? null : `${String(failed)} ${failed === 1 ? "step" : "steps"} failed`;
  if (steps.length > 0) {
    const duration = calls.length === 0 ? null : segmentDuration(steps);
    return { text: segmentText(calls, thoughts), duration, failures };
  }
  if (missed) return { text: "Worked before this page connected", duration: null, failures };
  return null;
}

const ENDING_WORDS: Readonly<Record<TurnEnding, string | null>> = {
  finished: null,
  stopped: "Stopped by you",
  interrupted: "Didn't finish",
};

export function turnEndingLine(observed: ObservedTurn | undefined, worked: boolean): string | null {
  if (observed?.ending == null || observed.endedAt === null) return null;
  const took =
    observed.startedAt === null ? null : durationWords(observed.endedAt - observed.startedAt);
  const ending = ENDING_WORDS[observed.ending];
  if (ending !== null) return took === null ? ending : `${ending} · ${took}`;
  return worked && took !== null ? `Worked for ${took}` : null;
}

export function gapNote(stepsSeen: number): string {
  return stepsSeen === 0
    ? "Earlier steps happened before this page connected"
    : "Steps taken while this page was reconnecting may be missing";
}
