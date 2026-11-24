import { untrack } from "svelte";
import { hub } from "./hub.svelte";
import { agentWatchWsUrl } from "./paths";
import { WsTransport } from "./transport.svelte";
import { WatchRegistry, type WatchHandler, type WatchOwner } from "./watch-registry";
import { normalizeWatchPrefix } from "./workspace-watch";
import { ws } from "./ws.svelte";

class AgentWatchConnection {
  readonly registry: WatchRegistry;
  private readonly transport: WsTransport;
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

  add(): void {
    untrack(() => {
      this.owners++;
    });
  }

  remove(): boolean {
    return untrack(() => --this.owners > 0);
  }

  get held(): boolean {
    return untrack(() => this.owners > 0);
  }

  close(): void {
    this.stopFollowing();
    this.follow(false);
  }

  private follow(wanted: boolean): void {
    if (wanted) {
      if (this.transport.status === "disconnected") this.transport.reconnectNow();
      return;
    }
    const wasOpen = this.transport.status === "connected";
    this.transport.disconnect();
    if (wasOpen) this.registry.disconnected();
  }
}

const connections: Record<string, AgentWatchConnection | undefined> = Object.create(null) as Record<
  string,
  AgentWatchConnection | undefined
>;

export function watchAgentWorkspace(agent: string, handler: WatchHandler): WatchOwner {
  const bound = ws.watches.register(handler, { agent });
  let connection = connections[agent];
  if (connection === undefined) {
    connection = new AgentWatchConnection(agent);
    connections[agent] = connection;
  }
  connection.add();
  const own = connection.registry.register(handler, { readBeforeWatching: true });
  const held = connection;
  let released = false;

  return {
    get prefixes() {
      return bound.prefixes;
    },
    set: (prefixes) => {
      bound.set(prefixes);
      own.set(prefixes);
    },
    release: () => {
      if (released) return;
      released = true;
      bound.release();
      own.release();
      if (held.remove()) return;
      queueMicrotask(() => {
        if (held.held) return;
        held.close();
        if (connections[agent] === held) connections[agent] = undefined;
      });
    },
  };
}
