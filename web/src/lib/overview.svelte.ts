// ── Team overview (Svelte 5 runes) ───────────────────────────────────
//
// What Home and the rail's Home count show beyond the hub's agent list: each
// agent's overview, the team event log, and the newest unread inbox items.
// The hub socket feeds it (`agent_overview`, `team_event`), and it fetches
// again whenever the socket can't vouch for what it holds: on every
// connection, after a snapshot sent in place of lost frames, and when the
// hub process changed. The agent list, activity and stopping set stay in the
// hub store, which this reads.

import {
  ApiError,
  fetchHubInbox,
  fetchOverview,
  fetchTeamEvents,
  stopOutboundA2aTask,
  stopWatchingOutboundA2aTask,
} from "./api";
import { userErrorMessage } from "./errors";
import { hub } from "./hub.svelte";
import type {
  AgentOverview,
  AgentSummary,
  HubInboxItem,
  HubServerMessage,
  TeamEvent,
} from "./hub-types";
import { deriveNeedsYou, INBOX_ITEMS_SHOWN, type NeedsYou } from "./needs-you";
import { notifications } from "./notifications.svelte";
import { unreachableAgentMessage } from "./sessions.svelte";

/** How many team events the store keeps, newest first. */
const MAX_EVENTS = 50;

/** How many inbox items one request asks for while looking for the newest unread ones. */
const INBOX_PAGE = 50;

/** What the store reads from the hub store. */
export interface OverviewHub {
  readonly agents: readonly AgentSummary[];
  onFrame: (listener: (msg: HubServerMessage) => void) => () => void;
}

function newestFirst(a: TeamEvent, b: TeamEvent): number {
  return b.id - a.id;
}

export class OverviewStore {
  /** The hub process this connection belongs to, as `hub_boot` announced it. */
  bootId = $state<string | null>(null);
  /** Each agent's overview by name. Empty until the first fetch lands. */
  overviews = $state<Record<string, AgentOverview>>({});
  /** The overview has arrived from the current hub process. */
  loaded = $state(false);
  /** Why the last overview fetch failed, in plain words, or null. */
  loadError = $state<string | null>(null);

  /** Team events, newest first, at most `MAX_EVENTS`. */
  events = $state<TeamEvent[]>([]);
  eventsLoaded = $state(false);
  eventsError = $state<string | null>(null);

  /** The newest unread user-inbox items across agents, at most five. */
  unreadItems = $state<HubInboxItem[]>([]);
  unreadItemsError = $state<string | null>(null);

  /** Outbound tasks whose stop is in flight, as `agent:task`. */
  stoppingTasks = $state<string[]>([]);
  /** Why a task's stop couldn't reach its agent, by `agent:task`. */
  taskNotes = $state<Record<string, string>>({});

  /** What needs the user right now. */
  readonly needsYou: NeedsYou = $derived.by(() =>
    deriveNeedsYou({
      agents: this.source.agents,
      overviews: this.overviews,
      unreadItems: this.unreadItems,
    }),
  );

  /** The next snapshot is the one every connection starts with, not one sent after lost frames. */
  private awaitingConnectSnapshot = false;
  private overviewRequest = 0;
  /** Agents whose overview frame arrived while the overview request was out. */
  private framedDuringFetch: string[] = [];
  private eventsRequest = 0;
  private unreadRequest = 0;
  /** The per-agent unread counts the inbox items were last fetched for. */
  private unreadCountsFetched: string | null = null;

  constructor(private readonly source: OverviewHub) {}

  /** Follow the hub socket's frames. Returns a function that stops following them. */
  start(): () => void {
    return this.source.onFrame((msg) => {
      this.handleFrame(msg);
    });
  }

  overviewOf(name: string): AgentOverview | undefined {
    return this.overviews[name];
  }

  /** Unread user-inbox items across every agent. */
  get inboxUnread(): number {
    return Object.values(this.overviews).reduce((sum, o) => sum + o.inbox_unread, 0);
  }

