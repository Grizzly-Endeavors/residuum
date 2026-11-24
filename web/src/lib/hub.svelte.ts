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
import { hubNoticeText } from "./hub-notices";
import { hubReloadNotice } from "./reload-notices";

const MAX_NOTICES = 50;

export interface HubNotice {
  id: number;
  level: NoticeLevel;
  message: string;
  agent?: string;
  details?: string;
  at: Date;
}

interface NoticeExtras {
  agent?: string;
  details?: string;
  action?: ToastAction;
}

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
  agents = $state<AgentSummary[]>([]);
  loaded = $state(false);
  activity = $state<Record<string, AgentActivity>>({});
  stopping = $state<string[]>([]);
  notices = $state<HubNotice[]>([]);
  systemOne = $state<SystemOneStatus | null>(null);
  deleted = $state<DeletedAgent[]>([]);
  deletedLoaded = $state(false);
  deletedError = $state<string | null>(null);

  readonly transport: WsTransport<HubServerMessage, HubClientMessage>;

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
      this.teamWatches.connected();
    };
    this.transport.onDisconnected = () => {
      this.teamWatches.disconnected();
    };
  }

  connect(): void {
    this.transport.connect();
  }

  disconnect(): void {
    this.transport.disconnect();
    this.teamWatches.disconnected();
  }

  async refresh(force = false): Promise<void> {
    const list = await fetchAgents();
    if (this.snapshotSeen && !force) return;
    this.setList(list);
  }

  reconnectNow(): void {
    this.transport.reconnectNow();
  }

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

  agent(name: string): AgentSummary | undefined {
    return this.agents.find((a) => a.name === name);
  }

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

  isRunning(name: string): boolean {
    return this.displayStateOf(name) === "running";
  }

  displayStateOf(name: string): AgentDisplayState | null {
    const agent = this.agent(name);
    return agent === undefined ? null : displayState(agent.state, this.isStopping(name));
  }

  onFrame(listener: (msg: HubServerMessage) => void): () => void {
    this.frameListeners.add(listener);
    return () => this.frameListeners.delete(listener);
  }

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

  async restartAgent(name: string, force = false): Promise<boolean> {
    return this.lifecycle(name, (n) => apiRestartAgent(n, force), "restart");
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

  handleFrame(msg: HubServerMessage): void {
    switch (msg.type) {
      case "agents_snapshot":
        this.snapshotSeen = true;
        this.setList(msg);
        break;
      case "agent_state":
        this.noteFailure(msg.agent);
        this.upsert(msg.agent);
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
        this.addNotice("info", `${actorLabel(msg.by)} deleted ${label}.`, {
          action: {
            label: "Undo",
            onClick: () => {
              void this.restoreAgent(msg.name);
            },
          },
        });
        break;
      }
      case "agent_activity":
        this.activity = {
          ...this.activity,
          [msg.name]: {
            busy: msg.busy,
            busy_since: msg.busy_since,
            unread: msg.unread,
            ...(msg.restart_pending === true ? { restart_pending: true } : {}),
          },
        };
        break;
      case "notice": {
        const { message, details } = hubNoticeText(msg.kind, msg.message);
        this.addNotice(msg.level, message, { agent: msg.agent, details });
        break;
      }
      case "hub_config_reloaded": {
        const notice = hubReloadNotice(msg);
        if (notice !== null) {
          this.addNotice(notice.kind === "error" ? "error" : "info", notice.message, {
            details: notice.details,
          });
        }
        break;
      }
      case "workspace_watch_unavailable":
        this.addNotice("warn", msg.message);
        break;
      case "system_one_status":
        this.systemOne = msg.status;
        break;
      case "pong":
        break;
      case "hub_boot":
      case "team_event":
      case "agent_overview":
      case "artifact_updated":
      case "artifact_removed":
      case "subscribed":
      case "session_frame":
      case "session_relay_lagged":
        break;
      case "workspace_changed":
      case "workspace_resync":
        break;
    }
    for (const listener of this.frameListeners) listener(msg);
    this.teamWatches.handleFrame(msg);
  }

  private noteFailure(next: AgentSummary): void {
    if (next.state !== "failed") return;
    if (this.agent(next.name)?.state === "failed") return;
    const reason = next.last_error?.message;
    this.addNotice("error", reason ? `${next.name} failed: ${reason}` : `${next.name} failed.`, {
      agent: next.name,
    });
  }

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
    { agent, details, action }: NoticeExtras = {},
  ): void {
    // eslint-disable-next-line svelte/prefer-svelte-reactivity -- a timestamp, never mutated
    const at = new Date();
    const notice: HubNotice = { id: ++this.noticeCounter, level, message, agent, details, at };
    this.notices = [notice, ...this.notices].slice(0, MAX_NOTICES);
    notifications.surface(
      level === "error" ? "error" : "notice",
      agent && !message.startsWith(agent) ? `${agent}: ${message}` : message,
      details,
      action,
    );
  }
}

export const hub = new HubStore();
