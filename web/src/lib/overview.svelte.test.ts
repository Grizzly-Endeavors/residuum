import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeWebSocket } from "../test/fake-websocket";
import { snapshot } from "../test/hub-frames";
import { HubStore } from "./hub.svelte";
import type {
  AgentOverview,
  AgentSummary,
  HubInboxItem,
  HubInboxPage,
  OverviewResponse,
  TeamEvent,
  TeamEventPage,
} from "./hub-types";
import { notifications } from "./notifications.svelte";
import { OverviewStore } from "./overview.svelte";
import { toast } from "./toast.svelte";
import { waitFor } from "../test/wait";

function agent(name: string, overrides: Partial<AgentSummary> = {}): AgentSummary {
  return {
    name,
    display_name: name,
    state: "running",
    last_error: null,
    autostart: true,
    role: null,
    a2a_visibility: "private",
    teams_configured: false,
    ...overrides,
  };
}

function overviewOf(name: string, overrides: Partial<AgentOverview> = {}): AgentOverview {
  return {
    name,
    last_message: null,
    live_sessions: [],
    upcoming: [],
    inbox_unread: 0,
    outbound_problems: [],
    ...overrides,
  };
}

function teamEvent(id: number, summary = `event ${String(id)}`): TeamEvent {
  return {
    id,
    at: "2026-03-14T12:00:00Z",
    agent: null,
    kind: "hub_notice",
    level: "info",
    summary,
    target: null,
  };
}

function inboxItem(id: string, read = false): HubInboxItem {
  return {
    agent: "atlas",
    id,
    title: id,
    body: "",
    source: "agent",
    at: "2026-03-14T12:00:00Z",
    read,
    attachments: [],
  };
}

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

/** What the stubbed hub answers. Tests change these between steps. */
let answers: {
  overview: OverviewResponse;
  events: (url: URL) => TeamEventPage;
  inbox: (url: URL) => HubInboxPage;
  /** Held back until released, for a test about a request that is still out. */
  holdOverview: Promise<void> | null;
  task: (url: URL) => Response;
};
let requests: string[] = [];

function stubHub(): void {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (path: string, init?: RequestInit): Promise<Response> => {
      const url = new URL(path, "http://localhost");
      const method = init?.method ?? "GET";
      requests.push(`${method} ${url.pathname}${url.search}`);
      if (url.pathname === "/api/hub/overview") {
        const answer = answers.overview;
        if (answers.holdOverview) await answers.holdOverview;
        return json(answer);
      }
      if (url.pathname === "/api/hub/events") return json(answers.events(url));
      if (url.pathname === "/api/hub/inbox") return json(answers.inbox(url));
      if (url.pathname.includes("/a2a/outbound/")) return answers.task(url);
      return json({ error: `unexpected ${url.pathname}` }, 500);
    }),
  );
}

const BOOT = "boot-1";

let hub: HubStore;
let store: OverviewStore;
let stop: () => void;

beforeEach(() => {
  FakeWebSocket.install();
  vi.stubGlobal("location", { protocol: "http:", host: "localhost:7700" });
  requests = [];
  answers = {
    overview: { boot_id: BOOT, agents: [overviewOf("atlas"), overviewOf("scout")] },
    events: () => ({ boot_id: BOOT, events: [teamEvent(2), teamEvent(1)], next_before: null }),
    inbox: () => ({ items: [], next_cursor: null }),
    holdOverview: null,
    task: () => json({}),
  };
  stubHub();
  hub = new HubStore();
  store = new OverviewStore(hub);
  stop = store.start();
});

afterEach(() => {
  stop();
  vi.unstubAllGlobals();
  notifications.history = [];
  for (const id of [...toast.toasts.keys()]) toast.dismiss(id);
});

/** The frames every hub socket connection starts with. */
function connect(bootId = BOOT, agents = [agent("atlas"), agent("scout")]): void {
  hub.handleFrame({ type: "hub_boot", boot_id: bootId });
  hub.handleFrame(snapshot(agents));
}

const overviewRequests = (): number => requests.filter((r) => r.includes("/overview")).length;
const eventRequests = (): string[] => requests.filter((r) => r.includes("/events"));

