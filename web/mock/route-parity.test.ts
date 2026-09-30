import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type * as ApiModule from "../src/lib/api";
import type { RepoKind } from "../src/lib/generated/protocol";
import type { WorkspaceScope } from "../src/lib/hub-types";
import * as api from "../src/lib/api";
import { agentWsUrl, hubWsUrl, scopeApiPath } from "../src/lib/paths";
import { apiRoutes } from "./api-routes";
import { matchRoute, type Route } from "./routes";
import { isRefusal, scopeRequest } from "./scope";
import { HUB_SOCKET_PATH, agentSocketPath } from "./sockets";
import { createStubHub } from "./test-support";

/**
 * Every request the API client can make has to land on a route the mock
 * serves. The test calls each exported client function with sample arguments
 * against a `fetch` that records what it is asked for, and looks up each
 * recorded method and path the way the mock's own handler would: scoped to an
 * agent, the hub or the team, then matched against `apiRoutes`.
 */

type Api = typeof ApiModule;

/** What a sample call may do: call the client, and choose the agent its scoped calls address. */
type Sample = (client: Api, setAgent: (name: string | null) => void) => Promise<unknown>;

/** The client's exports that only work on values the caller has, and make no requests. */
const MAKES_NO_REQUESTS = [
  "validationFromApiError",
  "workspaceConflictFromApiError",
  "parseWorkspaceCheckpoints",
  "cacheKeyStatus",
  "cacheKeyConfigRaw",
  "cacheKeyProvidersRaw",
  "cacheKeyMcpRaw",
  "cacheKeyA2aAgentsRaw",
] as const;

type RequestFunction = Exclude<
  {
    [K in keyof Api]: Api[K] extends (...args: never[]) => unknown ? K : never;
  }[keyof Api],
  (typeof MAKES_NO_REQUESTS)[number]
>;

const SCOPES: readonly WorkspaceScope[] = ["agent", "team"];
const REPOS: readonly RepoKind[] = ["workspace", "agent_config", "team", "hub"];

