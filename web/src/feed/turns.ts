// A conversation's items grouped by turn: each turn's output shows as one
// block, in the order it happened. A run of tool calls and reasoning is one
// activity segment between the agent's messages, so what the agent said
// stays beside the work it said it about. Shared by the main chat and session transcripts,
// which tag their items with turn ids the same way.

import type { FeedItem, ToolCallState } from "../lib/types";
import { callSteps, type SegmentStep } from "./activity";

/**
 * A run of tool calls and reasoning with no message of the agent's between
 * them, shown as one activity line at its place in the turn.
 */
export interface ActivitySegment {
  kind: "activity";
  /** Stable for keyed lists: taken from the segment's first tool group. */
  key: string;
  /** What the segment holds, in the order it happened. */
  steps: SegmentStep[];
  /** The tool calls among `steps`. */
  calls: ToolCallState[];
  /** How many of the turn's tool calls came before this segment's. */
  callsBefore: number;
}

/** A message or attachment of the turn, shown as it is. */
export interface TurnMessage {
  kind: "message";
  key: string;
  item: FeedItem;
}

export type TurnPart = ActivitySegment | TurnMessage;

/** One turn's output. */
export interface FeedTurn {
  kind: "turn";
  /** Stable for keyed lists: taken from the turn's first item. */
  key: string;
  /** The turn's correlation id, when its items carry one. */
  turnId: string | null;
  /**
   * What the turn produced, in the order it happened: runs of tool calls
   * between the agent's intermediate texts, its attachments and its final
   * reply, and any message that reached the agent while the turn ran.
   */
  parts: TurnPart[];
  /** The turn is still running. */
  live: boolean;
}

/** An item shown on its own: a message that starts a turn, a divider, a note. */
export interface FeedSingle {
  kind: "single";
  key: number;
  item: FeedItem;
}

export type FeedEntry = FeedSingle | FeedTurn;

/** What the agent produces in a turn. */
function isOutput(item: FeedItem): boolean {
  return (
    item.kind === "assistant" ||
    item.kind === "tool-group" ||
    item.kind === "file-attachment" ||
    item.kind === "turn-failure" ||
    item.kind === "thinking"
  );
}

/** A message that starts a turn when it arrives outside one. */
function isTurnMessage(item: FeedItem): boolean {
  return item.kind === "user" || item.kind === "agent-message";
}

function emptyBlock(turnId: string): FeedTurn {
  return { kind: "turn", key: `turn:${turnId}`, turnId, parts: [], live: false };
}

/** How many tool calls `turn` has made so far. */
export function turnCallCount(turn: FeedTurn): number {
  let count = 0;
  for (const part of turn.parts) if (part.kind === "activity") count += part.calls.length;
  return count;
}

/** The steps `item` adds to an activity segment, or none when it isn't one. */
function stepsOf(item: FeedItem): SegmentStep[] | null {
  if (item.kind === "tool-group") return callSteps(item.calls);
  if (item.kind === "thinking") return [{ kind: "thought", item }];
  return null;
}

/** Add `item` to the end of `turn`, joining a trailing run of tool calls and reasoning. */
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

/**
 * The parts of `turn` as they are drawn, given where the page may have
 * missed steps (`gaps`, as the number of steps it had seen). A turn the page
 * joined while it ran, whose first thing is not a step, gets an empty
 * segment at its head to hold the note that earlier steps aren't shown.
 */
export function drawnParts(turn: FeedTurn, gaps: readonly number[]): TurnPart[] {
  const first = turn.parts[0];
  if (!gaps.includes(0) || first?.kind === "activity") return turn.parts;
  return [
    { kind: "activity", key: `${turn.key}:lead`, steps: [], calls: [], callsBefore: 0 },
    ...turn.parts,
  ];
}

/**
 * Where the notes for `gaps` go in `segment`: for each, the number of the
 * segment's steps that come before it, so `segment.calls.length` means at
 * its end. A gap before the first step belongs to the turn's first part; the
 * rest follow the step they came after.
 */
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

/**
 * Group `items` by turn. `liveTurnId` is the turn in flight, if any.
 *
 * Turns are bounded by a change of turn id where items carry one, and
 * otherwise by the next user message or agent message. A message flagged
 * `midTurn` reached the agent while a turn ran, and stays inside that turn's
 * block. Any other user or agent message starts a turn, whatever its id:
 * ids repeat in older history, where every page load counted from `web-1`
 * again, so an id alone can't say that two messages belong to one turn.
 * Dividers and notes stand between turns, so output after one starts a new
 * block.
 *
 * A turn with no output still gets a block, just after the message that
 * began it, while it runs and when `keepEmpty` says it has something to
 * show (a turn the user stopped before it did anything).
 */
export function groupTurns(
  items: readonly FeedItem[],
  liveTurnId: string | null,
  keepEmpty: (turnId: string) => boolean = () => false,
): FeedEntry[] {
  const entries: FeedEntry[] = [];
  /** The turn the walk is in, and its output block once it has one. */
  let turn: { id: string | undefined; block: FeedTurn | null } | null = null;
  /** Turns that have a block, so a turn's first block keeps one key from start to end. */
  const keyed = new Set<string>();
  const live: { block: FeedTurn | null } = { block: null };
  /** A turn a message began, and where its block goes if it never has output. */
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
        // Another message before the turn's first output: still the same turn.
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
  // A turn joined partway may have no message that began it.
  if (liveTurnId !== null && live.block === null && !keyed.has(liveTurnId)) {
    live.block = emptyBlock(liveTurnId);
    entries.push(live.block);
  }
  // Only the turn's latest block is live.
  if (live.block !== null) live.block.live = true;
  return entries;
}
