// ── Agent message headers ────────────────────────────────────────────
//
// Messages between agents reach a transcript as user-role text with a header
// the backend writes (`AgentMessageEvent::format_for_agent` in
// `src/bus/events.rs`), plus a structured `agent_sender` field. The field is
// what the UI trusts: anyone can type the header. The header is only honoured
// for history written before the field existed, and only where a person
// couldn't have typed it. Keep these patterns in step with that function.

import type { RecentMessage } from "./types";

/** A message from another agent, as recognized from its header. */
export interface ParsedAgentMessage {
  /** Sender's address (`main` or a session address). */
  from: string;
  /** Sender's category label (`main`, `scheduled`, `external`, `spawned`). */
  category: string;
  /** The message body without the header. */
  body: string;
}

const AGENT_MESSAGE_HEADER =
  /^\[Agent Message from ([^\s()[\]]+) \((main|scheduled|external|spawned)\)\]\n/;

const OWNER_MESSAGE_HEADER = /^\[Message from the owner via the web UI[^\]\n]*\]\n/;

/**
 * Recognize a message one agent sent another in chat history.
 *
 * A message carrying `agent_sender` is one. Without it (history from before
 * the field existed), the header is trusted only on messages a person
 * couldn't have typed: in `main` mode, background-visibility messages (the
 * owner's own messages are user-visibility); in `session` mode, messages with
 * no identified `sender` (people in an external conversation have one).
 */
export function historyAgentMessage(
  msg: RecentMessage,
  mode: "main" | "session",
): ParsedAgentMessage | null {
  if (msg.role !== "user") return null;
  if (mode === "main" && msg.visibility !== "background") return null;
  const parsed = parseAgentMessage(msg.content);
  if (msg.agent_sender) {
    return {
      from: msg.agent_sender.address,
      category: msg.agent_sender.category,
      body: parsed ? parsed.body : msg.content,
    };
  }
  if (mode === "session" && msg.sender) return null;
  return parsed;
}

/**
 * Recognize a message one agent sent another (a session result relayed to
 * its spawner, or a `message_agent` call). Returns `null` for anything else —
 * including unrelated background input such as pulse prompts, which must not
 * be shown as session results.
 */
export function parseAgentMessage(content: string): ParsedAgentMessage | null {
  const match = AGENT_MESSAGE_HEADER.exec(content);
  if (!match) return null;
  const [header, from, category] = match;
  if (from === undefined || category === undefined) return null;
  return { from, category, body: content.slice(header.length) };
}

/**
 * Recognize a message the owner sent a session from the web UI, returning its
 * body without the header, or `null` if it isn't one.
 */
export function parseOwnerMessage(content: string): string | null {
  const match = OWNER_MESSAGE_HEADER.exec(content);
  return match ? content.slice(match[0].length) : null;
}