const SAMPLES: Record<RequestFunction, Sample[]> = {
  fetchStatus: [(a) => a.fetchStatus()],
  fetchChatHistory: [(a) => a.fetchChatHistory()],
  fetchUsageTotals: [(a) => a.fetchUsageTotals()],
  fetchChatSegment: [(a) => a.fetchChatSegment("ep-003")],
  submitBugReport: [
    (a) =>
      a.submitBugReport({
        what_happened: "it broke",
        what_expected: "it works",
        what_doing: "clicking",
        severity: "broken",
      }),
  ],
  submitFeedback: [(a) => a.submitFeedback({ message: "hello", category: "idea" })],
  fetchTimezone: [(a) => a.fetchTimezone()],
  fetchProviderModels: [
    (a) => a.fetchProviderModels("openai", "key", "http://localhost"),
    // Onboarding lists models before any agent exists.
    (a, setAgent) => {
      setAgent(null);
      return a.fetchProviderModels("openai");
    },
  ],
  fetchMcpCatalogOrThrow: [(a) => a.fetchMcpCatalogOrThrow()],
  fetchMcpCatalog: [(a) => a.fetchMcpCatalog()],
  storeSecret: [(a) => a.storeSecret("name", "value")],
  completeSetup: [
    (a) =>
      a.completeSetup({
        hubConfig: "",
        agentName: "first",
        userName: "Bear",
        config: "",
        providers: "",
        mcpJson: "{}",
      }),
  ],
  fetchConfigRaw: [(a) => a.fetchConfigRaw()],
  putConfigRaw: [(a) => a.putConfigRaw("a = 1")],
  patchConfig: [(a) => a.patchConfig({ a: 1 })],
  validateConfig: [(a) => a.validateConfig("a = 1")],
  fetchHubConfigRaw: [(a) => a.fetchHubConfigRaw()],
  putHubConfigRaw: [(a) => a.putHubConfigRaw("a = 1")],
  patchHubConfig: [(a) => a.patchHubConfig({ a: 1 })],
  validateHubConfig: [(a) => a.validateHubConfig("a = 1")],
  fetchProvidersRaw: [(a) => a.fetchProvidersRaw()],
  putProvidersRaw: [(a) => a.putProvidersRaw("a = 1")],
  patchProviders: [(a) => a.patchProviders({ a: 1 })],
  validateProviders: [(a) => a.validateProviders("a = 1")],
  fetchMcpRaw: [(a) => a.fetchMcpRaw()],
  putMcpRaw: [(a) => a.putMcpRaw("{}")],
  patchMcp: [(a) => a.patchMcp({ a: 1 })],
  fetchSecretNames: [(a) => a.fetchSecretNames()],
  listSecrets: [(a) => a.listSecrets()],
  deleteSecret: [(a) => a.deleteSecret("name")],
  fetchAgentKeys: [(a) => a.fetchAgentKeys()],
  storeAgentKey: [(a) => a.storeAgentKey("name", "value", "what it is for")],
  deleteAgentKey: [(a) => a.deleteAgentKey("name")],
  fetchA2aStatus: [(a) => a.fetchA2aStatus()],
  fetchA2aCard: [(a) => a.fetchA2aCard()],
  fetchA2aKeys: [(a) => a.fetchA2aKeys()],
  createA2aKey: [(a) => a.createA2aKey("laptop", "my laptop")],
  revokeA2aKey: [(a) => a.revokeA2aKey("laptop")],
  fetchA2aAgents: [(a) => a.fetchA2aAgents()],
  markUserInboxItemRead: [(a) => a.markUserInboxItemRead("item-1")],
  archiveUserInboxItem: [(a) => a.archiveUserInboxItem("item-1")],
  fetchArchivedUserInbox: [(a) => a.fetchArchivedUserInbox()],
  restoreUserInboxItem: [(a) => a.restoreUserInboxItem("item-1")],
  fetchOutboundA2aTasks: [(a) => a.fetchOutboundA2aTasks()],
  stopOutboundA2aTask: [(a) => a.stopOutboundA2aTask("task-1")],
  stopWatchingOutboundA2aTask: [(a) => a.stopWatchingOutboundA2aTask("task-1")],
  fetchA2aAgentsRaw: [(a) => a.fetchA2aAgentsRaw()],
  putA2aAgentsRaw: [(a) => a.putA2aAgentsRaw('{"agents":{}}')],
  fetchSessions: [
    (a) =>
      a.fetchSessions({
        category: "spawned",
        before: "run-1",
        limit: 10,
        address: "spawned-1",
        artifact: "tip-splitter",
      }),
  ],
  fetchSessionTranscript: [(a) => a.fetchSessionTranscript("run-1")],
  fetchScheduledPulses: [(a) => a.fetchScheduledPulses()],
  setPulseEnabled: [(a) => a.setPulseEnabled("inbox_check", false)],
  fetchScheduledActions: [(a) => a.fetchScheduledActions()],
  cancelScheduledAction: [(a) => a.cancelScheduledAction("act-1")],
  fetchWorkbenchArtifacts: [(a) => a.fetchWorkbenchArtifacts()],
  fetchWorkbenchInfo: [(a) => a.fetchWorkbenchInfo()],
  deleteWorkbenchArtifact: [(a) => a.deleteWorkbenchArtifact("tip-splitter")],
  fetchWorkspaceFiles: SCOPES.map(
    (scope): Sample =>
      (a) =>
        a.fetchWorkspaceFiles("notes", scope),
  ),
  fetchWorkspaceFile: SCOPES.map(
    (scope): Sample =>
      (a) =>
        a.fetchWorkspaceFile("a.md", scope),
  ),
  putWorkspaceFile: SCOPES.map(
    (scope): Sample =>
      (a) =>
        a.putWorkspaceFile("a.md", "text", "v1", scope),
  ),
  validateWorkspaceFile: SCOPES.map(
    (scope): Sample =>
      (a) =>
        a.validateWorkspaceFile("a.md", "text", scope),
  ),
  deleteWorkspaceFile: SCOPES.map(
    (scope): Sample =>
      (a) =>
        a.deleteWorkspaceFile("a.md", scope),
  ),
  moveWorkspaceFile: SCOPES.map(
    (scope): Sample =>
      (a) =>
        a.moveWorkspaceFile("a.md", "b.md", true, scope),
  ),
  fetchCheckpoints: REPOS.map(
    (repo): Sample =>
      (a) =>
        a.fetchCheckpoints({ repo, path: "a.md", turnId: "turn-1", before: "abc", limit: 5 }),
  ),
  fetchCheckpointStats: REPOS.map(
    (repo): Sample =>
      (a) =>
        a.fetchCheckpointStats(repo),
  ),
  fetchCheckpointDetail: REPOS.map(
    (repo): Sample =>
      (a) =>
        a.fetchCheckpointDetail("abc", repo),
  ),
  fetchCheckpointDiff: REPOS.map(
    (repo): Sample =>
      (a) =>
        a.fetchCheckpointDiff("abc", repo, "a.md"),
  ),
  fetchCheckpointFile: REPOS.map(
    (repo): Sample =>
      (a) =>
        a.fetchCheckpointFile("abc", repo, "a.md"),
  ),
  restoreCheckpoint: REPOS.map(
    (repo): Sample =>
      (a) =>
        a.restoreCheckpoint("abc", repo, "a.md"),
  ),
  undoCheckpoint: REPOS.map(
    (repo): Sample =>
      (a) =>
        a.undoCheckpoint("abc", repo),
  ),
  undoLastAction: REPOS.map(
    (repo): Sample =>
      (a) =>
        a.undoLastAction("abc", repo, "a.md"),
  ),
  fetchCloudStatus: [(a) => a.fetchCloudStatus()],
  disconnectCloud: [(a) => a.disconnectCloud()],
  fetchUpdateStatus: [(a) => a.fetchUpdateStatus()],
  triggerUpdateCheck: [(a) => a.triggerUpdateCheck()],
  applyUpdate: [(a) => a.applyUpdate()],
  fetchAgents: [(a) => a.fetchAgents()],
  fetchHubStatus: [(a) => a.fetchHubStatus()],
  createAgent: [
    (a) =>
      a.createAgent({
        name: "newbie",
        description: "a new agent",
        models_from: "scout",
        providers_toml: null,
        a2a_visibility: "private",
      }),
  ],
  deleteAgent: [(a) => a.deleteAgent("scout")],
  fetchDeletedAgents: [(a) => a.fetchDeletedAgents()],
  restoreAgent: [(a) => a.restoreAgent("scout", "ckpt-1")],
  startAgent: [(a) => a.startAgent("drifter")],
  stopAgent: [(a) => a.stopAgent("scout")],
  restartAgent: [(a) => a.restartAgent("scout")],
  setAgentVisibility: [(a) => a.setAgentVisibility("scout", "public")],
  setAgentAutostart: [(a) => a.setAgentAutostart("scout", false)],
};