describe("OverviewStore on connect", () => {
  it("fetches the overview and the team events when the hub socket connects", async () => {
    connect();
    await waitFor(() => {
      expect(store.loaded).toBe(true);
      expect(store.eventsLoaded).toBe(true);
    });
    expect(Object.keys(store.overviews)).toEqual(["atlas", "scout"]);
    expect(store.events.map((e) => e.id)).toEqual([2, 1]);
    expect(eventRequests()).toEqual(["GET /api/hub/events?limit=50"]);
  });

  it("keeps what it holds when the same hub reconnects, and asks only for newer events", async () => {
    connect();
    await waitFor(() => {
      expect(store.eventsLoaded).toBe(true);
    });
    answers.events = () => ({ boot_id: BOOT, events: [teamEvent(3)], next_before: null });

    connect();
    expect(store.loaded).toBe(true);
    await waitFor(() => {
      expect(store.events.map((e) => e.id)).toEqual([3, 2, 1]);
    });
    expect(eventRequests().at(-1)).toBe("GET /api/hub/events?after=2&limit=50");
    expect(overviewRequests()).toBe(2);
  });

  it("drops everything from the old hub process when the boot id changes", async () => {
    connect();
    await waitFor(() => {
      expect(store.eventsLoaded).toBe(true);
    });
    answers.overview = { boot_id: "boot-2", agents: [overviewOf("atlas", { inbox_unread: 0 })] };
    answers.events = () => ({
      boot_id: "boot-2",
      events: [teamEvent(1, "fresh")],
      next_before: null,
    });

    hub.handleFrame({ type: "hub_boot", boot_id: "boot-2" });
    expect(store.overviews).toEqual({});
    expect(store.loaded).toBe(false);
    expect(store.events).toEqual([]);

    await waitFor(() => {
      expect(store.events.map((e) => e.summary)).toEqual(["fresh"]);
    });
    expect(Object.keys(store.overviews)).toEqual(["atlas"]);
    expect(eventRequests().at(-1)).toBe("GET /api/hub/events?limit=50");
  });

  it("ignores an overview answered by a different hub process than the socket announced", async () => {
    answers.overview = { boot_id: "boot-old", agents: [overviewOf("atlas")] };
    connect();
    await waitFor(() => {
      expect(overviewRequests()).toBe(1);
    });
    await waitFor(() => {
      expect(store.eventsLoaded).toBe(true);
    });
    expect(store.loaded).toBe(false);
    expect(store.overviews).toEqual({});
  });
});

describe("OverviewStore after lag", () => {
  it("refetches the overview and the newer events after a snapshot sent in place of lost frames", async () => {
    connect();
    await waitFor(() => {
      expect(store.eventsLoaded).toBe(true);
    });
    expect(overviewRequests()).toBe(1);

    answers.overview = {
      boot_id: BOOT,
      agents: [overviewOf("atlas", { inbox_unread: 0, live_sessions: [] }), overviewOf("scout")],
    };
    answers.events = () => ({ boot_id: BOOT, events: [teamEvent(5)], next_before: null });
    hub.handleFrame(snapshot([agent("atlas"), agent("scout")]));

    await waitFor(() => {
      expect(store.events.map((e) => e.id)).toEqual([5, 2, 1]);
    });
    expect(overviewRequests()).toBe(2);
    expect(eventRequests().at(-1)).toBe("GET /api/hub/events?after=2&limit=50");
  });

  it("doesn't refetch for the snapshot every connection starts with", async () => {
    connect();
    await waitFor(() => {
      expect(store.loaded).toBe(true);
    });
    expect(overviewRequests()).toBe(1);
    expect(eventRequests()).toHaveLength(1);
  });
});

describe("OverviewStore frames", () => {
  it("replaces an agent's overview with each frame, and forgets a deleted agent", async () => {
    connect();
    await waitFor(() => {
      expect(store.loaded).toBe(true);
    });
    const busier = overviewOf("atlas", {
      live_sessions: [
        {
          address: "a",
          run_id: "r",
          category: "spawned",
          source_label: "agent:x",
          purpose: "Researching",
          state: "running",
          started_at: "2026-03-14T12:00:00Z",
        },
      ],
    });
    hub.handleFrame({ type: "agent_overview", overview: busier });
    expect(store.overviewOf("atlas")).toEqual(busier);

    hub.handleFrame({ type: "agent_deleted", name: "scout", by: "user" });
    expect(store.overviewOf("scout")).toBeUndefined();
  });

  it("keeps a frame that arrived while the overview request was out over the request's answer", async () => {
    let release: () => void = () => {};
    answers.holdOverview = new Promise((resolve) => {
      release = resolve;
    });
    connect();
    await waitFor(() => {
      expect(overviewRequests()).toBe(1);
    });
    const framed = overviewOf("atlas", { inbox_unread: 3 });
    hub.handleFrame({ type: "agent_overview", overview: framed });

    release();
    await waitFor(() => {
      expect(store.loaded).toBe(true);
    });
    expect(store.overviewOf("atlas")).toEqual(framed);
    expect(store.overviewOf("scout")).toEqual(overviewOf("scout"));
  });

  it("adds each team event once, newest first", async () => {
    connect();
    await waitFor(() => {
      expect(store.eventsLoaded).toBe(true);
    });
    hub.handleFrame({ type: "team_event", boot_id: BOOT, event: teamEvent(3) });
    hub.handleFrame({ type: "team_event", boot_id: BOOT, event: teamEvent(3) });
    expect(store.events.map((e) => e.id)).toEqual([3, 2, 1]);
  });

  it("starts the events over when one comes from another hub process", async () => {
    connect();
    await waitFor(() => {
      expect(store.eventsLoaded).toBe(true);
    });
    hub.handleFrame({ type: "team_event", boot_id: "boot-other", event: teamEvent(9) });
    expect(store.events).toEqual([]);
    await waitFor(() => {
      expect(store.events.map((e) => e.id)).toEqual([2, 1]);
    });
    expect(eventRequests().at(-1)).toBe("GET /api/hub/events?limit=50");
  });
});

