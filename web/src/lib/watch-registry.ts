import type { HubServerMessage } from "./hub-types";
import type { ServerMessage, WorkspaceChange, WorkspaceResyncReason } from "./types";
import { changesUnder } from "./workspace-watch";

export interface WatchHandler {
  changed: (changes: WorkspaceChange[]) => void;
  resync?: (reason: WorkspaceResyncReason) => void;
  unavailable?: (message: string) => void;
  reconnected?: () => void;
}

export interface WatchOwner {
  readonly prefixes: readonly string[];
  /**
   * Watch exactly these prefixes, replacing this owner's previous ones and no
   * one else's. `[]` stops this owner watching without releasing it. A prefix
   * the socket would refuse throws a `TypeError` and leaves the owner's
   * prefixes as they were.
   */
  set: (prefixes: readonly string[]) => void;
  release: () => void;
}

export interface WatchOwnerOptions {
  agent?: string;
  readBeforeWatching?: boolean;
}

export interface WatchRegistryOptions {
  send: (prefixes: string[]) => void;
  /** Spell a prefix as the socket's server does, or `null` for one it would refuse. */
  normalize: (prefix: string) => string | null;
  refusal: (prefix: string) => string;
}

interface OwnerState {
  readonly handler: WatchHandler;
  readonly agent: string | undefined;
  prefixes: readonly string[];
  released: boolean;
  watched: boolean;
  readonly readBeforeWatching: boolean;
}

function sameSet(a: readonly string[], b: readonly string[]): boolean {
  return a.length === b.length && a.every((prefix, i) => prefix === b[i]);
}

export class WatchRegistry {
  private readonly owners = new Set<OwnerState>();
  private bound: string | null = null;
  private open = false;
  private sent: readonly string[] = [];

  constructor(private readonly options: WatchRegistryOptions) {}

  register(handler: WatchHandler, options: WatchOwnerOptions = {}): WatchOwner {
    const owner: OwnerState = {
      handler,
      agent: options.agent,
      prefixes: [],
      released: false,
      watched: false,
      readBeforeWatching: options.readBeforeWatching ?? false,
    };
    this.owners.add(owner);
    if (this.open) owner.watched = this.live().includes(owner);
    return {
      get prefixes() {
        return owner.prefixes;
      },
      set: (prefixes) => {
        this.setPrefixes(owner, prefixes);
      },
      release: () => {
        this.release(owner);
      },
    };
  }

  get prefixes(): readonly string[] {
    return this.union();
  }

  bind(agent: string | null): void {
    if (agent === this.bound) return;
    this.bound = agent;
    this.open = false;
  }

  connected(): void {
    this.open = true;
    this.sent = this.union();
    if (this.sent.length > 0) this.options.send([...this.sent]);
    const live = this.live();
    const missed = live.filter((owner) => owner.watched || owner.readBeforeWatching);
    for (const owner of live) owner.watched = true;
    this.deliver(
      missed.map((owner) => ({
        owner,
        call: () => {
          owner.handler.reconnected?.();
        },
      })),
    );
  }

  disconnected(): void {
    this.open = false;
  }

  handleFrame(frame: ServerMessage | HubServerMessage): void {
    const deliveries: { owner: OwnerState; call: () => void }[] = [];
    for (const owner of this.live()) {
      const { handler } = owner;
      if (frame.type === "workspace_changed") {
        const matching = changesUnder(frame.changes, owner.prefixes);
        if (matching.length === 0) continue;
        deliveries.push({
          owner,
          call: () => {
            handler.changed(matching);
          },
        });
      } else if (frame.type === "workspace_resync") {
        deliveries.push({
          owner,
          call: () => {
            handler.resync?.(frame.reason);
          },
        });
      } else if (frame.type === "workspace_watch_unavailable") {
        deliveries.push({
          owner,
          call: () => {
            handler.unavailable?.(frame.message);
          },
        });
      }
    }
    this.deliver(deliveries);
  }

  private deliver(deliveries: readonly { owner: OwnerState; call: () => void }[]): void {
    const failures: unknown[] = [];
    for (const { owner, call } of deliveries) {
      if (owner.released) continue;
      try {
        call();
      } catch (err) {
        failures.push(err);
      }
    }
    if (failures.length === 1) throw failures[0];
    if (failures.length > 1) throw new AggregateError(failures, "several watch handlers failed");
  }

  private setPrefixes(owner: OwnerState, prefixes: readonly string[]): void {
    if (owner.released) return;
    const normalized = new Set<string>();
    for (const prefix of prefixes) {
      const spelled = this.options.normalize(prefix);
      if (spelled === null) throw new TypeError(this.options.refusal(prefix));
      normalized.add(spelled);
    }
    const next = [...normalized].sort();
    if (sameSet(next, owner.prefixes)) return;
    owner.prefixes = next;
    this.sync();
  }

  private release(owner: OwnerState): void {
    if (owner.released) return;
    owner.released = true;
    this.owners.delete(owner);
    this.sync();
  }

  private live(): OwnerState[] {
    return [...this.owners].filter(
      (owner) => !owner.released && (owner.agent === undefined || owner.agent === this.bound),
    );
  }

  private union(): string[] {
    const merged = new Set<string>();
    for (const owner of this.live()) for (const prefix of owner.prefixes) merged.add(prefix);
    return [...merged].sort();
  }

  private sync(): void {
    if (!this.open) return;
    const next = this.union();
    if (sameSet(next, this.sent)) return;
    this.sent = next;
    this.options.send([...next]);
  }
}
