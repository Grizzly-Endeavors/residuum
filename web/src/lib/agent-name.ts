// The name a person types for an agent, mirrored from `src/config/agent_name.rs`.
// The folder, the URL, and the A2A path stay a short ASCII slug.

/** Longest typed name, in Unicode scalar values after the name is cleaned up. */
export const MAX_DISPLAY_NAME_CHARS = 32;

/** The default name offered for the first agent. */
export const DEFAULT_AGENT_NAME = "assistant";

/** The name people see. An empty `display_name` means the folder name. */
export function agentLabel(agent: { name: string; display_name: string }): string {
  return agent.display_name === "" ? agent.name : agent.display_name;
}

const RESERVED_AGENT_NAMES: readonly string[] = ["hub", "team", "agents"];

const LATIN_ASCII: Readonly<Record<string, string>> = {
  ß: "ss",
  æ: "ae",
  ø: "o",
  ł: "l",
  đ: "d",
  ð: "d",
  þ: "th",
  œ: "oe",
  ı: "i",
  ŋ: "ng",
};

const MAX_SLUG_LEN = 24;

/** The cleaned name, or `null` when `raw` cannot be one. */
export function canonicalAgentName(raw: string): string | null {
  return agentNameProblem(raw) === null ? cleaned(raw) : null;
}

/** The identity of a cleaned name, so `Atlas` and `atlas` are one agent. */
export function displayNameKey(canonical: string): string {
  return canonical.toLowerCase();
}

/**
 * Check a proposed agent name. Returns a plain-language problem, or null
 * when the name is acceptable.
 */
export function agentNameProblem(name: string): string | null {
  const problem = classify(name);
  if (problem === null) return null;
  if (problem.kind === "empty") return "Give your agent a name.";
  if (problem.kind === "long") return `Use ${MAX_DISPLAY_NAME_CHARS} characters or fewer.`;
  if (problem.kind === "edge") {
    return "The name can't start or end with a hyphen or an apostrophe.";
  }
  if (problem.kind === "chars") {
    return "Use letters, numbers, spaces, hyphens, and apostrophes.";
  }
  return `"${problem.name}" is reserved. Pick a different name.`;
}

/**
 * The backend's refusal for `name`, or `null` when the name is acceptable.
 * The mock answers with these strings.
 */
export function backendAgentNameProblem(name: string): string | null {
  const problem = classify(name);
  if (problem === null) return null;
  if (problem.kind === "empty") return "agent name must not be empty";
  if (problem.kind === "long") {
    return `agent name '${problem.name}' is too long: at most ${MAX_DISPLAY_NAME_CHARS} characters`;
  }
  if (problem.kind === "edge") {
    return `agent name '${problem.name}' can't start or end with a hyphen or an apostrophe`;
  }
  if (problem.kind === "chars") {
    return `agent name '${problem.name}' can use letters, numbers, spaces, hyphens, and apostrophes`;
  }
  return `agent name '${problem.name}' is reserved and cannot be used; reserved names: ${RESERVED_AGENT_NAMES.join(", ")}`;
}

/** Whether `typed` is the same agent as one of `taken`, ignoring case. */
export function nameIsTaken(typed: string, taken: readonly string[]): boolean {
  const canonical = canonicalAgentName(typed);
  if (canonical === null) return false;
  const key = displayNameKey(canonical);
  return taken.some((existing) => displayNameKey(existing) === key);
}

/** Check a name for a new agent: the rules above, and not one of `existing`. */
export function newAgentNameProblem(name: string, existing: readonly string[]): string | null {
  const problem = agentNameProblem(name);
  if (problem !== null) return problem;
  const canonical = cleaned(name);
  if (nameIsTaken(canonical, existing)) {
    return `You already have an agent called ${canonical}.`;
  }
  return null;
}

/** The folder name derived from a cleaned typed name. */
export function slugBase(display: string): string {
  const ascii = foldToAscii(displayNameKey(display));
  return ascii === "" ? hashSlug(displayNameKey(display)) : ascii;
}

