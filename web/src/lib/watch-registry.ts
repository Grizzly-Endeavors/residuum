// ── Watch registry ───────────────────────────────────────────────────
//
// A socket keeps one watch set: the path prefixes it wants change frames for.
// Several parts of the app want to watch at once (a file tree, the Schedule,
// a config watcher), and a set that any of them can replace would let
// one wipe out another's. The registry is the only thing that sets it. Each
// part registers as an owner and sets its own prefixes; the registry sends
// the union and hands every change to the owners whose prefixes it concerns.
//
// There is one registry per socket: the bound agent's (`ws.watches`) and the
// hub's team watch (`hub.teamWatches`).

import type { HubServerMessage } from "./hub-types";
import type { ServerMessage, WorkspaceChange, WorkspaceResyncReason } from "./types";
import { changesUnder } from "./workspace-watch";

/** What an owner hears from its socket's change feed. */
export interface WatchHandler {
  /** Changes under the owner's prefixes, by the gateway's matching rule. Never empty. */
  changed: (changes: WorkspaceChange[]) => void;
  /** The feed lost track of changes, so the owner's view may be stale. Every owner hears it. */
  resync?: (reason: WorkspaceResyncReason) => void;
  /** No watcher is running, so nothing will change live. Every owner hears it. */
  unavailable?: (message: string) => void;
  /**
   * The socket connected again after a connection this owner watched through
   * had dropped. Changes made while it was down were never reported, so the
   * owner's view may be stale.
   */
  reconnected?: () => void;
}

/** One part of the app's claim on a registry. It can only change its own prefixes. */
export interface WatchOwner {
  /** The prefixes this owner watches, normalized and sorted. */
  readonly prefixes: readonly string[];
  /**
   * Watch exactly these prefixes, replacing this owner's previous ones and no
   * one else's. `[]` stops this owner watching without releasing it. A prefix
   * the socket would refuse throws a `TypeError` and leaves the owner's
   * prefixes as they were.
   */
  set: (prefixes: readonly string[]) => void;
  /** Stop watching and stop hearing frames. The owner can't be used again. */
  release: () => void;
}

export interface WatchOwnerOptions {
  /**
   * Tie the owner to one agent: its prefixes are watched, and its handler
   * called, only while that agent is bound. Without it the owner follows
   * whichever agent is bound (and, on the hub's registry, which has no agents,
   * is always live).
   */
  agent?: string;
}

export interface WatchRegistryOptions {
  /** Replace the socket's watch set with `prefixes`. */
  send: (prefixes: string[]) => void;
  /** Spell a prefix as the socket's server does, or `null` for one it would refuse. */
  normalize: (prefix: string) => string | null;
  /** Why `normalize` refused `prefix`, in words for the developer who passed it. */
  refusal: (prefix: string) => string;
}

/** What the registry keeps about one owner. */
interface OwnerState {
  readonly handler: WatchHandler;
  readonly agent: string | undefined;
  prefixes: readonly string[];
  released: boolean;
  /** It was live while a connection was open, so a later connection is a reconnect for it. */
  watched: boolean;
}

function sameSet(a: readonly string[], b: readonly string[]): boolean {
  return a.length === b.length && a.every((prefix, i) => prefix === b[i]);
}

/** Merges the watches of every owner onto one socket. */
export class WatchRegistry {
  private readonly owners = new Set<OwnerState>();
  private bound: string | null = null;
  /** The socket is open, so a change to the union can be sent. */
  private open = false;
  /** The set the current connection is known to hold. */
  private sent: readonly string[] = [];

  constructor(private readonly options: WatchRegistryOptions) {}

  /** Become an owner. It watches nothing until it calls `set`. */
  register(handler: WatchHandler, options: WatchOwnerOptions = {}): WatchOwner {
    const owner: OwnerState = {
      handler,
      agent: options.agent,
      prefixes: [],
      released: false,
      watched: false,
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

  /** The prefixes the socket watches now: every live owner's, merged. */
  get prefixes(): readonly string[] {
    return this.union();
  }

  /**
   * The socket is about to belong to `agent` (`null` for none): its old
   * connection is gone and a new one will open. The owners tied to another
   * agent stop applying; the rest are sent on the new connection.
   */
  bind(agent: string | null): void {
    if (agent === this.bound) return;
    this.bound = agent;
    this.open = false;
  }

  /**
   * The socket opened. A new connection watches nothing, so the union goes out
   * again, and the owners that watched through an earlier connection hear that
   * they may have missed changes. A handler that throws doesn't keep the others
   * from hearing it; once they all have, the failure is thrown to the caller.
   */
  connected(): void {
    this.open = true;
    this.sent = this.union();
    if (this.sent.length > 0) this.options.send([...this.sent]);
    const live = this.live();
    const missed = live.filter((owner) => owner.watched);
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

  /** The socket closed. Changes to the union wait for the next `connected`. */
  disconnected(): void {
    this.open = false;
  }

  /**
   * Hand a change-feed frame to the owners: each owner's changes to that
   * owner alone, resync and unavailable frames to every owner. Any other
   * frame is ignored. A handler that throws doesn't keep the others from the
   * frame; once they have all heard it, the failure is thrown to the caller.
   */
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

  /** Call each owner's handler, then throw what any of them threw. */
  private deliver(deliveries: readonly { owner: OwnerState; call: () => void }[]): void {
    const failures: unknown[] = [];
    for (const { owner, call } of deliveries) {
      // An owner that an earlier handler released while hearing this hears no more of it.
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

  /** The owners whose watches apply to the bound agent, as they are now. */
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

  /** Tell the socket the union, if it is open and the union is not what it holds. */
  private sync(): void {
    if (!this.open) return;
    const next = this.union();
    if (sameSet(next, this.sent)) return;
    this.sent = next;
    this.options.send([...next]);
  }
}
