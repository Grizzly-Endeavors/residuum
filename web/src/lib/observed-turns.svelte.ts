// What a page saw of each turn while it ran (design §4): when it started and
// ended, how it ended, and where the page may have missed steps. History
// records none of this, so only turns this page watched have a record, and a
// history load that renders a turn again drops its record.

import { SvelteMap } from "svelte/reactivity";

/** How a watched turn ended: on its own, stopped by the user, or cut off (the agent stopped). */
export type TurnEnding = "finished" | "stopped" | "interrupted";

export interface ObservedTurn {
  /** `Date.now()` when the turn started, or null when the page joined it running and can't tell. */
  startedAt: number | null;
  /** When the page saw it end, or null while it runs. */
  endedAt: number | null;
  ending: TurnEnding | null;
  /** The user asked to stop it. */
  stopAsked: boolean;
  /**
   * Where the page may have missed steps, as the number of steps it had seen
   * at that point: 0 when it joined a turn already running, more when it
   * reconnected partway through.
   */
  gaps: readonly number[];
}

/** The page's record of a turn, if it watched it. */
export type ObservedTurnLookup = (turnId: string) => ObservedTurn | undefined;

/** Ids given to a turn the page joined before any frame named it. */
const UNNAMED_PREFIX = "joined-";

export class ObservedTurns {
  private readonly turns = new SvelteMap<string, ObservedTurn>();
  private unnamed = 0;

  readonly get: ObservedTurnLookup = (turnId) => this.turns.get(turnId);

  /** The page saw the turn start. */
  start(turnId: string, at = Date.now()): void {
    this.turns.set(turnId, {
      startedAt: at,
      endedAt: null,
      ending: null,
      stopAsked: false,
      gaps: [],
    });
  }

  /**
   * The page connected while the turn ran, so steps before now may be
   * missing. Without an id, it gets a stand-in until a frame names it.
   * Returns the id it is kept under.
   */
  join(turnId: string | null, startedAt: number | null): string {
    const id = turnId ?? `${UNNAMED_PREFIX}${String(++this.unnamed)}`;
    this.turns.set(id, { startedAt, endedAt: null, ending: null, stopAsked: false, gaps: [0] });
    return id;
  }

  /** Whether `turnId` is a stand-in from `join`. */
  isUnnamed(turnId: string): boolean {
    return turnId.startsWith(UNNAMED_PREFIX);
  }

  /** A frame named the turn kept under a stand-in id. */
  rename(from: string, to: string): void {
    const record = this.turns.get(from);
    if (record === undefined) return;
    this.turns.delete(from);
    this.turns.set(to, record);
  }

  /** The page reconnected partway through the turn, having seen `stepsSeen` steps. */
  gap(turnId: string, stepsSeen: number): void {
    this.update(turnId, (record) => ({ gaps: [...record.gaps, stepsSeen] }));
  }

  askStop(turnId: string): void {
    this.update(turnId, () => ({ stopAsked: true }));
  }

  /** The turn ended. Unless told otherwise, it was stopped if the user asked, and finished if not. */
  end(turnId: string, ending?: TurnEnding, at = Date.now()): TurnEnding {
    const record = this.turns.get(turnId);
    const how = ending ?? (record?.stopAsked === true ? "stopped" : "finished");
    if (record?.endedAt === null) this.turns.set(turnId, { ...record, endedAt: at, ending: how });
    return how;
  }

  /** History renders the turn now, which has no timing: drop the page's record. */
  forget(turnId: string): void {
    this.turns.delete(turnId);
  }

  /** Drop every record but `keep`'s. */
  clear(keep: string | null = null): void {
    for (const id of [...this.turns.keys()]) if (id !== keep) this.turns.delete(id);
  }

  private update(turnId: string, change: (record: ObservedTurn) => Partial<ObservedTurn>): void {
    const record = this.turns.get(turnId);
    if (record !== undefined) this.turns.set(turnId, { ...record, ...change(record) });
  }
}
