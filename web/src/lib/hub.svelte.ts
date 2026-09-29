// ── Hub state (Svelte 5 runes) ───────────────────────────────────────
//
// What the hub knows about every agent, fed by `/api/hub/ws`: the agent list
// with each agent's state, per-agent busy and unread, hub notices, and the
// team change feed. The hub connection is independent of the agent
// connection (`ws.svelte.ts`) and stays up across agent switches.

import { WsTransport } from "./transport.svelte";
import { hubWsUrl } from "./paths";
import { notifications } from "./notifications.svelte";
import { userErrorMessage } from "./errors";
import {
  createAgent as apiCreateAgent,
  deleteAgent as apiDeleteAgent,
  fetchAgents,
  restartAgent as apiRestartAgent,
  setAgentAutostart as apiSetAgentAutostart,
  setAgentVisibility as apiSetAgentVisibility,
  startAgent as apiStartAgent,
  stopAgent as apiStopAgent,
} from "./api";
import type {
  A2aVisibility,
  AgentActivity,
  AgentSummary,
  CreateAgentRequest,
  DeleteOutcome,
  HubActor,
  HubClientMessage,
  HubNoticeLevel,
  HubServerMessage,
} from "./hub-types";
import type { WorkspaceChange } from "./types";
import { normalizeTeamWatchPrefix } from "./workspace-watch";

/** How many notices the store keeps for recall. */
const MAX_NOTICES = 50;

/** A hub notice, kept after its toast is gone. */
export interface HubNotice {
  id: number;
  level: HubNoticeLevel;
  message: string;
  /** The agent it concerns, if any. */
  agent?: string;
  at: Date;
}

/** Who acted, as a sentence subject: "You" for the user, else the teammate's name. */
function actorLabel(by: HubActor): string {
  return by === "user" ? "You" : by.slice("agent:".length);
}

const IDLE: AgentActivity = { busy: false, unread: 0 };

function byName(a: AgentSummary, b: AgentSummary): number {
  if (a.name === b.name) return 0;
  return a.name < b.name ? -1 : 1;
}

export class HubStore {
  /** Every agent, sorted by name. */
  agents = $state<AgentSummary[]>([]);
  /** The agent list has arrived, from the hub socket or a fetch. */
  loaded = $state(false);
  /** Busy and unread per agent name. Agents with no report are idle. */
  activity = $state<Record<string, AgentActivity>>({});
  /** Notices received this session, newest first. */
  notices = $state<HubNotice[]>([]);

  readonly transport: WsTransport<HubServerMessage, HubClientMessage>;

  private noticeCounter = 0;
  private watchedPrefixes: string[] = [];
  // eslint-disable-next-line svelte/prefer-svelte-reactivity -- bookkeeping only, never rendered
  private teamListeners = new Set<(changes: WorkspaceChange[]) => void>();
  // eslint-disable-next-line svelte/prefer-svelte-reactivity -- bookkeeping only, never rendered
  private frameListeners = new Set<(msg: HubServerMessage) => void>();
  /** The socket has delivered a snapshot, which is newer than any fetch. */
  private snapshotSeen = false;

  constructor() {
    this.transport = new WsTransport<HubServerMessage, HubClientMessage>({
      url: hubWsUrl,
      keepalive: false,
    });
    this.transport.onMessage = (msg) => {
      this.handleFrame(msg);
    };
    this.transport.onConnected = () => {
      // A new connection watches nothing until told.
      if (this.watchedPrefixes.length > 0) {
        this.transport.send({ type: "watch_team", prefixes: this.watchedPrefixes });
      }
    };
  }

  /** Connect to the hub socket. Its first frame is the agent list. */
  connect(): void {
    this.transport.connect();
  }

  /** Close the hub socket. */
  disconnect(): void {
    this.transport.disconnect();
  }

  /**
   * Fetch the agent list over HTTP. The socket's snapshot is newer, so a
   * fetch that lands after one is ignored unless `force` is set, for when
   * the caller knows the list changed outside the socket's view. Throws
   * `ApiError` on failure.
   */
  async refresh(force = false): Promise<void> {
    const agents = await fetchAgents();
    if (this.snapshotSeen && !force) return;
    this.setAgents(agents);
  }

  // ── Reading ────────────────────────────────────────────────────────

  agent(name: string): AgentSummary | undefined {
    return this.agents.find((a) => a.name === name);
  }

  activityOf(name: string): AgentActivity {
    return this.activity[name] ?? IDLE;
  }

  // ── Team change feed ───────────────────────────────────────────────

  /**
   * Watch these team path prefixes on the hub connection, replacing any
   * before. A prefix is `team` or a path under `team/` (`team/wiki`), the
   * spelling the hub's change feed uses; anything else throws a `TypeError`,
   * since the hub would refuse it. `[]` stops watching. Re-sent after every
   * reconnect.
   */
  watchTeam(prefixes: readonly string[]): void {
    const normalized = prefixes.map((prefix) => {
      const team = normalizeTeamWatchPrefix(prefix);
      if (team === null) {
        throw new TypeError(
          `can't watch "${prefix}": team watch paths are "team" or start with "team/", like "team/wiki"`,
        );
      }
      return team;
    });
    // eslint-disable-next-line svelte/prefer-svelte-reactivity -- non-reactive scratch
    const next = [...new Set(normalized)].sort();
    if (
      next.length === this.watchedPrefixes.length &&
      next.every((p, i) => p === this.watchedPrefixes[i])
    ) {
      return;
    }
    this.watchedPrefixes = next;
    this.transport.send({ type: "watch_team", prefixes: next });
  }