/**
 * What workbench artifacts can reach through the bridge, in the unscoped
 * spelling artifact authors use (`scopeApiPath` scopes it). The client doesn't
 * call these, so they are listed here.
 */
const ARTIFACT_CALLS: ReadonlyArray<readonly [string, string]> = [
  ["POST", "/api/agent-inbox"],
  ["POST", "/api/model/complete"],
  ["GET", "/api/sessions"],
  ["POST", "/api/sessions"],
  ["POST", "/api/sessions/spawned-1/stop"],
  ["POST", "/api/sessions/spawned-1/messages"],
  ["GET", "/api/workspace/files?path=a"],
  ["GET", "/api/workspace/file?path=a.md"],
  ["PUT", "/api/workspace/file"],
  ["DELETE", "/api/workspace/file?path=a.md"],
  ["GET", "/api/workspace/raw?path=a.md"],
  ["PUT", "/api/workspace/raw?path=a.md"],
  ["POST", "/api/workspace/dir"],
  ["POST", "/api/workspace/move"],
  ["GET", "/api/workspace/tree?path=a"],
  ["POST", "/api/workspace/read"],
  ["GET", "/api/workbench/artifacts"],
];

interface Request {
  method: string;
  url: string;
}

/** The problem with a request the mock would not serve, or `null` when it would. */
function problemWith(routes: readonly Route[], { method, url }: Request): string | null {
  const hub = createStubHub();
  hub.createAgent("atlas");
  const { pathname, searchParams } = new URL(url, "http://mock.invalid");
  const scoped = scopeRequest(hub, pathname, searchParams);
  if (isRefusal(scoped)) return `refused ${String(scoped.status)}: ${scoped.body.error}`;
  return matchRoute(routes, method, scoped.path) === undefined
    ? `no route for ${method} ${scoped.path}`
    : null;
}