  handleFrame(msg: HubServerMessage): void {
    switch (msg.type) {
      case "hub_boot":
        this.connected(msg.boot_id);
        break;
      case "agents_snapshot":
        if (this.awaitingConnectSnapshot) {
          this.awaitingConnectSnapshot = false;
        } else {
          // The hub sends a snapshot in place of frames this connection lost.
          void this.refreshOverview();
          void this.refreshEvents();
        }
        break;
      case "agent_overview":
        this.framedDuringFetch.push(msg.overview.name);
        this.setOverviews({ ...this.overviews, [msg.overview.name]: msg.overview });
        break;
      case "agent_deleted":
        this.setOverviews(
          Object.fromEntries(Object.entries(this.overviews).filter(([name]) => name !== msg.name)),
        );
        break;
      case "team_event":
        if (msg.boot_id !== this.bootId) {
          this.events = [];
          void this.refreshEvents();
        } else if (!this.events.some((event) => event.id === msg.event.id)) {
          this.events = [msg.event, ...this.events].sort(newestFirst).slice(0, MAX_EVENTS);
        }
        break;
      case "agent_state":
      case "agent_stopping":
      case "agent_created":
      case "agent_restored":
      case "agent_activity":
      case "notice":
      case "hub_config_reloaded":
      case "workspace_changed":
      case "workspace_resync":
      case "workspace_watch_unavailable":
      case "artifact_updated":
      case "artifact_removed":
      case "subscribed":
      case "session_frame":
      case "session_relay_lagged":
        // Other owners keep these; an agent's overview changes reach here as its own frame.
        break;
    }
  }

  /** A connection opened: what it says from now on belongs to `bootId`. */
  private connected(bootId: string): void {
    if (this.bootId !== bootId) {
      // A different hub process: nothing held came from it.
      this.overviews = {};
      this.loaded = false;
      this.events = [];
      this.eventsLoaded = false;
      this.unreadItems = [];
      this.unreadCountsFetched = null;
    }
    this.bootId = bootId;
    this.awaitingConnectSnapshot = true;
    void this.refreshOverview();
    void this.refreshEvents();
  }

  /** Fetch every agent's overview. A failure lands in `loadError`. */
  async refreshOverview(): Promise<void> {
    const request = ++this.overviewRequest;
    this.framedDuringFetch = [];
    try {
      const response = await fetchOverview();
      if (request !== this.overviewRequest) return;
      // An answer from another hub process is stale; that process's own
      // connection fetches again.
      if (this.bootId !== null && response.boot_id !== this.bootId) return;
      const next: Record<string, AgentOverview> = {};
      for (const overview of response.agents) {
        // A frame that arrived while the request was out is at least as new.
        const framed = this.framedDuringFetch.includes(overview.name)
          ? this.overviews[overview.name]
          : undefined;
        next[overview.name] = framed ?? overview;
      }
      this.setOverviews(next);
      this.loaded = true;
      this.loadError = null;
    } catch (err) {
      if (request !== this.overviewRequest) return;
      this.loadError = userErrorMessage(err, {
        action: "Couldn't load what your agents are doing.",
      });
    }
  }

  /**
   * Fetch the team events newer than the newest one held, or the newest page
   * when none are held. A failure lands in `eventsError`.
   */
  async refreshEvents(): Promise<void> {
    const request = ++this.eventsRequest;
    const after = this.events[0]?.id;
    try {
      const page = await fetchTeamEvents(
        after === undefined ? { limit: MAX_EVENTS } : { after, limit: MAX_EVENTS },
      );
      if (request !== this.eventsRequest) return;
      if (this.bootId !== null && page.boot_id !== this.bootId) return;
      const held = after === undefined ? [] : this.events;
      const ids = held.map((event) => event.id);
      this.events = [...page.events.filter((event) => !ids.includes(event.id)), ...held]
        .sort(newestFirst)
        .slice(0, MAX_EVENTS);
      this.eventsLoaded = true;
      this.eventsError = null;
    } catch (err) {
      if (request !== this.eventsRequest) return;
      this.eventsError = userErrorMessage(err, {
        action: "Couldn't load what happened across the team.",
      });
    }
  }

