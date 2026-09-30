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

/** The agent every agent-scoped sample addresses, and the one the stub hub runs. */
const AGENT = "atlas";

/** A sample call: calls one client function with arguments it could really receive. */
type Sample = (client: Api) => Promise<unknown>;

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

/** The team scope is addressed without an agent; the agent scope needs one. */
function agentForScope(scope: WorkspaceScope): string | null {
  return scope === "team" ? null : AGENT;
}

/** The hub-level repositories are addressed without an agent; the agent-level ones need one. */
function agentForRepo(repo: RepoKind): string | null {
  return repo === "hub" || repo === "team" ? null : AGENT;
}

const SAMPLES: Record<RequestFunction, Sample[]> = {
  fetchStatus: [(a) => a.fetchStatus(AGENT)],
  fetchChatHistory: [(a) => a.fetchChatHistory(AGENT)],
  fetchUsageTotals: [(a) => a.fetchUsageTotals(AGENT)],
  fetchChatSegment: [(a) => a.fetchChatSegment(AGENT, "ep-003")],
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
    (a) => a.fetchProviderModels(AGENT, "openai", "key", "http://localhost"),
    // Onboarding lists models before any agent exists.
    (a) => a.fetchProviderModels(null, "openai"),
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
  fetchConfigRaw: [(a) => a.fetchConfigRaw(AGENT)],
  putConfigRaw: [(a) => a.putConfigRaw(AGENT, "a = 1")],
  patchConfig: [(a) => a.patchConfig(AGENT, { a: 1 })],
  validateConfig: [(a) => a.validateConfig(AGENT, "a = 1")],
  fetchHubConfigRaw: [(a) => a.fetchHubConfigRaw()],
  putHubConfigRaw: [(a) => a.putHubConfigRaw("a = 1")],
  patchHubConfig: [(a) => a.patchHubConfig({ a: 1 })],
  validateHubConfig: [(a) => a.validateHubConfig("a = 1")],
  fetchProvidersRaw: [(a) => a.fetchProvidersRaw(AGENT)],
  putProvidersRaw: [(a) => a.putProvidersRaw(AGENT, "a = 1")],
  patchProviders: [(a) => a.patchProviders(AGENT, { a: 1 })],
  validateProviders: [(a) => a.validateProviders(AGENT, "a = 1")],
  fetchMcpRaw: [(a) => a.fetchMcpRaw(AGENT)],
  putMcpRaw: [(a) => a.putMcpRaw(AGENT, "{}")],
  patchMcp: [(a) => a.patchMcp(AGENT, { a: 1 })],
  fetchSecretNames: [(a) => a.fetchSecretNames()],
  listSecrets: [(a) => a.listSecrets()],
  deleteSecret: [(a) => a.deleteSecret("name")],
  fetchAgentKeys: [(a) => a.fetchAgentKeys()],
  storeAgentKey: [(a) => a.storeAgentKey("name", "value", "what it is for")],
  deleteAgentKey: [(a) => a.deleteAgentKey("name")],
  fetchA2aStatus: [(a) => a.fetchA2aStatus(AGENT)],
  fetchA2aCard: [(a) => a.fetchA2aCard(AGENT)],
  fetchA2aKeys: [(a) => a.fetchA2aKeys()],
  createA2aKey: [(a) => a.createA2aKey("laptop", "my laptop")],
  revokeA2aKey: [(a) => a.revokeA2aKey("laptop")],
  fetchA2aAgents: [(a) => a.fetchA2aAgents(AGENT)],
  markUserInboxItemRead: [(a) => a.markUserInboxItemRead(AGENT, "item-1")],
  archiveUserInboxItem: [(a) => a.archiveUserInboxItem(AGENT, "item-1")],
  fetchArchivedUserInbox: [(a) => a.fetchArchivedUserInbox(AGENT)],
  restoreUserInboxItem: [(a) => a.restoreUserInboxItem(AGENT, "item-1")],
  fetchOutboundA2aTasks: [(a) => a.fetchOutboundA2aTasks(AGENT)],
  stopOutboundA2aTask: [(a) => a.stopOutboundA2aTask(AGENT, "task-1")],
  stopWatchingOutboundA2aTask: [(a) => a.stopWatchingOutboundA2aTask(AGENT, "task-1")],
  fetchA2aAgentsRaw: [(a) => a.fetchA2aAgentsRaw(AGENT)],
  putA2aAgentsRaw: [(a) => a.putA2aAgentsRaw(AGENT, '{"agents":{}}')],
  fetchSessions: [
    (a) =>
      a.fetchSessions(AGENT, {
        category: "spawned",
        before: "run-1",
        limit: 10,
        address: "spawned-1",
        artifact: "tip-splitter",
      }),
  ],
  fetchSessionTranscript: [(a) => a.fetchSessionTranscript(AGENT, "run-1")],
  fetchScheduledPulses: [(a) => a.fetchScheduledPulses(AGENT)],
  setPulseEnabled: [(a) => a.setPulseEnabled(AGENT, "inbox_check", false)],
  fetchScheduledActions: [(a) => a.fetchScheduledActions(AGENT)],
  cancelScheduledAction: [(a) => a.cancelScheduledAction(AGENT, "act-1")],
  fetchWorkbenchArtifacts: [(a) => a.fetchWorkbenchArtifacts()],
  fetchWorkbenchInfo: [(a) => a.fetchWorkbenchInfo()],
  deleteWorkbenchArtifact: [(a) => a.deleteWorkbenchArtifact("tip-splitter")],
  fetchWorkspaceFiles: SCOPES.map(
    (scope): Sample =>
      (a) =>
        a.fetchWorkspaceFiles(agentForScope(scope), "notes", scope),
  ),
  fetchWorkspaceFile: SCOPES.map(
    (scope): Sample =>
      (a) =>
        a.fetchWorkspaceFile(agentForScope(scope), "a.md", scope),
  ),
  putWorkspaceFile: SCOPES.map(
    (scope): Sample =>
      (a) =>
        a.putWorkspaceFile(agentForScope(scope), "a.md", "text", "v1", scope),
  ),
  validateWorkspaceFile: SCOPES.map(
    (scope): Sample =>
      (a) =>
        a.validateWorkspaceFile(agentForScope(scope), "a.md", "text", scope),
  ),
  deleteWorkspaceFile: SCOPES.map(
    (scope): Sample =>
      (a) =>
        a.deleteWorkspaceFile(agentForScope(scope), "a.md", scope),
  ),
  moveWorkspaceFile: SCOPES.map(
    (scope): Sample =>
      (a) =>
        a.moveWorkspaceFile(agentForScope(scope), "a.md", "b.md", true, scope),
  ),
  fetchCheckpoints: REPOS.map(
    (repo): Sample =>
      (a) =>
        a.fetchCheckpoints(agentForRepo(repo), {
          repo,
          path: "a.md",
          turnId: "turn-1",
          before: "abc",
          limit: 5,
        }),
  ),
  fetchCheckpointStats: REPOS.map(
    (repo): Sample =>
      (a) =>
        a.fetchCheckpointStats(agentForRepo(repo), repo),
  ),
  fetchCheckpointDetail: REPOS.map(
    (repo): Sample =>
      (a) =>
        a.fetchCheckpointDetail(agentForRepo(repo), "abc", repo),
  ),
  fetchCheckpointDiff: REPOS.map(
    (repo): Sample =>
      (a) =>
        a.fetchCheckpointDiff(agentForRepo(repo), "abc", repo, "a.md"),
  ),
  fetchCheckpointFile: REPOS.map(
    (repo): Sample =>
      (a) =>
        a.fetchCheckpointFile(agentForRepo(repo), "abc", repo, "a.md"),
  ),
  restoreCheckpoint: REPOS.map(
    (repo): Sample =>
      (a) =>
        a.restoreCheckpoint(agentForRepo(repo), "abc", repo, "a.md"),
  ),
  undoCheckpoint: REPOS.map(
    (repo): Sample =>
      (a) =>
        a.undoCheckpoint(agentForRepo(repo), "abc", repo),
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
  hub.createAgent(AGENT);
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
  try {
    await sample(client);
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
      `fetchScheduledPulses: GET /api/agents/${AGENT}/scheduled/pulses (no route for GET /api/scheduled/pulses)`,
    ]);
  });

  it("serves every route an artifact can reach through the bridge", () => {
    const unserved = ARTIFACT_CALLS.flatMap(([method, path]) => {
      const problem = problemWith(apiRoutes, { method, url: scopeApiPath(path, AGENT) });
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
      expect(new URL(agentWsUrl(AGENT)).pathname).toBe(agentSocketPath(AGENT));
    });
  });
});
