import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as api from "./api";
import { invalidate } from "./cache";
import { userInbox } from "./inbox.svelte";
import { NoAgentSelectedError } from "./paths";
import { scheduled } from "./scheduled.svelte";
import { SessionsStore, SessionView } from "./sessions.svelte";
import { fetchModels } from "./models";

/** A working `localStorage`, which the Node test environment lacks. */
function stubStorage(): void {
  const data = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => data.get(key) ?? null,
    setItem: (key: string, value: string) => {
      data.set(key, value);
    },
    removeItem: (key: string) => {
      data.delete(key);
    },
    clear: () => {
      data.clear();
    },
  });
}

/** Answer every request with an empty success and record "METHOD url" for each. */
function recordRequests(): string[] {
  const seen: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((input: string, init?: RequestInit) => {
      seen.push(`${init?.method ?? "GET"} ${input}`);
      return Promise.resolve(
        new Response(
          JSON.stringify({
            live: [],
            completed: [],
            next_cursor: null,
            items: [],
            diff: null,
            models: [],
            messages: [],
            session: null,
            checkpoint_id: null,
          }),
          { status: 200, headers: { "Content-Type": "application/json" } },
        ),
      );
    }),
  );
  return seen;
}

/** The requests `call` makes. */
async function requestsOf(call: () => Promise<unknown>): Promise<string[]> {
  const seen = recordRequests();
  await call().catch(() => undefined);
  return seen;
}

beforeEach(() => {
  stubStorage();
});

afterEach(() => {
  vi.unstubAllGlobals();
  scheduled.reset(null);
  userInbox.reset(null);
});

const AGENTS = ["scout", "atlas"] as const;

/** One area's calls: a label, how to make the call for `agent`, and the request it makes. */
type Row = [string, (agent: string) => Promise<unknown>, string];

