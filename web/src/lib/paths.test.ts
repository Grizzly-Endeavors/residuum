import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  agentPath,
  agentWsUrl,
  hubPath,
  hubWsUrl,
  NoAgentSelectedError,
  onCurrentAgentChange,
  readLastAgent,
  rememberLastAgent,
  scopeApiPath,
  setCurrentAgent,
  teamPath,
} from "./paths";
import * as api from "./api";

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

beforeEach(() => {
  stubStorage();
  setCurrentAgent("scout");
  vi.stubGlobal("location", { protocol: "http:", host: "localhost:7700" });
});

afterEach(() => {
  setCurrentAgent(null);
  vi.unstubAllGlobals();
});

describe("base path helpers", () => {
  it("scopes agent calls to the current agent", () => {
    expect(agentPath("/status")).toBe("/api/agents/scout/status");
  });

  it("follows the current agent when it changes", () => {
    setCurrentAgent("atlas");
    expect(agentPath("/status")).toBe("/api/agents/atlas/status");
  });

  it("addresses another agent when given one", () => {
    expect(agentPath("/inbox", "atlas")).toBe("/api/agents/atlas/inbox");
  });

  it("encodes the agent name", () => {
    expect(agentPath("/status", "a b/c")).toBe("/api/agents/a%20b%2Fc/status");
  });

  it("refuses an agent call with no agent chosen", () => {
    setCurrentAgent(null);
    expect(() => agentPath("/status")).toThrow(NoAgentSelectedError);
  });

  it("builds hub and team paths", () => {
    expect(hubPath("/agents")).toBe("/api/hub/agents");
    expect(teamPath("/workbench/info")).toBe("/api/team/workbench/info");
  });

  it("builds the WebSocket URLs", () => {
    expect(agentWsUrl("scout")).toBe("ws://localhost:7700/api/agents/scout/ws");
    expect(hubWsUrl()).toBe("ws://localhost:7700/api/hub/ws");
  });

  it("uses wss over https", () => {
    vi.stubGlobal("location", { protocol: "https:", host: "example.test" });
    expect(agentWsUrl("scout")).toBe("wss://example.test/api/agents/scout/ws");
  });
});

describe("current agent", () => {
  it("tells listeners when it changes, and not when it stays", () => {
    const seen: (string | null)[] = [];
    const stop = onCurrentAgentChange((name) => {
      seen.push(name);
    });
    setCurrentAgent("scout");
    setCurrentAgent("atlas");
    setCurrentAgent(null);
    stop();
    setCurrentAgent("scout");
    expect(seen).toEqual(["atlas", null]);
  });

  it("remembers the last-used agent", () => {
    rememberLastAgent("atlas");
    expect(readLastAgent()).toBe("atlas");
  });

  it("survives unavailable storage", () => {
    vi.stubGlobal("localStorage", {
      getItem: () => {
        throw new Error("blocked");
      },
      setItem: () => {
        throw new Error("blocked");
      },
    });
    expect(() => {
      rememberLastAgent("atlas");
    }).not.toThrow();
    expect(readLastAgent()).toBeNull();
  });
});

