import type { IconName } from "../lib/icons";

export interface CardSender {
  kind: string;
  sender: string;
  icon: IconName;
  isSession: boolean;
}

const SESSION_KINDS: Readonly<Record<string, string>> = {
  spawned: "Background session",
  scheduled: "Scheduled session",
  external: "Conversation in another app",
  artifact: "Workbench page session",
};

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

const ENDPOINT_NAMES: Readonly<Record<string, string>> = {
  telegram: "Telegram",
  discord: "Discord",
  teams: "Teams",
};

/** A chat interface as people name it: `telegram` is "Telegram"; any other is its id, capitalized. */
export function endpointName(endpoint: string): string {
  return ENDPOINT_NAMES[endpoint] ?? `${endpoint.charAt(0).toUpperCase()}${endpoint.slice(1)}`;
}