const AREAS: Record<string, Row[]> = {
  chat: [
    ["fetchStatus", (a) => api.fetchStatus(a), "GET /api/agents/{agent}/status"],
    ["fetchChatHistory", (a) => api.fetchChatHistory(a), "GET /api/agents/{agent}/chat/history"],
    ["fetchUsageTotals", (a) => api.fetchUsageTotals(a), "GET /api/agents/{agent}/usage"],
    [
      "fetchChatSegment",
      (a) => api.fetchChatSegment(a, "ep-1"),
      "GET /api/agents/{agent}/chat/history?episode=ep-1",
    ],
  ],
  config: [
    ["fetchConfigRaw", (a) => api.fetchConfigRaw(a), "GET /api/agents/{agent}/config/raw"],
    ["putConfigRaw", (a) => api.putConfigRaw(a, "x"), "PUT /api/agents/{agent}/config/raw"],
    [
      "patchConfig",
      (a) => api.patchConfig(a, { timezone: "UTC" }),
      "PATCH /api/agents/{agent}/config/patch",
    ],
    [
      "validateConfig",
      (a) => api.validateConfig(a, "x"),
      "POST /api/agents/{agent}/config/validate",
    ],
  ],
  providers: [
    ["fetchProvidersRaw", (a) => api.fetchProvidersRaw(a), "GET /api/agents/{agent}/providers/raw"],
    [
      "putProvidersRaw",
      (a) => api.putProvidersRaw(a, "x"),
      "PUT /api/agents/{agent}/providers/raw",
    ],
    [
      "patchProviders",
      (a) => api.patchProviders(a, { models: {} }),
      "PATCH /api/agents/{agent}/providers/patch",
    ],
    [
      "validateProviders",
      (a) => api.validateProviders(a, "x"),
      "POST /api/agents/{agent}/providers/validate",
    ],
    [
      "fetchProviderModels",
      (a) => api.fetchProviderModels(a, "anthropic"),
      "POST /api/agents/{agent}/providers/models",
    ],
  ],
  mcp: [
    ["fetchMcpRaw", (a) => api.fetchMcpRaw(a), "GET /api/agents/{agent}/mcp/raw"],
    ["putMcpRaw", (a) => api.putMcpRaw(a, "{}"), "PUT /api/agents/{agent}/mcp/raw"],
    ["patchMcp", (a) => api.patchMcp(a, { servers: {} }), "PATCH /api/agents/{agent}/mcp/patch"],
  ],
  workspace: [
    [
      "fetchWorkspaceFiles",
      (a) => api.fetchWorkspaceFiles(a, "wiki"),
      "GET /api/agents/{agent}/workspace/files?path=wiki",
    ],
    [
      "fetchWorkspaceFile",
      (a) => api.fetchWorkspaceFile(a, "a.md"),
      "GET /api/agents/{agent}/workspace/file?path=a.md",
    ],
    [
      "putWorkspaceFile",
      (a) => api.putWorkspaceFile(a, "a.md", "x", null),
      "PUT /api/agents/{agent}/workspace/file",
    ],
    [
      "validateWorkspaceFile",
      (a) => api.validateWorkspaceFile(a, "a.md", "x"),
      "POST /api/agents/{agent}/workspace/validate",
    ],
    [
      "deleteWorkspaceFile",
      (a) => api.deleteWorkspaceFile(a, "a.md"),
      "DELETE /api/agents/{agent}/workspace/file?path=a.md",
    ],
    [
      "moveWorkspaceFile",
      (a) => api.moveWorkspaceFile(a, "a.md", "b.md"),
      "POST /api/agents/{agent}/workspace/move",
    ],
  ],
  checkpoints: [
    [
      "fetchCheckpoints",
      (a) => api.fetchCheckpoints(a, { repo: "workspace" }),
      "GET /api/agents/{agent}/checkpoints?repo=workspace",
    ],
    [
      "fetchCheckpointStats",
      (a) => api.fetchCheckpointStats(a, "agent_config"),
      "GET /api/agents/{agent}/checkpoints/stats?repo=agent_config",
    ],
    [
      "fetchCheckpointDetail",
      (a) => api.fetchCheckpointDetail(a, "c1", "workspace"),
      "GET /api/agents/{agent}/checkpoints/c1?repo=workspace",
    ],
    [
      "fetchCheckpointDiff",
      (a) => api.fetchCheckpointDiff(a, "c1", "workspace", "a.md"),
      "GET /api/agents/{agent}/checkpoints/c1/diff?repo=workspace&path=a.md",
    ],
    [
      "fetchCheckpointFile",
      (a) => api.fetchCheckpointFile(a, "c1", "workspace", "a.md"),
      "GET /api/agents/{agent}/checkpoints/c1/file?repo=workspace&path=a.md",
    ],
    [
      "restoreCheckpoint",
      (a) => api.restoreCheckpoint(a, "c1", "agent_config", "a.md"),
      "POST /api/agents/{agent}/checkpoints/c1/restore",
    ],
    [
      "undoCheckpoint",
      (a) => api.undoCheckpoint(a, "c1", "workspace"),
      "POST /api/agents/{agent}/checkpoints/c1/undo",
    ],
  ],
  sessions: [
    [
      "fetchSessions",
      (a) => api.fetchSessions(a, { category: "spawned", limit: 5 }),
      "GET /api/agents/{agent}/sessions?category=spawned&limit=5",
    ],
    [
      "fetchSessionTranscript",
      (a) => api.fetchSessionTranscript(a, "r1"),
      "GET /api/agents/{agent}/sessions/runs/r1/transcript",
    ],
  ],
  inbox: [
    [
      "markUserInboxItemRead",
      (a) => api.markUserInboxItemRead(a, "i1"),
      "PUT /api/agents/{agent}/inbox/i1/read",
    ],
    [
      "archiveUserInboxItem",
      (a) => api.archiveUserInboxItem(a, "i1"),
      "POST /api/agents/{agent}/inbox/i1/archive",
    ],
    [
      "fetchArchivedUserInbox",
      (a) => api.fetchArchivedUserInbox(a),
      "GET /api/agents/{agent}/inbox/archive",
    ],
    [
      "restoreUserInboxItem",
      (a) => api.restoreUserInboxItem(a, "i1"),
      "POST /api/agents/{agent}/inbox/i1/restore",
    ],
  ],
  scheduled: [
    [
      "fetchScheduledPulses",
      (a) => api.fetchScheduledPulses(a),
      "GET /api/agents/{agent}/scheduled/pulses",
    ],
    [
      "setPulseEnabled",
      (a) => api.setPulseEnabled(a, "morning", true),
      "PUT /api/agents/{agent}/scheduled/pulses/morning/enabled",
    ],
    [
      "fetchScheduledActions",
      (a) => api.fetchScheduledActions(a),
      "GET /api/agents/{agent}/scheduled/actions",
    ],
    [
      "cancelScheduledAction",
      (a) => api.cancelScheduledAction(a, "act-1"),
      "DELETE /api/agents/{agent}/scheduled/actions/act-1",
    ],
  ],
  a2a: [
    ["fetchA2aStatus", (a) => api.fetchA2aStatus(a), "GET /api/agents/{agent}/a2a/status"],
    ["fetchA2aCard", (a) => api.fetchA2aCard(a), "GET /api/agents/{agent}/a2a/card"],
    ["fetchA2aAgents", (a) => api.fetchA2aAgents(a), "GET /api/agents/{agent}/a2a/agents"],
    [
      "fetchA2aAgentsRaw",
      (a) => api.fetchA2aAgentsRaw(a),
      "GET /api/agents/{agent}/a2a/agents/raw",
    ],
    [
      "putA2aAgentsRaw",
      (a) => api.putA2aAgentsRaw(a, "[]"),
      "PUT /api/agents/{agent}/a2a/agents/raw",
    ],
    [
      "fetchOutboundA2aTasks",
      (a) => api.fetchOutboundA2aTasks(a),
      "GET /api/agents/{agent}/a2a/outbound",
    ],
    [
      "stopOutboundA2aTask",
      (a) => api.stopOutboundA2aTask(a, "t1"),
      "POST /api/agents/{agent}/a2a/outbound/t1/stop",
    ],
    [
      "stopWatchingOutboundA2aTask",
      (a) => api.stopWatchingOutboundA2aTask(a, "t1"),
      "POST /api/agents/{agent}/a2a/outbound/t1/stop-watching",
    ],
  ],
};

