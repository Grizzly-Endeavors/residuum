// A step's arguments, summarized per tool for its details: the command, path
// or query first, then what qualifies it, then any long text it carries.
// A tool without a summary shows every argument by name.

/**
 * One line of a step's arguments: `code` (a command or address), `path`,
 * `query` (quoted), `label` (a name), `meta` (what qualifies the call),
 * `quote` (long text, clamped), or `pairs` (every argument by name).
 */
export type ArgLine =
  | { kind: "code" | "path" | "query" | "label" | "meta" | "quote"; text: string }
  | { kind: "pairs"; pairs: [name: string, value: string][] };

type Args = Record<string, unknown>;

function str(args: Args, key: string): string {
  const value = args[key];
  if (value === null || value === undefined) return "";
  if (typeof value === "string") return value;
  if (typeof value === "number" || typeof value === "boolean") return String(value);
  return JSON.stringify(value);
}

/** Qualifiers joined with " · ", leaving out the ones that aren't set. */
function meta(parts: [label: string, key: string][], args: Args): ArgLine | null {
  const text = parts
    .filter(([, key]) => str(args, key) !== "")
    .map(([label, key]) => `${label}: ${str(args, key)}`)
    .join(" · ");
  return text === "" ? null : { kind: "meta", text };
}

function line(kind: "code" | "path" | "query" | "label" | "quote", text: string): ArgLine | null {
  return text === "" ? null : { kind, text };
}

function readRange(args: Args): ArgLine | null {
  const offset = typeof args.offset === "number" ? args.offset : null;
  const limit = typeof args.limit === "number" ? args.limit : null;
  if (offset !== null && limit !== null)
    return { kind: "meta", text: `lines ${String(offset)}–${String(offset + limit)}` };
  if (offset !== null) return { kind: "meta", text: `from line ${String(offset)}` };
  if (limit !== null) return { kind: "meta", text: `first ${String(limit)} lines` };
  return null;
}

function count(args: Args, key: string, noun: string): ArgLine | null {
  const list = args[key];
  if (!Array.isArray(list)) return null;
  return { kind: "meta", text: `${String(list.length)} ${noun}${list.length === 1 ? "" : "s"}` };
}

const SUMMARIES: Readonly<Record<string, (args: Args) => (ArgLine | null)[]>> = {
  exec: (a) => [line("code", `$ ${str(a, "command")}`), meta([["timeout", "timeout_secs"]], a)],
  read_file: (a) => [line("path", str(a, "path")), readRange(a)],
  write_file: (a) => [line("path", str(a, "path"))],
  edit_file: (a) => [line("path", str(a, "path")), count(a, "edits", "edit")],
  memory_search: (a) => [
    line("query", str(a, "query")),
    meta(
      [
        ["Source", "source"],
        ["Since", "date_from"],
        ["Until", "date_to"],
        ["Limit", "limit"],
      ],
      a,
    ),
  ],
  memory_get: (a) => [line("label", str(a, "episode_id") || str(a, "run_id"))],
  ollama_web_search: (a) => [
    line("query", str(a, "query")),
    meta([["max results", "max_results"]], a),
  ],
  web_fetch: (a) => [line("code", str(a, "url"))],
  send_message: (a) => [
    meta([["To", "endpoint"]], a),
    line("label", str(a, "title")),
    line("quote", str(a, "message")),
  ],
  subagent_spawn: (a) => [
    meta(
      [
        ["Skill", "skill"],
        ["Model", "model"],
      ],
      a,
    ),
    line("quote", str(a, "task")),
  ],
  message_agent: (a) => [meta([["To", "to"]], a), line("quote", str(a, "message"))],
  schedule_action: (a) => [
    line("label", str(a, "name")),
    meta(
      [
        ["At", "run_at"],
        ["Skill", "agent_name"],
        ["Tier", "model_tier"],
      ],
      a,
    ),
    line("quote", str(a, "prompt")),
  ],
  inbox_list: (a) => [a.unread_only === true ? { kind: "meta", text: "unread only" } : null],
  inbox_archive: (a) => [count(a, "ids", "item")],
  inbox_restore: (a) => [count(a, "ids", "item")],
  inbox_read: (a) => [line("label", str(a, "id"))],
  cancel_action: (a) => [line("label", str(a, "id"))],
  stop_agent: (a) => [line("code", str(a, "address"))],
  skill_activate: (a) => [line("label", str(a, "name"))],
  skill_deactivate: (a) => [line("label", str(a, "name"))],
  switch_endpoint: (a) => [line("label", str(a, "endpoint"))],
};

/** `name`'s arguments as the lines its details show; none when it took none. */
export function argumentLines(name: string, args: Args): ArgLine[] {
  const set = Object.entries(args).filter(([, value]) => value !== null && value !== "");
  if (set.length === 0) return [];
  const summary = Object.hasOwn(SUMMARIES, name) ? SUMMARIES[name] : undefined;
  if (summary !== undefined) {
    const lines = summary(args).filter((l): l is ArgLine => l !== null);
    if (lines.length > 0) return lines;
  }
  return [{ kind: "pairs", pairs: set.map(([key]) => [key, str(args, key)]) }];
}