  /** Observe team changes under the watched prefixes. Returns a function that stops observing. */
  onTeamChange(listener: (changes: WorkspaceChange[]) => void): () => void {
    this.teamListeners.add(listener);
    return () => this.teamListeners.delete(listener);
  }

  /** Observe every hub frame, after the store has handled it. Returns a function that stops observing. */
  onFrame(listener: (msg: HubServerMessage) => void): () => void {
    this.frameListeners.add(listener);
    return () => this.frameListeners.delete(listener);
  }

  // ── Lifecycle actions ──────────────────────────────────────────────
  //
  // Each calls the hub API, folds the returned summary into the list so the
  // UI reflects it before the socket's own frame, and surfaces a failure as a
  // notification. They resolve to whether the action succeeded.

  async createAgent(request: CreateAgentRequest): Promise<AgentSummary | null> {
    try {
      const agent = await apiCreateAgent(request);
      this.upsert(agent);
      return agent;
    } catch (err) {
      this.reportFailure(err, `Couldn't create ${request.name}.`);
      return null;
    }
  }

  async deleteAgent(name: string): Promise<DeleteOutcome | null> {
    try {
      const result = await apiDeleteAgent(name);
      this.removeAgent(name);
      return result;
    } catch (err) {
      this.reportFailure(err, `Couldn't delete ${name}.`);
      return null;
    }
  }

  async startAgent(name: string): Promise<boolean> {
    return this.lifecycle(name, apiStartAgent, "start");
  }

  async stopAgent(name: string): Promise<boolean> {
    return this.lifecycle(name, apiStopAgent, "stop");
  }

  async restartAgent(name: string): Promise<boolean> {
    return this.lifecycle(name, apiRestartAgent, "restart");
  }

  async setAutostart(name: string, autostart: boolean): Promise<boolean> {
    return this.lifecycle(
      name,
      (n) => apiSetAgentAutostart(n, autostart),
      "change the autostart setting of",
    );
  }

  async setVisibility(name: string, visibility: A2aVisibility): Promise<boolean> {
    return this.lifecycle(
      name,
      (n) => apiSetAgentVisibility(n, visibility),
      "change the A2A visibility of",
    );
  }

  private async lifecycle(
    name: string,
    call: (name: string) => Promise<AgentSummary>,
    verb: string,
  ): Promise<boolean> {
    try {
      this.upsert(await call(name));
      return true;
    } catch (err) {
      this.reportFailure(err, `Couldn't ${verb} ${name}.`);
      return false;
    }
  }

  private reportFailure(err: unknown, action: string): void {
    notifications.surface(
      "error",
      userErrorMessage(err, { action, notFound: "That agent no longer exists." }),
    );
  }

  // ── Frames ─────────────────────────────────────────────────────────

  /** Fold one hub frame into the store. */
  handleFrame(msg: HubServerMessage): void {
    switch (msg.type) {
      case "agents_snapshot":
        this.snapshotSeen = true;
        this.setAgents(msg.agents);
        break;
      case "agent_state":
        this.noteFailure(msg.agent);
        this.upsert(msg.agent);
        break;
      case "agent_created":
        this.upsert(msg.agent);
        this.addNotice("info", `${actorLabel(msg.by)} created ${msg.agent.name}.`);
        break;
      case "agent_deleted":
        this.removeAgent(msg.name);
        this.addNotice("info", `${actorLabel(msg.by)} deleted ${msg.name}.`);
        break;
      case "agent_activity":
        this.activity = { ...this.activity, [msg.name]: { busy: msg.busy, unread: msg.unread } };
        break;
      case "notice":
        this.addNotice(msg.level, msg.message, msg.agent);
        break;
      case "workspace_changed":
        for (const listener of this.teamListeners) listener(msg.changes);
        break;
      case "workspace_watch_unavailable":
        this.addNotice("warn", msg.message);
        break;
      case "workspace_resync":
        break;
    }
    for (const listener of this.frameListeners) listener(msg);
  }

  /**
   * A failure reaches the user only through its `agent_state` frame (the hub
   * sends no separate notice), so the transition into `failed` raises one.
   */
  private noteFailure(next: AgentSummary): void {
    if (next.state !== "failed") return;
    if (this.agent(next.name)?.state === "failed") return;
    const reason = next.last_error?.message;
    this.addNotice(
      "error",
      reason ? `${next.name} failed: ${reason}` : `${next.name} failed.`,
      next.name,
    );
  }

  private setAgents(agents: AgentSummary[]): void {
    this.agents = [...agents].sort(byName);
    // eslint-disable-next-line svelte/prefer-svelte-reactivity -- non-reactive scratch
    const known = new Set(agents.map((a) => a.name));
    this.activity = Object.fromEntries(
      Object.entries(this.activity).filter(([name]) => known.has(name)),
    );
    this.loaded = true;
  }

  private upsert(agent: AgentSummary): void {
    this.agents = [...this.agents.filter((a) => a.name !== agent.name), agent].sort(byName);
    this.loaded = true;
  }

  private removeAgent(name: string): void {
    this.agents = this.agents.filter((a) => a.name !== name);
    this.activity = Object.fromEntries(Object.entries(this.activity).filter(([n]) => n !== name));
  }

  private addNotice(level: HubNoticeLevel, message: string, agent?: string): void {
    // eslint-disable-next-line svelte/prefer-svelte-reactivity -- a timestamp, never mutated
    const at = new Date();
    const notice: HubNotice = { id: ++this.noticeCounter, level, message, agent, at };
    this.notices = [notice, ...this.notices].slice(0, MAX_NOTICES);
    notifications.surface(
      level === "error" ? "error" : "notice",
      agent ? `${agent}: ${message}` : message,
    );
  }
}

export const hub = new HubStore();