describe("every agent-scoped call goes to the agent it names", () => {
  for (const [area, rows] of Object.entries(AREAS)) {
    describe(area, () => {
      it.each(rows)("%s", async (_name, call, template) => {
        for (const agent of AGENTS) {
          const requests = await requestsOf(() => call(agent));
          expect(requests).toEqual([template.replace("{agent}", agent)]);
        }
      });
    });
  }
});

describe("calls that are not one agent's ignore the agent they are given", () => {
  it("sends the team's workspace to the team whichever agent is named", async () => {
    for (const agent of [null, ...AGENTS]) {
      expect(await requestsOf(() => api.fetchWorkspaceFiles(agent, "wiki", "team"))).toEqual([
        "GET /api/team/workspace/files?path=wiki",
      ]);
    }
  });

  it("sends the hub and team checkpoint repositories to the hub whichever agent is named", async () => {
    for (const agent of [null, ...AGENTS]) {
      expect(await requestsOf(() => api.fetchCheckpoints(agent, { repo: "hub" }))).toEqual([
        "GET /api/hub/checkpoints?repo=hub",
      ]);
      expect(await requestsOf(() => api.fetchCheckpointStats(agent, "team"))).toEqual([
        "GET /api/hub/checkpoints/stats?repo=team",
      ]);
    }
  });

  it("asks the hub for provider models when no agent exists yet", async () => {
    expect(await requestsOf(() => api.fetchProviderModels(null, "anthropic"))).toEqual([
      "POST /api/hub/providers/models",
    ]);
  });
});

describe("a call that needs an agent and has none makes no request", () => {
  it.each<[string, () => Promise<unknown>]>([
    ["the agent's workspace", () => api.fetchWorkspaceFiles(null, "wiki")],
    [
      "an agent-level checkpoint repository",
      () => api.fetchCheckpoints(null, { repo: "workspace" }),
    ],
    ["an agent's config repository", () => api.undoCheckpoint(null, "c1", "agent_config")],
  ])("%s", async (_name, call) => {
    const seen = recordRequests();
    await expect(call()).rejects.toBeInstanceOf(NoAgentSelectedError);
    expect(seen).toEqual([]);
  });
});