  /**
   * Fetch the newest unread inbox items, paging until five are found or the
   * inbox ends. A failure lands in `unreadItemsError`.
   */
  async refreshUnreadItems(): Promise<void> {
    const request = ++this.unreadRequest;
    this.unreadCountsFetched = this.unreadCounts();
    if (this.inboxUnread === 0) {
      this.unreadItems = [];
      this.unreadItemsError = null;
      return;
    }
    try {
      const found: HubInboxItem[] = [];
      let before: string | undefined;
      do {
        const page = await fetchHubInbox({ status: "active", limit: INBOX_PAGE, before });
        found.push(...page.items.filter((item) => !item.read));
        before = page.next_cursor ?? undefined;
      } while (found.length < INBOX_ITEMS_SHOWN && before !== undefined);
      if (request !== this.unreadRequest) return;
      this.unreadItems = found.slice(0, INBOX_ITEMS_SHOWN);
      this.unreadItemsError = null;
    } catch (err) {
      if (request !== this.unreadRequest) return;
      this.unreadItemsError = userErrorMessage(err, {
        action: "Couldn't load your newest inbox items.",
      });
    }
  }

  // ── Outbound tasks ─────────────────────────────────────────────────

  isStoppingTask(agent: string, taskId: string): boolean {
    return this.stoppingTasks.includes(`${agent}:${taskId}`);
  }

  /** Ask the task's remote agent to cancel it. When the agent can't be reached, say so on the task. */
  async stopTask(agent: string, taskId: string): Promise<void> {
    await this.taskCommand(agent, taskId, "Couldn't stop the task.", async () => {
      await stopOutboundA2aTask(agent, taskId);
    });
  }

  /** Stop following the task here, without reaching its agent. */
  async stopWatching(agent: string, taskId: string): Promise<void> {
    await this.taskCommand(agent, taskId, "Couldn't stop watching the task.", async () => {
      await stopWatchingOutboundA2aTask(agent, taskId);
    });
  }

  private async taskCommand(
    agent: string,
    taskId: string,
    action: string,
    call: () => Promise<void>,
  ): Promise<void> {
    const key = `${agent}:${taskId}`;
    if (this.stoppingTasks.includes(key)) return;
    this.stoppingTasks = [...this.stoppingTasks, key];
    try {
      await call();
      this.clearTaskNote(key);
      // The hub's frame follows within a second; the task is already over.
      this.dropProblem(agent, taskId);
    } catch (err) {
      const unreachable = unreachableAgentMessage(err);
      if (unreachable !== null) {
        this.taskNotes = { ...this.taskNotes, [key]: unreachable };
      } else if (err instanceof ApiError && err.status === 404) {
        // It ended some other way.
        this.dropProblem(agent, taskId);
      } else {
        notifications.surface("error", userErrorMessage(err, { action }));
      }
    } finally {
      this.stoppingTasks = this.stoppingTasks.filter((k) => k !== key);
    }
  }

  private clearTaskNote(key: string): void {
    if (!(key in this.taskNotes)) return;
    this.taskNotes = Object.fromEntries(Object.entries(this.taskNotes).filter(([k]) => k !== key));
  }

  private dropProblem(agent: string, taskId: string): void {
    const overview = this.overviews[agent];
    if (overview === undefined) return;
    this.overviews = {
      ...this.overviews,
      [agent]: {
        ...overview,
        outbound_problems: overview.outbound_problems.filter((p) => p.task_id !== taskId),
      },
    };
  }

  // ── Keeping the inbox items in step ────────────────────────────────

  private setOverviews(next: Record<string, AgentOverview>): void {
    this.overviews = next;
    if (this.unreadCounts() !== this.unreadCountsFetched) void this.refreshUnreadItems();
  }

  /** Each agent's unread count, as one comparable value: it changes whenever any count does. */
  unreadCounts(): string {
    return Object.values(this.overviews)
      .map((o) => `${o.name}=${String(o.inbox_unread)}`)
      .sort()
      .join(",");
  }
}

export const overview = new OverviewStore(hub);
