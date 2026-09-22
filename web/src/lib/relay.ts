// ── Agent message headers ────────────────────────────────────────────
//
// Messages between agents reach a transcript as plain user-role text with a
// header the backend writes (`AgentMessageEvent::format_for_agent` in
// `src/bus/events.rs`). History carries no structured sender field, so the
// header is how the UI attributes these messages. Keep these patterns in step
// with that function.

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
