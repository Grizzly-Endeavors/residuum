// A conversation's items grouped by turn (design §4): each turn's output
// shows as one block, with every tool call of the turn at its head and then
// what the agent said, in order. Shared by the main chat and session
// transcripts, which tag their items with turn ids the same way.

import type { FeedItem, ToolCallState } from "../lib/types";

/** One turn's output. */
export interface FeedTurn {
  kind: "turn";
  /** Stable for keyed lists: taken from the turn's first item. */
  key: string;
  /** The turn's correlation id, when its items carry one. */
  turnId: string | null;
  /** Every tool call the turn made, in order. */
  calls: ToolCallState[];
  /**
   * Everything else in the turn, in order: the agent's intermediate texts,
   * its attachments and its final reply, and any message that reached the
   * agent while the turn ran.
   */
  items: FeedItem[];
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
  return item.kind === "assistant" || item.kind === "tool-group" || item.kind === "file-attachment";
}

/** A message that starts a turn when it arrives outside one. */
function isTurnMessage(item: FeedItem): boolean {
  return item.kind === "user" || item.kind === "agent-message";
}

function emptyBlock(turnId: string): FeedTurn {
  return { kind: "turn", key: `turn:${turnId}`, turnId, calls: [], items: [], live: false };
}

/**
 * Group `items` by turn. `liveTurnId` is the turn in flight, if any.
 *
 * Turns are bounded by a change of turn id where items carry one, and
 * otherwise by the next user message or agent message. A message that
 * carries the id of the turn whose output came before it reached the agent
 * mid-turn, and stays inside that turn. Dividers and notes stand between
 * turns, so output after one starts a new block.
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
          calls: [],
          items: [],
          live: false,
        };
        if (id !== undefined && id === liveTurnId) live.block = turn.block;
        entries.push(turn.block);
      }
      if (item.kind === "tool-group") turn.block.calls.push(...item.calls);
      else turn.block.items.push(item);
    } else if (isTurnMessage(item)) {
      if (turn?.block && item.turnId !== undefined && item.turnId === turn.id) {
        turn.block.items.push(item);
      } else {
        if (begun.turn?.id !== item.turnId) placeBegun();
        entries.push({ kind: "single", key: item.id, item });
        turn = { id: item.turnId, block: null };
        if (item.turnId !== undefined && begun.turn === null) {
          begun.turn = { id: item.turnId, at: entries.length };
        }
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