/** The first free folder name derived from `base`. */
export function allocateSlug(base: string, taken: (candidate: string) => boolean): string {
  if (slugIsLegal(base) && !taken(base)) return base;
  for (let n = 2; n < 1000; n += 1) {
    const suffix = `-${n}`;
    let stem = scalarPrefix(base, MAX_SLUG_LEN - suffix.length);
    while (stem.endsWith("-")) stem = stem.slice(0, -1);
    if (stem === "") continue;
    const candidate = `${stem}${suffix}`;
    if (slugIsLegal(candidate) && !taken(candidate)) return candidate;
  }
  return `${base}-2`;
}

function cleaned(raw: string): string {
  return raw.trim().split(/\s+/u).join(" ").normalize("NFC");
}

type NameProblem =
  | { kind: "empty" }
  | { kind: "long" | "edge" | "chars" | "reserved"; name: string };

function classify(raw: string): NameProblem | null {
  const name = cleaned(raw);
  if (name === "") return { kind: "empty" };
  if (scalarCount(name) > MAX_DISPLAY_NAME_CHARS) return { kind: "long", name };
  if (name.startsWith("-") || name.startsWith("'") || name.endsWith("-") || name.endsWith("'")) {
    return { kind: "edge", name };
  }
  let allowed = true;
  let hasLetterOrNumber = false;
  for (const ch of name) {
    if (isLetter(ch) || isNumber(ch)) hasLetterOrNumber = true;
    else if (ch !== " " && ch !== "-" && ch !== "'") allowed = false;
  }
  if (!allowed || !hasLetterOrNumber) return { kind: "chars", name };
  if (RESERVED_AGENT_NAMES.includes(displayNameKey(name))) return { kind: "reserved", name };
  return null;
}

/** How many Unicode scalar values `text` holds. */
function scalarCount(text: string): number {
  let count = 0;
  for (const _ch of text) count += 1;
  return count;
}

/** The first `max` Unicode scalar values of `text`. */
function scalarPrefix(text: string, max: number): string {
  let out = "";
  let count = 0;
  for (const ch of text) {
    if (count >= max) break;
    out += ch;
    count += 1;
  }
  return out;
}

function isLetter(ch: string): boolean {
  return /^\p{L}$/u.test(ch);
}

function isNumber(ch: string): boolean {
  return /^\p{N}$/u.test(ch);
}

function foldToAscii(lowercased: string): string {
  let raw = "";
  for (const ch of lowercased.normalize("NFD")) {
    if (isCombiningMark(ch)) continue;
    if (/^[a-z0-9]$/u.test(ch)) raw += ch;
    else if (ch === " " || ch === "-") raw += "-";
    else if (ch === "'") continue;
    else if (LATIN_ASCII[ch] !== undefined) raw += LATIN_ASCII[ch];
  }
  let collapsed = "";
  for (const ch of raw) {
    if (ch === "-" && collapsed.endsWith("-")) continue;
    collapsed += ch;
  }
  let fitted = collapsed.replace(/^-+|-+$/gu, "");
  fitted = scalarPrefix(fitted, MAX_SLUG_LEN);
  while (fitted.endsWith("-")) fitted = fitted.slice(0, -1);
  return fitted;
}

function isCombiningMark(ch: string): boolean {
  const code = ch.codePointAt(0) ?? 0;
  return (
    (code >= 0x0300 && code <= 0x036f) ||
    (code >= 0x1ab0 && code <= 0x1aff) ||
    (code >= 0x1dc0 && code <= 0x1dff) ||
    (code >= 0x20d0 && code <= 0x20ff) ||
    (code >= 0xfe20 && code <= 0xfe2f)
  );
}

function fnv1aLow32(text: string): number {
  let hash = 0xcbf29ce484222325n;
  const prime = 0x100000001b3n;
  const mask = (1n << 64n) - 1n;
  for (const byte of new TextEncoder().encode(text)) {
    hash ^= BigInt(byte);
    hash = (hash * prime) & mask;
  }
  return Number(hash & 0xffffffffn);
}

function hashSlug(key: string): string {
  return `n${fnv1aLow32(key).toString(16).padStart(8, "0")}`;
}

/** Whether `name` is a folder, URL, and A2A segment: `[a-z0-9-]`, 1–24 characters. */
export function isAgentSlug(name: string): boolean {
  return slugIsLegal(name) && !RESERVED_AGENT_NAMES.includes(name);
}

function slugIsLegal(slug: string): boolean {
  return /^[a-z0-9](?:[a-z0-9-]{0,22}[a-z0-9])?$/u.test(slug);
}
