// residuum.fetch, residuum.ask and residuum.state in the workbench SDK
// (assets/workbench/sdk.js): how a page's calls reach Residuum on its own origin.

import { afterEach, describe, expect, it, vi } from "vitest";
import { described, lastRequest, loadSdk, settle } from "../test/workbench-sdk";

afterEach(() => {
  vi.useRealTimers();
});

describe("residuum.fetch paths", () => {
  it.each([
    ["/api/hub/status", "/api/hub/status"],
    ["/api/team/workspace/file?path=a.md", "/api/team/workspace/file?path=a.md"],
    ["/api/agents/atlas/sessions?artifact=chart", "/api/agents/atlas/sessions?artifact=chart"],
    ["/api/secrets", "/api/hub/secrets"],
    ["/api/a2a/keys/laptop", "/api/hub/a2a/keys/laptop"],
    ["/api/system/timezone", "/api/hub/system/timezone"],
    ["/api/update/status", "/api/hub/update/status"],
    ["/api/checkpoints?repo=team", "/api/hub/checkpoints?repo=team"],
    ["/api/workbench/artifacts", "/api/team/workbench/artifacts"],
    [`http://localhost:7702/api/hub/status`, "/api/hub/status"],
  ])("sends %s to %s", async (path, sent) => {
    const { sdk, requests } = loadSdk();
    void sdk.fetch(path);
    await settle();
    expect(lastRequest(requests).url).toBe(sent);
  });

  it.each([
    ["GET", "/api/status", "/api/agents/<name>/status"],
    ["GET", "/api/inbox?limit=5", "/api/agents/<name>/inbox"],
    ["GET", "/api/checkpoints?repo=workspace", "/api/agents/<name>/checkpoints"],
    ["GET", "/api/sessions", "/api/agents/<name>/sessions"],
    ["GET", "/api/secretsvault", "/api/agents/<name>/secretsvault"],
  ])(
    "answers an unscoped agent path (%s %s) with 400 saying to name the agent",
    async (method, path, named) => {
      const { sdk, requests, console } = loadSdk();
      const resp = await sdk.fetch(path, { method });
      expect(resp.status).toBe(400);
      const { error } = (await resp.json()) as { error: string };
      expect(error).toContain(named);
      expect(error).toContain("/api/hub/agents");
      expect(requests).toEqual([]);
      expect(console.warn).toHaveBeenCalledWith(expect.stringContaining(named));
    },
  );

  it("points a session start and a model call without an agent at the SDK call that names one", async () => {
    const { sdk, requests } = loadSdk();
    const session = await sdk.fetch("/api/sessions", { method: "POST", body: { prompt: "go" } });
    expect(session.status).toBe(400);
    expect(((await session.json()) as { error: string }).error).toContain(
      "residuum.sessions.start({ agent, prompt })",
    );
    const model = await sdk.fetch("/api/model/complete", { method: "post" });
    expect(model.status).toBe(400);
    expect(((await model.json()) as { error: string }).error).toContain(
      "residuum.ask(prompt, { agent })",
    );
    expect(requests).toEqual([]);
  });

  it("answers a path outside the API with 400", async () => {
    const { sdk, requests } = loadSdk();
    for (const path of [
      "/chart/data.json",
      "https://example.com/api/hub/status",
      "api/hub/status",
    ]) {
      const resp = await sdk.fetch(path);
      expect(resp.status).toBe(400);
      expect(((await resp.json()) as { error: string }).error).toContain("/api/");
    }
    expect(requests).toEqual([]);
  });

  it("rejects a path that isn't a string", async () => {
    const { sdk } = loadSdk();
    await expect(sdk.fetch(42).catch(described)).resolves.toMatchObject({ name: "TypeError" });
  });
});

describe("residuum.fetch requests", () => {
  it("names the artifact on every request, whatever the page set", async () => {
    const { sdk, requests } = loadSdk("chart");
    void sdk.fetch("/api/hub/status", { headers: { "x-residuum-artifact": "other" } });
    await settle();
    expect(lastRequest(requests).headers.get("X-Residuum-Artifact")).toBe("chart");
  });

  it("sends a plain object as JSON and binary bodies unchanged", async () => {
    const { sdk, requests } = loadSdk();
    void sdk.fetch("/api/team/workspace/file", { method: "PUT", body: { path: "a.md" } });
    await settle();
    const json = lastRequest(requests);
    expect(json.method).toBe("PUT");
    expect(json.body).toBe('{"path":"a.md"}');
    expect(json.headers.get("content-type")).toBe("application/json");

    const bytes = new Uint8Array([1, 2, 3]);
    void sdk.fetch("/api/team/workspace/raw?path=a.bin", { method: "PUT", body: bytes });
    await settle();
    expect(lastRequest(requests).body).toBe(bytes);
    expect(lastRequest(requests).headers.get("content-type")).toBeNull();
  });

  it("refuses a body it can't send", async () => {
    const { sdk } = loadSdk();
    await expect(
      sdk.fetch("/api/hub/status", { method: "POST", body: new Map() }).catch(described),
    ).resolves.toMatchObject({ name: "TypeError" });
  });

  it("returns Residuum's response as it is", async () => {
    const { sdk, requests } = loadSdk();
    const pending = sdk.fetch("/api/hub/shutdown", { method: "POST" });
    await settle();
    lastRequest(requests).respond(403, { error: "not from the workbench" });
    const resp = await pending;
    expect(resp.status).toBe(403);
    expect(await resp.json()).toEqual({ error: "not from the workbench" });
  });

  it("explains a request that never reached Residuum", async () => {
    const { sdk, requests } = loadSdk();
    const pending = sdk.fetch("/api/hub/status").catch(described);
    await settle();
    lastRequest(requests).fail(new TypeError("Failed to fetch"));
    expect(await pending).toEqual({
      name: "Error",
      message: "Couldn't reach Residuum. Check that it's running, then try again.",
    });
  });
});

