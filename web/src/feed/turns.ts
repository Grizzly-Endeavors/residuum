import type { FeedItem, ToolCallState } from "../lib/types";
import { callSteps, type SegmentStep } from "./activity";

export interface ActivitySegment {
  kind: "activity";
  key: string;
  steps: SegmentStep[];
  calls: ToolCallState[];
  callsBefore: number;
}

export interface TurnMessage {
  kind: "message";
  key: string;
  item: FeedItem;
}

export type TurnPart = ActivitySegment | TurnMessage;

export interface FeedTurn {
  kind: "turn";
  key: string;
  turnId: string | null;
  parts: TurnPart[];
  live: boolean;
}

export interface FeedSingle {
  kind: "single";
  key: number;
  item: FeedItem;
}

export type FeedEntry = FeedSingle | FeedTurn;

function isOutput(item: FeedItem): boolean {
  return (
    item.kind === "assistant" ||
    item.kind === "tool-group" ||
    item.kind === "file-attachment" ||
    item.kind === "turn-failure" ||
    item.kind === "thinking"
  );
}

function isTurnMessage(item: FeedItem): boolean {
  return item.kind === "user" || item.kind === "agent-message";
}

function emptyBlock(turnId: string): FeedTurn {
  return { kind: "turn", key: `turn:${turnId}`, turnId, parts: [], live: false };
}

export function turnCallCount(turn: FeedTurn): number {
  let count = 0;
  for (const part of turn.parts) if (part.kind === "activity") count += part.calls.length;
  return count;
}

function stepsOf(item: FeedItem): SegmentStep[] | null {
  if (item.kind === "tool-group") return callSteps(item.calls);
  if (item.kind === "thinking") return [{ kind: "thought", item }];
  return null;
}

function addToTurn(turn: FeedTurn, item: FeedItem): void {
  const steps = stepsOf(item);
  if (steps === null) {
    turn.parts.push({ kind: "message", key: `item-${String(item.id)}`, item });
    return;
  }
  const calls = item.kind === "tool-group" ? item.calls : [];
  const last = turn.parts.at(-1);
  if (last?.kind === "activity") {
    last.steps.push(...steps);
    last.calls.push(...calls);
    return;
  }
  turn.parts.push({
    kind: "activity",
    key: `activity-${String(item.id)}`,
    steps,
    calls: [...calls],
    callsBefore: turnCallCount(turn),
  });
}

export function drawnParts(turn: FeedTurn, gaps: readonly number[]): TurnPart[] {
  const first = turn.parts[0];
  if (!gaps.includes(0) || first?.kind === "activity") return turn.parts;
  return [
    { kind: "activity", key: `${turn.key}:lead`, steps: [], calls: [], callsBefore: 0 },
    ...turn.parts,
  ];
}

export function gapsWithin(
  gaps: readonly number[],
  segment: ActivitySegment,
  place: { first: boolean; last: boolean },
): number[] {
  const end = segment.callsBefore + segment.calls.length;
  const within: number[] = [];
  for (const seen of gaps) {
    if (seen === 0) {
      if (place.first) within.push(0);
    } else if (seen > segment.callsBefore && (seen <= end || place.last)) {
      within.push(Math.min(seen, end) - segment.callsBefore);
    }
  }
  return within;
}

export function groupTurns(
  items: readonly FeedItem[],
  liveTurnId: string | null,
  keepEmpty: (turnId: string) => boolean = () => false,
): FeedEntry[] {
  const entries: FeedEntry[] = [];
  let turn: { id: string | undefined; block: FeedTurn | null } | null = null;
  const keyed = new Set<string>();
  const live: { block: FeedTurn | null } = { block: null };
  const begun: { turn: { id: string; at: number } | null } = { turn: null };

  const placeBegun = (): void => {
    const pending = begun.turn;
    begun.turn = null;
    if (pending === null || keyed.has(pending.id)) return;
    const isLive = pending.id === liveTurnId;
    if (!isLive && !keepEmpty(pending.id)) return;
    const block = emptyBlock(pending.id);
    keyed.add(pending.id);
    entries.splice(pending.at, 0, block);
    if (isLive) live.block = block;
  };

  for (const item of items) {
    if (isOutput(item)) {
      if (turn === null || turn.id !== item.turnId) turn = { id: item.turnId, block: null };
      if (turn.block === null) {
        const id = item.turnId;
        const first = id !== undefined && !keyed.has(id);
        if (first) keyed.add(id);
        turn.block = {
          kind: "turn",
          key: first ? `turn:${id}` : `turn-${String(item.id)}`,
          turnId: id ?? null,
          parts: [],
          live: false,
        };
        if (id !== undefined && id === liveTurnId) live.block = turn.block;
        entries.push(turn.block);
      }
      addToTurn(turn.block, item);
    } else if (isTurnMessage(item)) {
      if (item.midTurn === true && turn?.block) {
        addToTurn(turn.block, item);
      } else if (turn?.block === null && item.turnId !== undefined && turn.id === item.turnId) {
        entries.push({ kind: "single", key: item.id, item });
        if (begun.turn?.id === item.turnId) begun.turn.at = entries.length;
      } else {
        placeBegun();
        entries.push({ kind: "single", key: item.id, item });
        turn = { id: item.turnId, block: null };
        if (item.turnId !== undefined) begun.turn = { id: item.turnId, at: entries.length };
      }
    } else {
      entries.push({ kind: "single", key: item.id, item });
      if (turn !== null) turn = { id: turn.id, block: null };
    }
  }
  placeBegun();
  if (liveTurnId !== null && live.block === null && !keyed.has(liveTurnId)) {
    live.block = emptyBlock(liveTurnId);
    entries.push(live.block);
  }
  if (live.block !== null) live.block.live = true;
  return entries;
}
