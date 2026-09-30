import { afterEach, describe, expect, it } from "vitest";
import { fetchJson, startMockServer, type MockServerHarness } from "./test-support";

describe("the API handler", () => {
  let harness: MockServerHarness;

  afterEach(async () => {
    await harness.close();
  });

  it("scopes a request to the agent and runs its route", async () => {
    harness = await startMockServer();
    const res = await fetchJson(`${harness.baseUrl}/api/agents/atlas/system/timezone`);
    expect(res).toEqual({ status: 200, body: { timezone: "America/New_York" } });
  });

  it("passes a request outside /api on", async () => {
    harness = await startMockServer();
    const res = await fetch(`${harness.baseUrl}/index.html`);
    expect(res.status).toBe(404);
    expect(await res.text()).toBe("");
  });

  it("names the endpoint when no route and no fallback takes the request", async () => {
    harness = await startMockServer();
    expect(await fetchJson(`${harness.baseUrl}/api/agents/atlas/nothing-here?x=1`)).toEqual({
      status: 404,
      body: { error: "mock: unknown endpoint GET /api/nothing-here" },
    });
  });

  it("refuses an /api path that skips the scope", async () => {
    harness = await startMockServer();
    const res = await fetchJson(`${harness.baseUrl}/api/status`);
    expect(res.status).toBe(404);
    expect((res.body as { error: string }).error).toContain("is not a hub, team, or agent route");
  });

  it("gives a request no table matched to the fallback, with the scoped state", async () => {
    harness = await startMockServer({
      fallback: ({ res, state, path, query }) => {
        res.writeHead(200, { "Content-Type": "application/json" });
        res.end(JSON.stringify({ agent: state.agentName, path, q: query.get("q") }));
        return Promise.resolve(true);
      },
    });
    const res = await fetchJson(`${harness.baseUrl}/api/agents/scout/custom?q=1`);
    expect(res).toEqual({ status: 200, body: { agent: "scout", path: "/api/custom", q: "1" } });
  });

  it("answers 500 when a handler throws", async () => {
    harness = await startMockServer({
      fallback: () => Promise.reject(new Error("the fallback failed")),
    });
    const res = await fetchJson(`${harness.baseUrl}/api/agents/scout/custom`);
    expect(res).toEqual({
      status: 500,
      body: { error: "mock server error: the fallback failed" },
    });
  });

  it("answers 500 for a body a route can't parse", async () => {
    harness = await startMockServer();
    const res = await fetchJson(`${harness.baseUrl}/api/agents/atlas/config/patch`, {
      method: "PATCH",
      body: "not json",
    });
    expect(res.status).toBe(500);
  });
});