describe("cache keys are per agent", () => {
  it.each([
    ["cacheKeyStatus", api.cacheKeyStatus],
    ["cacheKeyConfigRaw", api.cacheKeyConfigRaw],
    ["cacheKeyProvidersRaw", api.cacheKeyProvidersRaw],
    ["cacheKeyMcpRaw", api.cacheKeyMcpRaw],
    ["cacheKeyA2aAgentsRaw", api.cacheKeyA2aAgentsRaw],
  ])("%s differs between agents and names the agent", (_name, key) => {
    expect(key("scout")).not.toBe(key("atlas"));
    expect(key("scout")).toContain("/api/agents/scout/");
    expect(key("atlas")).toContain("/api/agents/atlas/");
  });

  it("does not answer one agent's read from another agent's cache", async () => {
    const seen = recordRequests();
    await api.fetchConfigRaw("scout").catch(() => undefined);
    await api.fetchConfigRaw("atlas").catch(() => undefined);
    expect(seen).toEqual(["GET /api/agents/scout/config/raw", "GET /api/agents/atlas/config/raw"]);
  });

  it("answers a repeat read for the same agent from its own cache", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.resolve(new Response("timezone = 'UTC'", { status: 200 }))),
    );
    await api.fetchConfigRaw("scout");
    await api.fetchConfigRaw("scout");
    expect(fetch).toHaveBeenCalledTimes(1);
  });

  it("clears only the written agent's entry on a save", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.resolve(new Response('{"valid":true}', { status: 200 }))),
    );
    await api.fetchProvidersRaw("scout");
    await api.fetchProvidersRaw("atlas");
    expect(fetch).toHaveBeenCalledTimes(2);

    await api.putProvidersRaw("atlas", "x");
    expect(fetch).toHaveBeenCalledTimes(3);

    await api.fetchProvidersRaw("scout");
    expect(fetch).toHaveBeenCalledTimes(3);
    await api.fetchProvidersRaw("atlas");
    expect(fetch).toHaveBeenCalledTimes(4);
  });

  it("keeps an episode cached for one agent from answering another's", async () => {
    const seen = recordRequests();
    invalidate("GET /api/agents/scout/chat/history?episode=ep-1");
    invalidate("GET /api/agents/atlas/chat/history?episode=ep-1");
    await api.fetchChatSegment("scout", "ep-1").catch(() => undefined);
    await api.fetchChatSegment("atlas", "ep-1").catch(() => undefined);
    expect(seen).toEqual([
      "GET /api/agents/scout/chat/history?episode=ep-1",
      "GET /api/agents/atlas/chat/history?episode=ep-1",
    ]);
  });

  it("keeps a model list looked up through one agent from answering another's", async () => {
    const seen: string[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn((input: string, init?: RequestInit) => {
        seen.push(`${init?.method ?? "GET"} ${input}`);
        return Promise.resolve(
          new Response(JSON.stringify({ models: [{ id: "m", name: "m" }] }), { status: 200 }),
        );
      }),
    );
    await fetchModels("scout", "anthropic", "key", "https://api.test");
    await fetchModels("atlas", "anthropic", "key", "https://api.test");
    await fetchModels("scout", "anthropic", "key", "https://api.test");
    expect(seen).toEqual([
      "POST /api/agents/scout/providers/models",
      "POST /api/agents/atlas/providers/models",
    ]);
  });
});

describe("stores address the agent they hold", () => {
  it("loads the scheduled view from whichever agent it was last reset to", async () => {
    const seen = recordRequests();
    scheduled.reset("scout");
    await scheduled.load();
    scheduled.reset("atlas");
    await scheduled.load();
    expect(seen.sort()).toEqual([
      "GET /api/agents/atlas/scheduled/actions",
      "GET /api/agents/atlas/scheduled/pulses",
      "GET /api/agents/scout/scheduled/actions",
      "GET /api/agents/scout/scheduled/pulses",
    ]);
  });

  it("makes no scheduled request before an agent is bound", async () => {
    const seen = recordRequests();
    scheduled.reset(null);
    await scheduled.load();
    expect(seen).toEqual([]);
  });

  it("polls and acts on the inbox of whichever agent it was last reset to", async () => {
    const seen = recordRequests();
    userInbox.reset("scout");
    await userInbox.refresh();
    userInbox.reset("atlas");
    await userInbox.refresh();
    await userInbox.markRead("i1");
    await userInbox.refreshArchive();
    expect(seen).toEqual([
      "GET /api/agents/scout/inbox",
      "GET /api/agents/atlas/inbox",
      "PUT /api/agents/atlas/inbox/i1/read",
      "GET /api/agents/atlas/inbox/archive",
    ]);
  });

  it("lists sessions and outbound tasks for the agent it was made for", async () => {
    for (const agent of AGENTS) {
      const seen = recordRequests();
      const store = new SessionsStore({ agent, send: () => {}, pushToMain: () => {} });
      await store.refresh();
      expect(seen.length).toBeGreaterThan(0);
      for (const request of seen) {
        expect(request).toMatch(new RegExp(`^GET /api/agents/${agent}/(sessions|a2a/outbound)`));
      }
    }
  });

  it("loads a run's transcript from the agent the view was made for", async () => {
    for (const agent of AGENTS) {
      const seen = recordRequests();
      const view = new SessionView("run-1", null, agent);
      await view.load();
      expect(seen).toEqual([`GET /api/agents/${agent}/sessions/runs/run-1/transcript`]);
    }
  });
});