// One row per line of the contract's route placement table.
describe("scopeApiPath places every contract row", () => {
  it.each([
    // Hub-level
    ["/api/hub/config/raw", "/api/hub/config/raw"],
    ["/api/hub/config/patch", "/api/hub/config/patch"],
    ["/api/hub/config/validate", "/api/hub/config/validate"],
    ["/api/secrets", "/api/hub/secrets"],
    ["/api/secrets/openai", "/api/hub/secrets/openai"],
    ["/api/agent-keys", "/api/hub/agent-keys"],
    ["/api/agent-keys/laptop", "/api/hub/agent-keys/laptop"],
    ["/api/a2a/keys", "/api/hub/a2a/keys"],
    ["/api/a2a/keys/peer", "/api/hub/a2a/keys/peer"],
    ["/api/cloud/status", "/api/hub/cloud/status"],
    ["/api/cloud/disconnect", "/api/hub/cloud/disconnect"],
    ["/api/update/check", "/api/hub/update/check"],
    ["/api/update/status", "/api/hub/update/status"],
    ["/api/update/apply", "/api/hub/update/apply"],
    ["/api/update/restart", "/api/hub/update/restart"],
    ["/api/tracing/status", "/api/hub/tracing/status"],
    ["/api/tracing/bug-report", "/api/hub/tracing/bug-report"],
    ["/api/shutdown", "/api/hub/shutdown"],
    ["/api/system/timezone", "/api/hub/system/timezone"],
    ["/api/mcp-catalog", "/api/hub/mcp-catalog"],
    ["/api/checkpoints?repo=hub", "/api/hub/checkpoints?repo=hub"],
    ["/api/checkpoints?repo=team&limit=5", "/api/hub/checkpoints?repo=team&limit=5"],
    ["/api/checkpoints/abc/diff?repo=hub&path=x", "/api/hub/checkpoints/abc/diff?repo=hub&path=x"],
    ["/api/hub/agents", "/api/hub/agents"],
    ["/api/hub/status", "/api/hub/status"],
    // Team-level
    ["/api/workbench/info", "/api/team/workbench/info"],
    ["/api/workbench/artifacts", "/api/team/workbench/artifacts"],
    ["/api/workbench/artifacts/chart", "/api/team/workbench/artifacts/chart"],
    ["/api/team/workspace/files", "/api/team/workspace/files"],
    // Agent-level
    ["/api/status", "/api/agents/scout/status"],
    ["/api/config/raw", "/api/agents/scout/config/raw"],
    ["/api/config/complete-setup", "/api/agents/scout/config/complete-setup"],
    ["/api/providers/models", "/api/agents/scout/providers/models"],
    ["/api/mcp/raw", "/api/agents/scout/mcp/raw"],
    ["/api/chat/history?episode=ep-1", "/api/agents/scout/chat/history?episode=ep-1"],
    ["/api/usage", "/api/agents/scout/usage"],
    ["/api/sessions?category=spawned", "/api/agents/scout/sessions?category=spawned"],
    ["/api/sessions/runs/r1/transcript", "/api/agents/scout/sessions/runs/r1/transcript"],
    ["/api/scheduled/pulses", "/api/agents/scout/scheduled/pulses"],
    ["/api/inbox", "/api/agents/scout/inbox"],
    ["/api/inbox/archive", "/api/agents/scout/inbox/archive"],
    ["/api/agent-inbox", "/api/agents/scout/agent-inbox"],
    ["/api/files/workspace?path=a.png", "/api/agents/scout/files/workspace?path=a.png"],
    ["/api/memory/search", "/api/agents/scout/memory/search"],
    ["/api/model/complete", "/api/agents/scout/model/complete"],
    [
      "/api/workspace/file?path=team/notes.md",
      "/api/agents/scout/workspace/file?path=team/notes.md",
    ],
    ["/api/checkpoints?repo=workspace", "/api/agents/scout/checkpoints?repo=workspace"],
    [
      "/api/checkpoints/abc/restore?repo=agent_config",
      "/api/agents/scout/checkpoints/abc/restore?repo=agent_config",
    ],
    ["/api/a2a/agents", "/api/agents/scout/a2a/agents"],
    ["/api/a2a/agents/raw", "/api/agents/scout/a2a/agents/raw"],
    ["/api/a2a/status", "/api/agents/scout/a2a/status"],
    ["/api/a2a/card", "/api/agents/scout/a2a/card"],
    ["/api/a2a/outbound", "/api/agents/scout/a2a/outbound"],
    ["/api/agents/atlas/status", "/api/agents/atlas/status"],
  ])("%s -> %s", (legacy, scoped) => {
    expect(scopeApiPath(legacy)).toBe(scoped);
  });

  it("does not mistake a longer name for a hub prefix", () => {
    expect(scopeApiPath("/api/secrets-report")).toBe("/api/agents/scout/secrets-report");
  });
});

/** Run `call` against a stubbed fetch and return the "METHOD url" it made. */
async function requestOf(call: () => Promise<unknown>): Promise<string> {
  const fetchMock = vi.fn((_input: string, init?: RequestInit) => {
    void init;
    return Promise.resolve(
      new Response(JSON.stringify({ agents: [], diff: null, checkpoint_id: null }), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      }),
    );
  });
  vi.stubGlobal("fetch", fetchMock);
  await call().catch(() => undefined);
  const [url, init] = fetchMock.mock.calls[0] ?? [];
  return `${init?.method ?? "GET"} ${url}`;
}

