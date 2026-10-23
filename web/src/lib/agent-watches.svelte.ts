// ── Watching any agent's workspace ───────────────────────────────────
//
// The agent socket (`ws`) belongs to the viewed agent, and its watch registry
// (`ws.watches`) follows that agent's files. A file of another agent can be in
// view too: a mail attachment beside the Inbox names the agent that sent it,
// whichever agent was viewed last. `watchAgentWorkspace` follows the files of
// any agent:
//
// - While the agent is the bound one, through `ws.watches`, as everything
//   else on that socket does.
// - Otherwise through a connection of its own to that agent, which carries the
//   watch and nothing else. It is open only while an owner is registered, the
//   agent is not the bound one and the hub lists it running; it closes when
//   the last owner lets go or the agent stops, and opens again when the agent
//   runs. A stopped agent has no change feed to follow, and its files are
//   still read and written through the hub's file routes.
//
// The connection asks to be left out of the agent's count of open pages
// (`?watch_only=true`), so a watched agent's unread count and its pushes carry
// on as if no page were watching it.

import { untrack } from "svelte";
import { hub } from "./hub.svelte";
import { agentWatchWsUrl } from "./paths";
import { WsTransport } from "./transport.svelte";
import { WatchRegistry, type WatchHandler, type WatchOwner } from "./watch-registry";
import { normalizeWatchPrefix } from "./workspace-watch";
import { ws } from "./ws.svelte";

/** One agent's watch-only connection, and the owners that want it. */
class AgentWatchConnection {
  /** Whatever follows this agent's files while the agent is not the bound one registers here. */
  readonly registry: WatchRegistry;
  private readonly transport: WsTransport;
  /** How many owners are registered: the connection is wanted while there are any. */
  private owners = $state(0);
  private readonly stopFollowing: () => void;

  constructor(private readonly agent: string) {
    this.transport = new WsTransport({ url: () => agentWatchWsUrl(agent), keepalive: true });
    this.registry = new WatchRegistry({
      send: (prefixes) => {
        this.transport.send({ type: "watch_workspace", prefixes });
      },
      normalize: normalizeWatchPrefix,
      refusal: (prefix) =>
        `can't watch "${prefix}": watch paths are relative to the workspace, like "team/wiki", and can't contain ".."`,
    });
    // The connection is this agent's alone, so its owners need no agent of their own.
    this.registry.bind(agent);

    this.transport.onConnected = () => {
      this.registry.connected();
    };
    this.transport.onDisconnected = () => {
      this.registry.disconnected();
    };
    this.transport.onMessage = (msg) => {
      this.registry.handleFrame(msg);
    };

    this.stopFollowing = $effect.root(() => {
      $effect(() => {
        const wanted =
          this.owners > 0 && ws.agent !== this.agent && hub.agent(this.agent)?.state === "running";
        untrack(() => {
          this.follow(wanted);
        });
      });
    });
  }

  /** Count one more owner. */
  add(): void {
    untrack(() => {
      this.owners++;
    });
  }

  /** Count one owner fewer, and whether any are left. */
  remove(): boolean {
    return untrack(() => --this.owners > 0);
  }

  /** Close the connection for good. */
  close(): void {
    this.stopFollowing();
    this.follow(false);
  }

  /** Open the connection while it is wanted, and close it while it is not. */
  private follow(wanted: boolean): void {
    if (wanted) {
      if (this.transport.status === "disconnected") this.transport.reconnectNow();
      return;
    }
    const wasOpen = this.transport.status === "connected";
    // Also cancels a reconnect that the agent's own shutdown scheduled.
    this.transport.disconnect();
    if (wasOpen) this.registry.disconnected();
  }
}

/** The open connections by agent name. A plain record without a prototype, since "constructor" is a valid agent name. */
const connections: Record<string, AgentWatchConnection | undefined> = Object.create(null) as Record<
  string,
  AgentWatchConnection | undefined
>;

/**
 * Follow `agent`'s workspace, whether or not it is the bound agent. The owner
 * watches nothing until it calls `set`, and hears its handler's frames while
 * the agent's changes can be followed; release it to stop.
 */
export function watchAgentWorkspace(agent: string, handler: WatchHandler): WatchOwner {
  const bound = ws.watches.register(handler, { agent });
  let connection = connections[agent];
  if (connection === undefined) {
    connection = new AgentWatchConnection(agent);
    connections[agent] = connection;
  }
  connection.add();
  const own = connection.registry.register(handler);
  const held = connection;
  let released = false;

  return {
    get prefixes() {
      return bound.prefixes;
    },
    set: (prefixes) => {
      // Both spell a prefix the same way, so a refused one is refused by the first.
      bound.set(prefixes);
      own.set(prefixes);
    },
    release: () => {
      if (released) return;
      released = true;
      bound.release();
      own.release();
      if (held.remove()) return;
      held.close();
      if (connections[agent] === held) connections[agent] = undefined;
    },
  };
}
