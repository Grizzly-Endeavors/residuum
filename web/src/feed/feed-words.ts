// The words a feed shows about who sent a message.

import type { IconName } from "../lib/icons";

/** Who sent an agent message, as its card names them. */
export interface CardSender {
  /** What kind of sender, in plain words: "Teammate", "Scheduled session". */
  kind: string;
  /** The sender: a teammate's name, or a session's address. */
  sender: string;
  icon: IconName;
  /** The sender is a session of the feed's agent, so the card offers Open session. */
  isSession: boolean;
}

const SESSION_KINDS: Readonly<Record<string, string>> = {
  spawned: "Background session",
  scheduled: "Scheduled session",
  external: "Conversation in another app",
  artifact: "Workbench page session",
};

/**
 * Name the sender of a message one agent sent another. `from` is the sender's
 * address: `main`, a session address, or `agent:<name>[/<session>]` for a
 * teammate. `agent` is the agent whose feed it is in.
 */
export function cardSender(from: string, category: string | null, agent: string): CardSender {
  if (from.startsWith("agent:") || category === "teammate") {
    return {
      kind: "Teammate",
      sender: from.replace(/^agent:/, ""),
      icon: "users",
      isSession: false,
    };
  }
  if (from === "main") {
    return { kind: "Main conversation", sender: agent, icon: "chat", isSession: false };
  }
  const kind = (category === null ? undefined : SESSION_KINDS[category]) ?? "Session";
  return { kind, sender: from, icon: "layers", isSession: true };
}