describe("residuum.fetch lanes", () => {
  it("runs at most 8 ordinary requests at once and starts the next in order", async () => {
    const { sdk, requests } = loadSdk();
    for (let i = 0; i < 10; i += 1) void sdk.fetch(`/api/hub/status?n=${i}`);
    await settle();
    expect(requests.map((r) => r.url)).toEqual(
      [0, 1, 2, 3, 4, 5, 6, 7].map((i) => `/api/hub/status?n=${i}`),
    );

    requests[3]?.respond(200, {});
    await settle();
    expect(requests).toHaveLength(9);
    expect(lastRequest(requests).url).toBe("/api/hub/status?n=8");
  });

  it("runs at most 4 model calls at once, beside the ordinary requests", async () => {
    const { sdk, requests } = loadSdk();
    for (let i = 0; i < 8; i += 1) void sdk.fetch(`/api/hub/status?n=${i}`);
    for (let i = 0; i < 6; i += 1) void sdk.ask(`question ${i}`, { agent: "atlas" });
    await settle();
    const modelCalls = (): number =>
      requests.filter((r) => r.url === "/api/agents/atlas/model/complete").length;
    expect(requests).toHaveLength(12);
    expect(modelCalls()).toBe(4);

    requests.find((r) => r.url === "/api/agents/atlas/model/complete")?.respond(200, {});
    await settle();
    expect(modelCalls()).toBe(5);
  });
});

describe("residuum.fetch overload responses", () => {
  it("returns a 503 as it is, without sending the request again", async () => {
    vi.useFakeTimers();
    const { sdk, requests } = loadSdk();
    const pending = sdk.fetch("/api/agents/atlas/status");
    await settle();
    lastRequest(requests).respond(503, "agent overloaded");
    const resp = await pending;
    expect(resp.status).toBe(503);
    expect(await resp.text()).toBe("agent overloaded");
    await vi.advanceTimersByTimeAsync(5000);
    expect(requests).toHaveLength(1);
  });
});

describe("residuum.state", () => {
  it("reads the artifact's state file from the team workbench", async () => {
    const { sdk, requests } = loadSdk("wiki");
    const loaded = sdk.state.get();
    await settle();
    const request = lastRequest(requests);
    expect(request.method).toBe("GET");
    expect(request.url).toBe("/api/team/workspace/file?path=workbench%2Fwiki.state.json");
    request.respond(200, { picked: 3 });
    expect(await loaded).toEqual({ picked: 3 });
  });

  it("resolves to null before the first set", async () => {
    const { sdk, requests } = loadSdk("wiki");
    const loaded = sdk.state.get();
    await settle();
    lastRequest(requests).respond(404, "");
    expect(await loaded).toBeNull();
  });

  it("writes the state file into the team workbench", async () => {
    const { sdk, requests } = loadSdk("wiki");
    const saved = sdk.state.set({ picked: 4 });
    await settle();
    const request = lastRequest(requests);
    expect(request.method).toBe("PUT");
    expect(request.url).toBe("/api/team/workspace/file");
    expect(JSON.parse(request.body as string)).toEqual({
      path: "workbench/wiki.state.json",
      content: JSON.stringify({ picked: 4 }),
    });
    request.respond(200, {});
    await saved;
  });
});

describe("residuum.ask", () => {
  it("calls the named agent's model route and leaves the agent out of the body", async () => {
    const { sdk, requests } = loadSdk();
    const asked = sdk.ask({ agent: "scout", prompt: "summarize", max_tokens: 50 });
    await settle();
    const request = lastRequest(requests);
    expect(request.method).toBe("POST");
    expect(request.url).toBe("/api/agents/scout/model/complete");
    expect(JSON.parse(request.body as string)).toEqual({ prompt: "summarize", max_tokens: 50 });
    request.respond(200, { content: "done" });
    expect(await asked).toEqual({ content: "done" });
  });

  it("takes the agent as a second argument for the prompt shorthand", async () => {
    const { sdk, requests } = loadSdk();
    void sdk.ask("hello", { agent: "scout" });
    await settle();
    expect(lastRequest(requests).url).toBe("/api/agents/scout/model/complete");
    expect(JSON.parse(lastRequest(requests).body as string)).toEqual({ prompt: "hello" });
  });

  it("needs an agent, and sends nothing without one", async () => {
    const { sdk, requests } = loadSdk();
    await expect(sdk.ask("hello").catch(described)).resolves.toMatchObject({
      name: "TypeError",
      message: expect.stringContaining("residuum.ask needs an agent") as unknown,
    });
    await expect(sdk.ask({ prompt: "hello" }).catch(described)).resolves.toMatchObject({
      name: "TypeError",
    });
    expect(requests).toEqual([]);
  });

  it("rejects with Residuum's error", async () => {
    const { sdk, requests } = loadSdk();
    const asked = sdk.ask("hello", { agent: "drifter" }).catch(described);
    await settle();
    lastRequest(requests).respond(409, { error: "drifter is stopped", state: "stopped" });
    expect(await asked).toMatchObject({ message: "drifter is stopped" });
  });
});
