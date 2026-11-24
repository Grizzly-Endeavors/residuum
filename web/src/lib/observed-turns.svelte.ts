import { SvelteMap } from "svelte/reactivity";

export type TurnEnding = "finished" | "stopped" | "interrupted";

export interface ObservedTurn {
  startedAt: number | null;
  endedAt: number | null;
  ending: TurnEnding | null;
  stopAsked: boolean;
  retrying: boolean;
  gaps: readonly number[];
}

export type ObservedTurnLookup = (turnId: string) => ObservedTurn | undefined;

const UNNAMED_PREFIX = "joined-";

export class ObservedTurns {
  private readonly turns = new SvelteMap<string, ObservedTurn>();
  private unnamed = 0;

  readonly get: ObservedTurnLookup = (turnId) => this.turns.get(turnId);

  start(turnId: string, at = Date.now()): void {
    this.turns.set(turnId, {
      startedAt: at,
      endedAt: null,
      ending: null,
      stopAsked: false,
      retrying: false,
      gaps: [],
    });
  }

  join(turnId: string | null, startedAt: number | null): string {
    const id = turnId ?? `${UNNAMED_PREFIX}${String(++this.unnamed)}`;
    this.turns.set(id, {
      startedAt,
      endedAt: null,
      ending: null,
      stopAsked: false,
      retrying: false,
      gaps: [0],
    });
    return id;
  }

  isUnnamed(turnId: string): boolean {
    return turnId.startsWith(UNNAMED_PREFIX);
  }

  rename(from: string, to: string): void {
    const record = this.turns.get(from);
    if (record === undefined) return;
    this.turns.delete(from);
    this.turns.set(to, record);
  }

  gap(turnId: string, stepsSeen: number): void {
    this.update(turnId, (record) => ({ gaps: [...record.gaps, stepsSeen] }));
  }

  retry(turnId: string, on: boolean): void {
    if (this.turns.get(turnId)?.retrying !== on) this.update(turnId, () => ({ retrying: on }));
  }

  askStop(turnId: string): void {
    this.update(turnId, () => ({ stopAsked: true }));
  }

  end(turnId: string, ending?: TurnEnding, at = Date.now()): TurnEnding {
    const record = this.turns.get(turnId);
    const how = ending ?? (record?.stopAsked === true ? "stopped" : "finished");
    if (record?.endedAt === null) this.turns.set(turnId, { ...record, endedAt: at, ending: how });
    return how;
  }

  forget(turnId: string): void {
    this.turns.delete(turnId);
  }

  clear(keep: string | null = null): void {
    for (const id of [...this.turns.keys()]) if (id !== keep) this.turns.delete(id);
  }

  private update(turnId: string, change: (record: ObservedTurn) => Partial<ObservedTurn>): void {
    const record = this.turns.get(turnId);
    if (record !== undefined) this.turns.set(turnId, { ...record, ...change(record) });
  }
}