// Every API function lands on the scope the contract puts its route in.
describe("api functions use the contract paths", () => {
  it.each<[string, () => Promise<unknown>, string]>([
    ["fetchStatus", () => api.fetchStatus(), "GET /api/agents/scout/status"],
    ["fetchChatHistory", () => api.fetchChatHistory(), "GET /api/agents/scout/chat/history"],
    ["fetchUsageTotals", () => api.fetchUsageTotals(), "GET /api/agents/scout/usage"],
    [
      "fetchChatSegment",
      () => api.fetchChatSegment("ep-1"),
      "GET /api/agents/scout/chat/history?episode=ep-1",
    ],
    [
      "submitBugReport",
      () => api.submitBugReport({ severity: "broken", message: "x" } as never),
      "POST /api/hub/tracing/bug-report",
    ],
    [
      "fetchProviderModels",
      () => api.fetchProviderModels({} as never),
      "POST /api/agents/scout/providers/models",
    ],
    ["storeSecret", () => api.storeSecret("n", "v"), "POST /api/hub/secrets"],
    ["listSecrets", () => api.listSecrets(), "GET /api/hub/secrets"],
    ["deleteSecret", () => api.deleteSecret("n"), "DELETE /api/hub/secrets/n"],
    ["fetchConfigRaw", () => api.fetchConfigRaw(), "GET /api/agents/scout/config/raw"],
    ["putConfigRaw", () => api.putConfigRaw("x"), "PUT /api/agents/scout/config/raw"],
    ["patchConfig", () => api.patchConfig({ a: 1 }), "PATCH /api/agents/scout/config/patch"],
    ["validateConfig", () => api.validateConfig("x"), "POST /api/agents/scout/config/validate"],
    ["fetchHubConfigRaw", () => api.fetchHubConfigRaw(), "GET /api/hub/config/raw"],
    ["putHubConfigRaw", () => api.putHubConfigRaw("x"), "PUT /api/hub/config/raw"],
    ["patchHubConfig", () => api.patchHubConfig({ a: 1 }), "PATCH /api/hub/config/patch"],
    ["validateHubConfig", () => api.validateHubConfig("x"), "POST /api/hub/config/validate"],
    ["fetchProvidersRaw", () => api.fetchProvidersRaw(), "GET /api/agents/scout/providers/raw"],
    ["putProvidersRaw", () => api.putProvidersRaw("x"), "PUT /api/agents/scout/providers/raw"],
    [
      "patchProviders",
      () => api.patchProviders({ a: 1 }),
      "PATCH /api/agents/scout/providers/patch",
    ],
    [
      "validateProviders",
      () => api.validateProviders("x"),
      "POST /api/agents/scout/providers/validate",
    ],
    ["fetchMcpRaw", () => api.fetchMcpRaw(), "GET /api/agents/scout/mcp/raw"],
    ["putMcpRaw", () => api.putMcpRaw("{}"), "PUT /api/agents/scout/mcp/raw"],
    ["patchMcp", () => api.patchMcp({ a: 1 }), "PATCH /api/agents/scout/mcp/patch"],
    ["fetchAgentKeys", () => api.fetchAgentKeys(), "GET /api/hub/agent-keys"],
    ["deleteAgentKey", () => api.deleteAgentKey("k"), "DELETE /api/hub/agent-keys/k"],
    ["fetchA2aStatus", () => api.fetchA2aStatus(), "GET /api/agents/scout/a2a/status"],
    ["fetchA2aCard", () => api.fetchA2aCard(), "GET /api/agents/scout/a2a/card"],
    ["fetchA2aKeys", () => api.fetchA2aKeys(), "GET /api/hub/a2a/keys"],
    ["revokeA2aKey", () => api.revokeA2aKey("k"), "DELETE /api/hub/a2a/keys/k"],
    ["fetchA2aAgents", () => api.fetchA2aAgents(), "GET /api/agents/scout/a2a/agents"],
    ["fetchA2aAgentsRaw", () => api.fetchA2aAgentsRaw(), "GET /api/agents/scout/a2a/agents/raw"],
    ["putA2aAgentsRaw", () => api.putA2aAgentsRaw("[]"), "PUT /api/agents/scout/a2a/agents/raw"],
    [
      "fetchOutboundA2aTasks",
      () => api.fetchOutboundA2aTasks(),
      "GET /api/agents/scout/a2a/outbound",
    ],
    [
      "markUserInboxItemRead",
      () => api.markUserInboxItemRead("i"),
      "PUT /api/agents/scout/inbox/i/read",
    ],
    [
      "archiveUserInboxItem",
      () => api.archiveUserInboxItem("i"),
      "POST /api/agents/scout/inbox/i/archive",
    ],
    [
      "fetchArchivedUserInbox",
      () => api.fetchArchivedUserInbox(),
      "GET /api/agents/scout/inbox/archive",
    ],
    [
      "restoreUserInboxItem",
      () => api.restoreUserInboxItem("i"),
      "POST /api/agents/scout/inbox/i/restore",
    ],
    [
      "fetchSessions",
      () => api.fetchSessions({ limit: 5 }),
      "GET /api/agents/scout/sessions?limit=5",
    ],
    [
      "fetchSessionTranscript",
      () => api.fetchSessionTranscript("r1"),
      "GET /api/agents/scout/sessions/runs/r1/transcript",
    ],
    [
      "fetchScheduledPulses",
      () => api.fetchScheduledPulses(),
      "GET /api/agents/scout/scheduled/pulses",
    ],
    [
      "setPulseEnabled",
      () => api.setPulseEnabled("p", true),
      "PUT /api/agents/scout/scheduled/pulses/p/enabled",
    ],
    [
      "fetchScheduledActions",
      () => api.fetchScheduledActions(),
      "GET /api/agents/scout/scheduled/actions",
    ],
    [
      "cancelScheduledAction",
      () => api.cancelScheduledAction("a"),
      "DELETE /api/agents/scout/scheduled/actions/a",
    ],
    [
      "fetchWorkbenchArtifacts",
      () => api.fetchWorkbenchArtifacts(),
      "GET /api/team/workbench/artifacts",
    ],
    ["fetchWorkbenchInfo", () => api.fetchWorkbenchInfo(), "GET /api/team/workbench/info"],
    [
      "deleteWorkbenchArtifact",
      () => api.deleteWorkbenchArtifact("c"),
      "DELETE /api/team/workbench/artifacts/c",
    ],
    [
      "fetchWorkspaceFiles",
      () => api.fetchWorkspaceFiles("wiki"),
      "GET /api/agents/scout/workspace/files?path=wiki",
    ],
    [
      "fetchWorkspaceFiles (team)",
      () => api.fetchWorkspaceFiles("wiki", "team"),
      "GET /api/team/workspace/files?path=wiki",
    ],
    [
      "fetchWorkspaceFile",
      () => api.fetchWorkspaceFile("a.md"),
      "GET /api/agents/scout/workspace/file?path=a.md",
    ],
    [
      "fetchWorkspaceFile (team)",
      () => api.fetchWorkspaceFile("a.md", "team"),
      "GET /api/team/workspace/file?path=a.md",
    ],
    [
      "putWorkspaceFile",
      () => api.putWorkspaceFile("a.md", "x", null),
      "PUT /api/agents/scout/workspace/file",
    ],
    [
      "putWorkspaceFile (team)",
      () => api.putWorkspaceFile("a.md", "x", null, "team"),
      "PUT /api/team/workspace/file",
    ],
    [
      "validateWorkspaceFile",
      () => api.validateWorkspaceFile("a", "x"),
      "POST /api/agents/scout/workspace/validate",
    ],
    [
      "deleteWorkspaceFile",
      () => api.deleteWorkspaceFile("a.md"),
      "DELETE /api/agents/scout/workspace/file?path=a.md",
    ],
    [
      "deleteWorkspaceFile (team)",
      () => api.deleteWorkspaceFile("a.md", "team"),
      "DELETE /api/team/workspace/file?path=a.md",
    ],
    [
      "moveWorkspaceFile",
      () => api.moveWorkspaceFile("a", "b"),
      "POST /api/agents/scout/workspace/move",
    ],
    [
      "moveWorkspaceFile (team)",
      () => api.moveWorkspaceFile("a", "b", false, "team"),
      "POST /api/team/workspace/move",
    ],
    [
      "fetchCheckpoints (workspace)",
      () => api.fetchCheckpoints({ repo: "workspace" }),
      "GET /api/agents/scout/checkpoints?repo=workspace",
    ],
    [
      "fetchCheckpoints (agent_config)",
      () => api.fetchCheckpoints({ repo: "agent_config" }),
      "GET /api/agents/scout/checkpoints?repo=agent_config",
    ],
    [
      "fetchCheckpoints (hub)",
      () => api.fetchCheckpoints({ repo: "hub" }),
      "GET /api/hub/checkpoints?repo=hub",
    ],
    [
      "fetchCheckpoints (team)",
      () => api.fetchCheckpoints({ repo: "team" }),
      "GET /api/hub/checkpoints?repo=team",
    ],
    [
      "fetchCheckpointStats",
      () => api.fetchCheckpointStats("workspace"),
      "GET /api/agents/scout/checkpoints/stats?repo=workspace",
    ],
    [
      "fetchCheckpointStats (team)",
      () => api.fetchCheckpointStats("team"),
      "GET /api/hub/checkpoints/stats?repo=team",
    ],
    [
      "fetchCheckpointDetail",
      () => api.fetchCheckpointDetail("c1", "hub"),
      "GET /api/hub/checkpoints/c1?repo=hub",
    ],
    [
      "fetchCheckpointDiff",
      () => api.fetchCheckpointDiff("c1", "workspace", "a.md"),
      "GET /api/agents/scout/checkpoints/c1/diff?repo=workspace&path=a.md",
    ],
    [
      "fetchCheckpointFile",
      () => api.fetchCheckpointFile("c1", "team", "a.md"),
      "GET /api/hub/checkpoints/c1/file?repo=team&path=a.md",
    ],
    [
      "restoreCheckpoint",
      () => api.restoreCheckpoint("c1", "agent_config", "a.md"),
      "POST /api/agents/scout/checkpoints/c1/restore",
    ],
    ["undoCheckpoint", () => api.undoCheckpoint("c1", "hub"), "POST /api/hub/checkpoints/c1/undo"],
    ["fetchCloudStatus", () => api.fetchCloudStatus(), "GET /api/hub/cloud/status"],
    ["disconnectCloud", () => api.disconnectCloud(), "POST /api/hub/cloud/disconnect"],
    ["fetchUpdateStatus", () => api.fetchUpdateStatus(), "GET /api/hub/update/status"],
    ["triggerUpdateCheck", () => api.triggerUpdateCheck(), "POST /api/hub/update/check"],
    ["applyUpdate", () => api.applyUpdate(), "POST /api/hub/update/apply"],
    ["fetchTimezone", () => api.fetchTimezone(), "GET /api/hub/system/timezone"],
    ["fetchMcpCatalog", () => api.fetchMcpCatalog(), "GET /api/hub/mcp-catalog"],
    ["fetchAgents", () => api.fetchAgents(), "GET /api/hub/agents"],
    ["fetchHubStatus", () => api.fetchHubStatus(), "GET /api/hub/status"],
    [
      "createAgent",
      () => api.createAgent({ name: "atlas", models_from: "scout" }),
      "POST /api/hub/agents",
    ],
    ["deleteAgent", () => api.deleteAgent("atlas"), "DELETE /api/hub/agents/atlas"],
    ["startAgent", () => api.startAgent("atlas"), "POST /api/hub/agents/atlas/start"],
    ["stopAgent", () => api.stopAgent("atlas"), "POST /api/hub/agents/atlas/stop"],
    ["restartAgent", () => api.restartAgent("atlas"), "POST /api/hub/agents/atlas/restart"],
    [
      "setAgentAutostart",
      () => api.setAgentAutostart("atlas", true),
      "PATCH /api/hub/agents/atlas",
    ],
  ])("%s", async (_name, call, expected) => {
    // The fetch cache is keyed by request and would answer a repeat call.
    localStorage.clear();
    expect(await requestOf(call)).toBe(expected);
  });

  it("lists provider models under the hub before any agent exists (onboarding)", async () => {
    setCurrentAgent(null);
    expect(await requestOf(() => api.fetchProviderModels("anthropic"))).toBe(
      "POST /api/hub/providers/models",
    );
  });

  it("keys cached agent-scoped reads by agent", () => {
    expect(api.cacheKeyConfigRaw()).toBe("GET /api/agents/scout/config/raw");
    setCurrentAgent("atlas");
    expect(api.cacheKeyConfigRaw()).toBe("GET /api/agents/atlas/config/raw");
  });
});
