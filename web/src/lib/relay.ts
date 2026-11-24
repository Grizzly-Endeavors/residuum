import type { AgentSender, RecentMessage } from "./types";

export interface ParsedAgentMessage {
  from: string;
  category: string;
  body: string;
}

const AGENT_MESSAGE_HEADER =
  /^\[Agent Message from ([^\s()[\]]+) \((main|scheduled|external|spawned|artifact)\)\]\n/;

const TEAMMATE_MESSAGE_HEADER =
  /^\[Message from teammate (agent:[a-z0-9-]+(?:\/[^\s,[\]]+)?), not the user\.[^\]\n]*\]\n/;

const OWNER_MESSAGE_HEADER = /^\[Message from the owner via the web UI[^\]\n]*\]\n/;

const ARTIFACT_MESSAGE_HEADER = /^\[Message from the workbench artifact "([a-z0-9-]+)"[^\]\n]*\]\n/;

export function historyAgentMessage(
  msg: RecentMessage,
  mode: "main" | "session",
): ParsedAgentMessage | null {
  if (msg.role !== "user") return null;
  if (msg.agent_sender) return namedAgentMessage(msg.agent_sender, msg.content);
  const typedByPerson = mode === "main" ? msg.visibility !== "background" : msg.sender != null;
  return typedByPerson ? null : parseAgentMessage(msg.content);
}

export function namedAgentMessage(sender: AgentSender, content: string): ParsedAgentMessage {
  const parsed = parseAgentMessage(content);
  return { from: sender.address, category: sender.category, body: parsed ? parsed.body : content };
}

export function parseAgentMessage(content: string): ParsedAgentMessage | null {
  const teammate = TEAMMATE_MESSAGE_HEADER.exec(content);
  if (teammate) {
    const [header, from] = teammate;
    if (from === undefined) return null;
    return { from, category: "teammate", body: content.slice(header.length) };
  }
  const match = AGENT_MESSAGE_HEADER.exec(content);
  if (!match) return null;
  const [header, from, category] = match;
  if (from === undefined || category === undefined) return null;
  return { from, category, body: content.slice(header.length) };
}

export function parseOwnerMessage(content: string): string | null {
  const match = OWNER_MESSAGE_HEADER.exec(content);
  return match ? content.slice(match[0].length) : null;
}

export interface ParsedArtifactMessage {
  artifact: string;
  body: string;
}

export function parseArtifactMessage(content: string): ParsedArtifactMessage | null {
  const match = ARTIFACT_MESSAGE_HEADER.exec(content);
  if (!match) return null;
  const [header, artifact] = match;
  if (artifact === undefined) return null;
  return { artifact, body: content.slice(header.length) };
}
