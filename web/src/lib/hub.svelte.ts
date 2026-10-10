// ── Hub state (Svelte 5 runes) ───────────────────────────────────────
//
// What the hub knows about every agent, fed by `/api/hub/ws`: the agent list
// with each agent's state, per-agent busy and unread, hub notices, and the
// team change feed. The hub connection is independent of the agent
// connection (`ws.svelte.ts`) and stays up across agent switches.

import { agentLabel } from "./agent-name";
import { displayState, type AgentDisplayState } from "./agent-display-state";
import { WsTransport } from "./transport.svelte";
import { hubWsUrl } from "./paths";
import { notifications } from "./notifications.svelte";
import { userErrorMessage } from "./errors";
import {
  createAgent as apiCreateAgent,
  deleteAgent as apiDeleteAgent,
  fetchAgents,
  fetchDeletedAgents,
  restartAgent as apiRestartAgent,
  restoreAgent as apiRestoreAgent,
  setAgentAutostart as apiSetAgentAutostart,
  setAgentVisibility as apiSetAgentVisibility,
  startAgent as apiStartAgent,
  stopAgent as apiStopAgent,
} from "./api";
import type {
  A2aVisibility,
  Actor,
  AgentActivity,
  AgentListResponse,
  AgentSummary,
  CreateAgentRequest,
  DeleteOutcome,
  DeletedAgent,
  HubClientMessage,
  HubServerMessage,
  NoticeLevel,
  SystemOneStatus,
} from "./hub-types";
import type { ToastAction } from "./toast.svelte";
import { normalizeTeamWatchPrefix } from "./workspace-watch";
import { WatchRegistry } from "./watch-registry";

/** How many notices the store keeps for recall. */
const MAX_NOTICES = 50;

/** A hub notice, kept after its toast is gone. */
export interface HubNotice {
  id: number;
  level: NoticeLevel;
  message: string;
  /** The agent it concerns, if any. */
  agent?: string;
  at: Date;
}

/** Who acted, as a sentence subject: "You" for the user, else the teammate's name. */
function actorLabel(by: Actor): string {
  return by === "user" ? "You" : by.slice("agent:".length);
}

const IDLE: AgentActivity = { busy: false, busy_since: null, unread: 0 };

function byName(a: AgentSummary, b: AgentSummary): number {
  const left = agentLabel(a).toLowerCase();
  const right = agentLabel(b).toLowerCase();
  if (left !== right) return left < right ? -1 : 1;
  if (a.name === b.name) return 0;
  return a.name < b.name ? -1 : 1;
}

export class HubStore {
  /** Every agent, sorted by name. */
  agents = $state<AgentSummary[]>([]);
  /** The agent list has arrived, from the hub socket or a fetch. */
  loaded = $state(false);
  /** Busy (and since when) and unread per agent name. Agents with no report are idle. */
  activity = $state<Record<string, AgentActivity>>({});
  /** Running agents whose stop has begun but not finished. Their state still reads `running`. */
  stopping = $state<string[]>([]);
  /** Notices received this session, newest first. */
  notices = $state<HubNotice[]>([]);
  /** The decision model service: configured or not, and any outage. Null until the socket says. */
  systemOne = $state<SystemOneStatus | null>(null);
  /**
   * Deleted agents that can be restored, newest deletion first. Empty until
   * `refreshDeleted` has run; kept current after that. Never lists an agent
   * that exists.
   */
  deleted = $state<DeletedAgent[]>([]);
  /** `refreshDeleted` has finished at least once. */
  deletedLoaded = $state(false);
  /** Why the last `refreshDeleted` failed, in plain words, or null. */
  deletedError = $state<string | null>(null);

  readonly transport: WsTransport<HubServerMessage, HubClientMessage>;

