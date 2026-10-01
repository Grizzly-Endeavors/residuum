// The activity line's words (design §4): each tool call as a step in plain
// language, and a turn's steps as one summary with repeats merged and
// counted. Labels come from one table keyed by tool name. Any other tool
// reads "Used <tool>", and a tool server's tool "Used <server>: <tool>".

import { formatElapsed } from "../lib/format-usage";
import type { IconName } from "../lib/icons";
import { isWorkspacePath } from "../lib/markdown";
import type { ObservedTurn, TurnEnding } from "../lib/observed-turns.svelte";
import type { ToolCallState } from "../lib/types";

/** A tool call as the line reads it. `server` names the tool server a tool came from, when known. */
export interface StepCall extends ToolCallState {
  server?: string | null;
}

export type StepStatus = "running" | "done" | "failed" | "stopped";

/**
 * What a step acted on: a workspace `path` (a link into the panel), a
 * `session` address (opens the session), a `query` (quoted), `code` (a
 * command, an address, an id) or a plain `name`.
 */
export interface StepTarget {
  kind: "path" | "session" | "query" | "code" | "name";
  text: string;
}

export interface ActivityStep {
  id: string;
  icon: IconName;
  /** The words before the target: "Reading" while it runs, "Read" once it has. */
  verb: string;
  target: StepTarget | null;
  status: StepStatus;
  call: StepCall;
}

type Args = Record<string, unknown>;

interface ToolWords {
  icon: IconName;
  /** Before the target while the call runs, and once it has; the whole label when there's no target, unless `bare` says otherwise. */
  running: string;
  done: string;
  /** Joins the verb to the target: "for" in "Searched memory for “x”". */
  by?: string;
  /** The label when there's no target, where the verbs alone don't read whole. */
  bare?: readonly [running: string, done: string];
  target?: (args: Args, result: string | undefined) => StepTarget | null;
  /** What `n` calls of this tool did, for the summary: "read 2 files". */
  summary: (n: number) => string;
}

/** Targets longer than this are cut in the line; the step's details hold them whole. */
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

/** A path argument: a link when it is a workspace path, plain code otherwise. */
function pathTarget(key = "path") {
  return (args: Args): StepTarget | null => {
    const text = argText(args, key);
    if (text === null) return null;
    return { kind: isWorkspacePath(text) ? "path" : "code", text: clip(text) };
  };
}

/** The session `subagent_spawn` started, named in its result ("Session spawned-x-3f9a spawned."). */
function spawnedSession(_args: Args, result: string | undefined): StepTarget | null {
  const address = /Session (\S+) spawned/.exec(result ?? "")?.[1];
  return address === undefined ? null : { kind: "session", text: address };
}

/** A session address to stop, or an agent-to-agent call (`a2a:<name>`), which opens nothing. */
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

/** Every built-in tool, in plain words. */
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
    summary: repeated("checked its inbox"),
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

/** A tool the table doesn't know: by its name, and its server's when it came from one. */
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

/** Each call of a turn as a step, in order. */
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

/** A step's label in plain text, the way the line shows it. */
export function stepText(step: ActivityStep): string {
  if (step.target === null) return step.verb;
  const target = step.target.kind === "query" ? `“${step.target.text}”` : step.target.text;
  return `${step.verb} ${target}`;
}

/** Summaries name this many kinds of step before folding the rest into "and N more steps". */
const SUMMARY_PHRASES = 4;

/** The steps in one phrase, repeats merged and counted: "Searched memory, read 2 files". */
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
  /** What the turn did, or how it ended when it did nothing the page saw. */
  text: string;
  /** How long it ran, for a turn the page watched from its start. */
  duration: string | null;
  /** "1 step failed", for a turn the page watched. */
  failures: string | null;
  /** How it ended when that wasn't on its own: "stopped by you", "didn't finish". */
  ending: string | null;
}

/** How a watched turn ended, when it wasn't on its own. */
const ENDING_WORDS: Readonly<Record<TurnEnding, string | null>> = {
  finished: null,
  stopped: "stopped by you",
  interrupted: "didn't finish",
};

/**
 * A finished turn's line, collapsed. History records neither timing nor
 * failures, so a turn the page didn't watch shows its steps alone. Null when
 * there is nothing to show: no steps, and nothing the page saw happen.
 */
export function summarizeActivity(
  calls: readonly StepCall[],
  observed: ObservedTurn | undefined,
): ActivitySummary | null {
  const ending = observed?.ending ? ENDING_WORDS[observed.ending] : null;
  const failed =
    observed === undefined ? 0 : calls.filter((call) => call.status === "error").length;
  const failures =
    failed === 0 ? null : `${String(failed)} ${failed === 1 ? "step" : "steps"} failed`;
  const duration =
    observed?.startedAt != null && observed.endedAt !== null
      ? formatElapsed(observed.endedAt - observed.startedAt)
      : null;
  if (calls.length > 0) return { text: stepsPhrase(calls), duration, failures, ending };
  if (ending !== null) {
    return {
      text: ending.charAt(0).toUpperCase() + ending.slice(1),
      duration,
      failures,
      ending: null,
    };
  }
  if (observed !== undefined && observed.gaps.length > 0) {
    return { text: "Worked before this page connected", duration, failures, ending: null };
  }
  return null;
}

/** The note shown where the page may have missed steps. */
export function gapNote(stepsSeen: number): string {
  return stepsSeen === 0
    ? "Earlier steps happened before this page connected"
    : "Steps taken while this page was reconnecting may be missing";
}