/** Call a sample against a fetch that records its requests, with a fresh client so nothing is cached. */
async function requestsOf(sample: Sample): Promise<Request[]> {
  const recorded: Request[] = [];
  vi.stubGlobal("fetch", (input: RequestInfo | URL, init?: RequestInit) => {
    const url = typeof input === "string" || input instanceof URL ? input.toString() : input.url;
    recorded.push({ method: (init?.method ?? "GET").toUpperCase(), url });
    return Promise.resolve(new Response("{}", { status: 200 }));
  });
  vi.resetModules();
  const client = await import("../src/lib/api");
  const paths = await import("../src/lib/paths");
  paths.setCurrentAgent("atlas");
  try {
    await sample(client, paths.setCurrentAgent);
  } catch {
    // The empty answers the stub gives may not be what the client does next, but the request it made is already recorded.
  }
  return recorded;
}

/** Every request the client can make that `routes` would not serve, with the function that makes it. */
async function unservedBy(routes: readonly Route[]): Promise<string[]> {
  const unserved: string[] = [];
  for (const [name, samples] of Object.entries(SAMPLES)) {
    for (const sample of samples) {
      for (const request of await requestsOf(sample)) {
        const problem = problemWith(routes, request);
        if (problem !== null)
          unserved.push(`${name}: ${request.method} ${request.url} (${problem})`);
      }
    }
  }
  return unserved;
}

describe("route parity between the API client and the mock", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("has a sample call for every function the client exports", () => {
    const exported = Object.entries(api)
      .filter(([name, value]) => typeof value === "function" && name !== "ApiError")
      .map(([name]) => name)
      .sort();
    expect(exported).toEqual([...Object.keys(SAMPLES), ...MAKES_NO_REQUESTS].sort());
  });

  it("makes every sample call reach fetch", async () => {
    for (const [name, samples] of Object.entries(SAMPLES)) {
      for (const sample of samples) {
        expect(await requestsOf(sample), name).not.toHaveLength(0);
      }
    }
  });

  it("serves every request the client makes", async () => {
    expect(await unservedBy(apiRoutes)).toEqual([]);
  });

  it("fails when a route the client needs is removed from the mock", async () => {
    const withoutPulses = apiRoutes.filter((route) => route.pattern !== "/api/scheduled/pulses");
    expect(withoutPulses).toHaveLength(apiRoutes.length - 1);
    const unserved = await unservedBy(withoutPulses);
    expect(unserved).toEqual([
      "fetchScheduledPulses: GET /api/agents/atlas/scheduled/pulses (no route for GET /api/scheduled/pulses)",
    ]);
  });

  it("serves every route an artifact can reach through the bridge", () => {
    const unserved = ARTIFACT_CALLS.flatMap(([method, path]) => {
      const problem = problemWith(apiRoutes, { method, url: scopeApiPath(path, "atlas") });
      return problem === null ? [] : [`${method} ${path} (${problem})`];
    });
    expect(unserved).toEqual([]);
  });

  describe("sockets", () => {
    beforeEach(() => {
      vi.stubGlobal("location", { protocol: "http:", host: "localhost:5173" });
    });

    it("opens the hub and agent sockets where the mock accepts them", () => {
      expect(new URL(hubWsUrl()).pathname).toBe(HUB_SOCKET_PATH);
      expect(new URL(agentWsUrl("atlas")).pathname).toBe(agentSocketPath("atlas"));
    });
  });
});