  /**
   * The team change feed. Whatever follows team files registers here with
   * `team` or `team/...` prefixes, and the registry keeps the hub socket's one
   * watch set the union of theirs.
   */
  readonly teamWatches = new WatchRegistry({
    send: (prefixes) => {
      this.transport.send({ type: "watch_team", prefixes });
    },
    normalize: normalizeTeamWatchPrefix,
    refusal: (prefix) =>
      `can't watch "${prefix}": team watch paths are "team" or start with "team/", like "team/wiki"`,
  });

  private noticeCounter = 0;
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
      this.teamWatches.connected();
    };
    this.transport.onDisconnected = () => {
      this.teamWatches.disconnected();
    };
  }

  /** Connect to the hub socket. Its first frame is the agent list. */
  connect(): void {
    this.transport.connect();
  }

  /** Close the hub socket. */
  disconnect(): void {
    this.transport.disconnect();
    this.teamWatches.disconnected();
  }

  /**
   * Fetch the agent list over HTTP. The socket's snapshot is newer, so a
   * fetch that lands after one is ignored unless `force` is set, for when
   * the caller knows the list changed outside the socket's view. Throws
   * `ApiError` on failure.
   */
  async refresh(force = false): Promise<void> {
    const list = await fetchAgents();
    if (this.snapshotSeen && !force) return;
    this.setList(list);
  }

  /** Give up waiting for the hub socket's next reconnect attempt and try now. */
  reconnectNow(): void {
    this.transport.reconnectNow();
  }

  /**
   * Fetch the deleted agents. A failure lands in `deletedError` for the view
   * that shows the list, since a toast would give it nothing to retry.
   */
  async refreshDeleted(): Promise<void> {
    try {
      this.deleted = await fetchDeletedAgents();
      this.deletedError = null;
    } catch (err) {
      this.deletedError = userErrorMessage(err, { action: "Couldn't load the deleted agents." });
    } finally {
      this.deletedLoaded = true;
    }
  }

  // ── Reading ────────────────────────────────────────────────────────

  agent(name: string): AgentSummary | undefined {
    return this.agents.find((a) => a.name === name);
  }

  /** The name people see for a folder name. The folder itself when it isn't loaded. */
  shownName(name: string): string {
    const agent = this.agent(name);
    return agent === undefined ? name : agentLabel(agent);
  }

  activityOf(name: string): AgentActivity {
    return this.activity[name] ?? IDLE;
  }

  isStopping(name: string): boolean {
    return this.stopping.includes(name);
  }

  /** Whether the agent is up and not on its way down, the state its live status and checks can be read in. */
  isRunning(name: string): boolean {
    return this.displayStateOf(name) === "running";
  }

  /** How to show the agent's state, or null while the list doesn't name it. */
  displayStateOf(name: string): AgentDisplayState | null {
    const agent = this.agent(name);
    return agent === undefined ? null : displayState(agent.state, this.isStopping(name));
  }

  // ── Hub frames ─────────────────────────────────────────────────────

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
      this.deletedListChanged();
      return result;
    } catch (err) {
      this.reportFailure(err, `Couldn't delete ${name}.`);
      return null;
    }
  }

  /**
   * Restore a deleted agent. `checkpointId` names the workspace checkpoint to
   * restore its files from; without it the hub uses the last one taken before
   * the deletion. Resolves to the restored agent, or null after telling the
   * user why not.
   */
  async restoreAgent(name: string, checkpointId?: string): Promise<AgentSummary | null> {
    try {
      const agent = await apiRestoreAgent(name, checkpointId);
      this.upsert(agent);
      return agent;
    } catch (err) {
      this.reportFailure(err, `Couldn't restore ${name}.`, "There is nothing to restore for it.");
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

  private reportFailure(
    err: unknown,
    action: string,
    notFound = "That agent no longer exists.",
  ): void {
    notifications.surface("error", userErrorMessage(err, { action, notFound }));
  }

  // ── Frames ─────────────────────────────────────────────────────────

  /** Fold one hub frame into the store. */
  handleFrame(msg: HubServerMessage): void {
    switch (msg.type) {
      case "agents_snapshot":
        this.snapshotSeen = true;
        this.setList(msg);
        break;
      case "agent_state":
        this.noteFailure(msg.agent);
        this.upsert(msg.agent);
        // A state change ends a stop that was under way.
        this.stopping = this.stopping.filter((name) => name !== msg.agent.name);
        break;
      case "agent_stopping":
        if (!this.isStopping(msg.name)) this.stopping = [...this.stopping, msg.name];
        break;
      case "agent_created":
        this.upsert(msg.agent);
        this.addNotice("info", `${actorLabel(msg.by)} created ${agentLabel(msg.agent)}.`);
        break;
      case "agent_restored":
        this.upsert(msg.agent);
        this.addNotice("info", `${actorLabel(msg.by)} restored ${agentLabel(msg.agent)}.`);
        break;
      case "agent_deleted": {
        const gone = this.agents.find((agent) => agent.name === msg.name);
        const label = gone === undefined ? msg.name : agentLabel(gone);
        this.removeAgent(msg.name);
        this.deletedListChanged();
        this.addNotice("info", `${actorLabel(msg.by)} deleted ${label}.`, undefined, {
          label: "Undo",
          onClick: () => {
            void this.restoreAgent(msg.name);
          },
        });
        break;
      }
      case "agent_activity":
        this.activity = {
          ...this.activity,
          [msg.name]: { busy: msg.busy, busy_since: msg.busy_since, unread: msg.unread },
        };
        break;
      case "notice":
        this.addNotice(msg.level, msg.message, msg.agent);
        break;
      case "workspace_watch_unavailable":
        this.addNotice("warn", msg.message);
        break;
      case "system_one_status":
        this.systemOne = msg.status;
        break;
      case "pong":
        // The answer to a `ping`, which the app itself never sends.
        break;
      case "hub_boot":
      case "hub_config_reloaded":
      case "team_event":
      case "agent_overview":
      case "artifact_updated":
      case "artifact_removed":
      case "subscribed":
      case "session_frame":
      case "session_relay_lagged":
        // Frame listeners act on these, below.
        break;
      case "workspace_changed":
      case "workspace_resync":
        // Only the team watch owners act on these, below.
        break;
    }
    for (const listener of this.frameListeners) listener(msg);
    this.teamWatches.handleFrame(msg);
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

  /** Take a whole list: the socket's snapshot, or the same three fields fetched over HTTP. */
  private setList({ agents, activity, stopping }: AgentListResponse): void {
    this.agents = [...agents].sort(byName);
    this.activity = { ...activity };
    this.stopping = [...stopping];
    this.loaded = true;
  }

  private upsert(agent: AgentSummary): void {
    this.agents = [...this.agents.filter((a) => a.name !== agent.name), agent].sort(byName);
    this.deleted = this.deleted.filter((d) => d.name !== agent.name);
    this.loaded = true;
  }

  /** The deleted list gained an entry; it is fetched again only once someone has asked for it. */
  private deletedListChanged(): void {
    if (this.deletedLoaded) void this.refreshDeleted();
  }

  private removeAgent(name: string): void {
    this.agents = this.agents.filter((a) => a.name !== name);
    this.activity = Object.fromEntries(Object.entries(this.activity).filter(([n]) => n !== name));
    this.stopping = this.stopping.filter((n) => n !== name);
  }

  private addNotice(
    level: NoticeLevel,
    message: string,
    agent?: string,
    action?: ToastAction,
  ): void {
    // eslint-disable-next-line svelte/prefer-svelte-reactivity -- a timestamp, never mutated
    const at = new Date();
    const notice: HubNotice = { id: ++this.noticeCounter, level, message, agent, at };
    this.notices = [notice, ...this.notices].slice(0, MAX_NOTICES);
    notifications.surface(
      level === "error" ? "error" : "notice",
      agent && !message.startsWith(agent) ? `${agent}: ${message}` : message,
      undefined,
      action,
    );
  }
}

export const hub = new HubStore();