describe("OverviewStore inbox items", () => {
  it("fetches the newest unread items when a count changes, paging until it has five", async () => {
    answers.overview = {
      boot_id: BOOT,
      agents: [overviewOf("atlas", { inbox_unread: 6 }), overviewOf("scout")],
    };
    answers.inbox = (url) =>
      url.searchParams.get("before") === null
        ? {
            items: [inboxItem("a"), inboxItem("b", true), inboxItem("c")],
            next_cursor: "cursor-1",
          }
        : {
            items: [inboxItem("d"), inboxItem("e"), inboxItem("f"), inboxItem("g")],
            next_cursor: "cursor-2",
          };
    connect();
    await waitFor(() => {
      expect(store.unreadItems.map((i) => i.id)).toEqual(["a", "c", "d", "e", "f"]);
    });
    expect(requests.filter((r) => r.includes("/inbox"))).toEqual([
      "GET /api/hub/inbox?status=active&limit=50",
      "GET /api/hub/inbox?status=active&limit=50&before=cursor-1",
    ]);
    expect(store.needsYou.count).toBe(5);
    expect(store.needsYou.moreInInbox).toBe(1);

    // A frame with the same counts asks for nothing; a changed count asks again.
    hub.handleFrame({ type: "agent_overview", overview: overviewOf("atlas", { inbox_unread: 6 }) });
    const asked = requests.length;
    hub.handleFrame({ type: "agent_overview", overview: overviewOf("atlas", { inbox_unread: 0 }) });
    await waitFor(() => {
      expect(store.unreadItems).toEqual([]);
    });
    expect(requests).toHaveLength(asked);
  });

  it("says when the items can't be loaded, and keeps counting them", async () => {
    answers.overview = { boot_id: BOOT, agents: [overviewOf("atlas", { inbox_unread: 2 })] };
    answers.inbox = () => {
      throw new Error("down");
    };
    connect(BOOT, [agent("atlas")]);
    await waitFor(() => {
      expect(store.unreadItemsError).toContain("Couldn't load your newest inbox items");
    });
    expect(store.needsYou.count).toBe(2);
  });
});

describe("OverviewStore outbound tasks", () => {
  const STUCK = {
    task_id: "task-1",
    remote_agent: "laptop",
    status_text: null,
    unreachable_since: "2026-03-14T11:00:00Z",
  };

  beforeEach(async () => {
    answers.overview = {
      boot_id: BOOT,
      agents: [overviewOf("atlas", { outbound_problems: [STUCK] })],
    };
    connect(BOOT, [agent("atlas")]);
    await waitFor(() => {
      expect(store.needsYou.items).toHaveLength(1);
    });
  });

  it("drops the problem as soon as Stop watching succeeds, and answers with the task", async () => {
    answers.task = () => json({ task_id: "task-1", open: false });
    await expect(store.stopWatching("atlas", "task-1")).resolves.toMatchObject({
      task_id: "task-1",
      open: false,
    });
    expect(requests).toContain("POST /api/agents/atlas/a2a/outbound/task-1/stop-watching");
    expect(store.needsYou.items).toEqual([]);
  });

  it("keeps the problem and notes why when Stop task can't reach the agent", async () => {
    answers.task = () =>
      json({ error: "laptop can't be reached right now.", code: "unreachable" }, 502);
    await expect(store.stopTask("atlas", "task-1")).resolves.toBeNull();
    expect(store.needsYou.items).toHaveLength(1);
    expect(store.taskNotes["atlas:task-1"]).toBe("laptop can't be reached right now.");

    answers.task = () => json({});
    await store.stopWatching("atlas", "task-1");
    expect(store.taskNotes).toEqual({});
    expect(store.needsYou.items).toEqual([]);
  });

  it("reports any other failure as an error and keeps the problem", async () => {
    answers.task = () => json({ error: "boom" }, 500);
    await store.stopWatching("atlas", "task-1");
    expect(store.needsYou.items).toHaveLength(1);
    expect([...toast.toasts.values()].some((t) => t.kind === "error")).toBe(true);
  });
});
