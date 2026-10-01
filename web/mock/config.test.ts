import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { configRoutes } from "./config";
import { createState } from "./state";
import { fetchJson, fetchText, startRouteHarness, type RouteHarness } from "./test-support";

type Body = Record<string, unknown>;

describe("config routes", () => {
  let harness: RouteHarness;

  beforeEach(async () => {
    harness = await startRouteHarness(configRoutes);
  });

  afterEach(async () => {
    await harness.close();
  });

  const url = (path: string): string => `${harness.baseUrl}${path}`;

  async function request(
    method: string,
    path: string,
    body?: unknown,
  ): Promise<{ status: number; body: Body }> {
    const res = await fetchJson(url(path), {
      method,
      headers: { "Content-Type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    return { status: res.status, body: res.body as Body };
  }

  async function putRaw(path: string, content: string): Promise<void> {
    const res = await fetch(url(path), { method: "PUT", body: content });
    expect(res.status).toBe(200);
  }

  describe("status and system", () => {
    it("reports the mode, the version, the features and each checkpoint repository", async () => {
      const { status, body } = await request("GET", "/api/status");
      expect(status).toBe(200);
      expect(Object.keys(body).sort()).toEqual(["checkpoints", "features", "mode", "version"]);
      expect(body).toMatchObject({
        mode: "running",
        version: "0.0.0-mock",
        features: ["model-complete", "artifact-sessions", "artifact-state"],
      });
      const { checkpoints } = body as { checkpoints: Record<string, Record<string, unknown>> };
      expect(Object.keys(checkpoints).sort()).toEqual(["agent_config", "hub", "team", "workspace"]);
      for (const stats of Object.values(checkpoints)) {
        expect(stats.on_disk_bytes).toEqual(expect.any(Number));
        expect(stats.checkpoint_count).toEqual(expect.any(Number));
        expect(Date.parse(String(stats.oldest))).not.toBeNaN();
      }
    });

    it("answers the timezone and feedback submissions", async () => {
      expect((await request("GET", "/api/system/timezone")).body).toEqual({
        timezone: "America/New_York",
      });
      expect(
        (await request("POST", "/api/tracing/bug-report", { description: "x" })).body,
      ).toMatchObject({
        public_id: "RR-MOCK-BUG-01",
      });
      expect(
        (await request("POST", "/api/tracing/feedback", { description: "x" })).body,
      ).toMatchObject({
        public_id: "RR-MOCK-FBK-01",
      });
    });
  });

  describe("config documents", () => {
    it.each([
      ["/api/config", "configToml"],
      ["/api/hub/config", "hubConfigToml"],
      ["/api/providers", "providersToml"],
    ] as const)("reads, replaces and validates %s", async (prefix, field) => {
      const before = await fetchText(url(`${prefix}/raw`));
      expect(before.status).toBe(200);
      expect(before.body).toBe(harness.state[field]);
      expect(before.body.length).toBeGreaterThan(0);

      await putRaw(`${prefix}/raw`, 'a = 1\n[t]\nb = "x"\n');
      expect((await fetchText(url(`${prefix}/raw`))).body).toBe('a = 1\n[t]\nb = "x"\n');

      const check = async (text: string): Promise<Body> => {
        const res = await fetchJson(url(`${prefix}/validate`), { method: "POST", body: text });
        return res.body as Body;
      };
      expect(await check("a = 1\n")).toEqual({ valid: true });
      expect(await check("a = 1\nb = \n")).toEqual({
        valid: false,
        error: "Invalid TOML document: invalid value",
        diagnostics: [
          {
            severity: "error",
            message: "Invalid TOML document: invalid value",
            location: { kind: "line_column", line: 2, column: 5 },
          },
        ],
      });
    });

    it("writes a raw save with problems, reports them, and checkpoints the file first", async () => {
      const before = harness.state.checkpoints.agent_config?.length ?? 0;
      const res = await fetchJson(url("/api/config/raw"), { method: "PUT", body: "a = \n" });
      expect(res.body).toMatchObject({ valid: false, diagnostics: [{ severity: "error" }] });
      expect(harness.state.configToml).toBe("a = \n");
      expect(harness.state.checkpoints.agent_config?.length).toBe(before + 1);
    });

    it("patches a TOML document: set, remove, inline tables, and pruning empty tables", async () => {
      await putRaw("/api/config/raw", 'a = 1\n[t]\nb = "x"\nc = 1\n[gone]\nk = 1\n');
      const patch = await request("PATCH", "/api/config/patch", {
        a: 2,
        t: { b: null, d: { e: "deep" } },
        gone: { k: null },
        z: { $inline: { q: 1 } },
      });
      expect(patch).toEqual({
        status: 200,
        body: { valid: true, checkpoint_id: expect.any(String) as unknown },
      });
      const raw = (await fetchText(url("/api/config/raw"))).body;
      expect(raw).toContain("a = 2");
      expect(raw).toMatch(/\[t\]\s+c = 1/);
      expect(raw).not.toContain('b = "x"');
      expect(raw).toContain('e = "deep"');
      expect(raw).not.toContain("gone");
      expect(raw).toMatch(/\[z\]\s+q = 1/);
    });

    it("refuses a patch the backend's validation would, and writes nothing", async () => {
      await putRaw("/api/config/raw", "timeout_secs = 30\n");
      const refused = await request("PATCH", "/api/config/patch", {
        agent: { max_tool_iterations: 0 },
      });
      expect(refused.status).toBe(400);
      expect(refused.body).toEqual({
        valid: false,
        error: "agent.max_tool_iterations must be at least 1 (leave it unset for unlimited)",
        diagnostics: [],
      });
      expect(harness.state.configToml).toBe("timeout_secs = 30\n");
    });

    it("patches an empty document", async () => {
      await putRaw("/api/providers/raw", "");
      await request("PATCH", "/api/providers/patch", { models: { main: "anthropic/x" } });
      expect(harness.state.providersToml).toContain('main = "anthropic/x"');
    });

    it("answers a body that is not a JSON object with 500 and says why", async () => {
      const { status, body } = await request("PATCH", "/api/config/patch", null);
      expect(status).toBe(500);
      expect(body.error).toBe("the request body must be a JSON object");
      const invalid = await fetchJson(url("/api/config/patch"), { method: "PATCH", body: "{oops" });
      expect(invalid.status).toBe(500);
    });
  });

  describe("MCP", () => {
    it("patches servers in and out, always keeping a `mcpServers` object", async () => {
      await putRaw("/api/mcp/raw", '{"mcpServers":{"a":{"command":"x"}}}');
      await request("PATCH", "/api/mcp/patch", { mcpServers: { b: { command: "y" }, a: null } });
      expect(JSON.parse(harness.state.mcpJson)).toEqual({ mcpServers: { b: { command: "y" } } });

      await request("PATCH", "/api/mcp/patch", { mcpServers: { b: null } });
      expect(JSON.parse(harness.state.mcpJson)).toEqual({ mcpServers: {} });
    });

    it("starts an empty document with `mcpServers`", async () => {
      await putRaw("/api/mcp/raw", "");
      await request("PATCH", "/api/mcp/patch", {});
      expect(JSON.parse(harness.state.mcpJson)).toEqual({ mcpServers: {} });
    });

    it("serves the server catalog", async () => {
      const { status, body } = await fetchJson(url("/api/mcp-catalog"));
      expect(status).toBe(200);
      expect(Array.isArray(body)).toBe(true);
      expect((body as unknown[]).length).toBeGreaterThan(0);
    });
  });

  describe("provider models", () => {
    it("matches a provider by the known name inside it, case-insensitively", async () => {
      const { body } = await request("POST", "/api/providers/models", { provider: "My-Anthropic" });
      expect(body.models).toContainEqual({ id: "claude-opus-4-6", name: "Claude Opus 4.6" });
    });

    it("offers one default model for a provider it does not know", async () => {
      const { body } = await request("POST", "/api/providers/models", { provider: "Weird" });
      expect(body.models).toEqual([{ id: "weird/default-model", name: "Default Model" }]);
    });
  });

  describe("agent keys", () => {
    it("lists keys by name without their values", async () => {
      const { body } = await request("GET", "/api/agent-keys");
      expect(body.keys).toEqual([
        {
          name: "cf_session",
          env_var: "CF_SESSION",
          description: "Short-lived Cloudflare API token minted for DNS updates",
          created_by: "agent",
        },
        {
          name: "github_token",
          env_var: "GITHUB_TOKEN",
          description: "Fine-grained token, read/write on my repos",
          created_by: "user",
        },
      ]);
    });

    it("adds a key, and refuses a bad name or a short value", async () => {
      const rejected = async (body: unknown): Promise<{ status: number; body: string }> =>
        fetchText(url("/api/agent-keys"), { method: "POST", body: JSON.stringify(body) });
      const invalid = { status: 400, body: "key name or value is invalid" };
      expect(await rejected({ name: "Bad", value: "12345678" })).toEqual(invalid);
      expect(await rejected({ name: "good", value: "123" })).toEqual(invalid);
      expect(await rejected({})).toEqual(invalid);

      const ok = await request("POST", "/api/agent-keys", {
        name: "good",
        value: "12345678",
        description: "d",
      });
      expect(ok).toEqual({ status: 200, body: { name: "good", env_var: "GOOD" } });
      expect(harness.state.agentKeys.get("good")).toEqual({
        value: "12345678",
        description: "d",
        created_by: "user",
      });
    });

    it("deletes a key once, then answers 404", async () => {
      const first = await request("DELETE", "/api/agent-keys/github_token");
      expect(first).toEqual({ status: 200, body: { deleted: true, checkpoint_id: null } });
      const second = await fetchText(url("/api/agent-keys/github_token"), { method: "DELETE" });
      expect(second).toEqual({ status: 404, body: "no agent key named 'github_token'" });
    });
  });

  describe("A2A", () => {
    it("reports how the scoped agent is reached: locally, with no relay and no address of its own", async () => {
      const { status, body } = await request("GET", "/api/a2a/status");
      expect(status).toBe(200);
      expect(body).toEqual({
        enabled: true,
        port: 7702,
        visibility: "public",
        public_url: null,
        local_url: "http://127.0.0.1:7702/agents/atlas",
        relay_access: false,
        relay_access_note: expect.stringMatching(/^Reachable locally\./) as unknown,
        listener_running: true,
        card_error: null,
      });
    });

    it("describes the card from the workspace file", async () => {
      const { body } = await request("GET", "/api/a2a/card");
      expect(body).toMatchObject({ name: "Residuum agent" });
      expect(Array.isArray(body.skills)).toBe(true);
      harness.state.workspaceFileContents["config/agent-card.json"] = "{}";
      expect((await request("GET", "/api/a2a/card")).body).toEqual({
        name: "Residuum agent",
        description: "",
        skills: [],
      });
    });

    it("creates caller keys with a one-time token, and refuses duplicates and bad names", async () => {
      expect((await request("POST", "/api/a2a/keys", { name: "BAD" })).status).toBe(400);
      const created = await request("POST", "/api/a2a/keys", { name: "peer", description: "p" });
      expect(created.status).toBe(200);
      expect(String(created.body.token)).toMatch(/^rsdm_a2a_mock[a-z0-9]+$/);
      expect((await request("POST", "/api/a2a/keys", { name: "peer" })).status).toBe(409);

      const keys = (await request("GET", "/api/a2a/keys")).body.keys as Body[];
      expect(keys.map((k) => k.name)).toEqual(["laptop", "peer"]);

      expect((await request("DELETE", "/api/a2a/keys/peer")).body).toEqual({
        revoked: true,
        checkpoint_id: null,
      });
      expect((await request("DELETE", "/api/a2a/keys/peer")).status).toBe(404);
    });

    it("lists remote agents and keeps the agents file as written", async () => {
      const { body } = await fetchJson(url("/api/a2a/agents"));
      expect((body as Body[]).map((a) => a.name)).toEqual(["research-buddy", "laptop"]);
      await putRaw("/api/a2a/agents/raw", '{"agents":{}}');
      const raw = await fetch(url("/api/a2a/agents/raw"));
      expect(raw.headers.get("content-type")).toBe("application/json");
      expect(await raw.text()).toBe('{"agents":{}}');
    });

    it("stops a reachable outbound task and announces it closed", async () => {
      const { status, body } = await request("POST", "/api/a2a/outbound/task-7f3a/stop");
      expect(status).toBe(200);
      expect(body).toMatchObject({ task_id: "task-7f3a", state: "canceled", open: false });
      expect(harness.frames).toContainEqual({ type: "session_outbound_a2a_task", task: body });
      expect(harness.state.outboundTasks.map((t) => t.task_id)).toEqual(["task-19c2"]);

      const again = await request("POST", "/api/a2a/outbound/task-7f3a/stop");
      expect(again.status).toBe(404);
      expect(again.body.code).toBe("not_open");
    });

    it("fails to stop an unreachable task, and lets the user stop watching it", async () => {
      const stop = await request("POST", "/api/a2a/outbound/task-19c2/stop");
      expect(stop.status).toBe(502);
      expect(stop.body.code).toBe("unreachable");
      expect(harness.state.outboundTasks).toHaveLength(2);

      const watching = await request("POST", "/api/a2a/outbound/task-19c2/stop-watching");
      expect(watching.status).toBe(200);
      expect(harness.state.outboundTasks.map((t) => t.task_id)).toEqual(["task-7f3a"]);
    });
  });

  describe("secrets", () => {
    it("adds and removes a secret, answering the reference to use", async () => {
      expect((await request("GET", "/api/secrets")).body).toEqual({
        names: ["anthropic_key", "openai_key"],
      });
      expect((await request("POST", "/api/secrets", { name: "s1", value: "v" })).body).toEqual({
        reference: "secret:s1",
      });
      expect((await request("GET", "/api/secrets")).body.names).toContain("s1");
      expect((await request("DELETE", "/api/secrets/s1")).body).toEqual({ deleted: true });
      expect((await request("GET", "/api/secrets")).body.names).not.toContain("s1");
    });
  });

  describe("setup completion", () => {
    const complete = (body: unknown): Promise<{ status: number; body: Body }> =>
      request("POST", "/api/hub/config/complete-setup", body);

    it("refuses a missing or malformed agent name", async () => {
      const missing = await complete({});
      expect(missing.status).toBe(400);
      expect(missing.body).toEqual({
        valid: false,
        error: "agent name must not be empty",
        diagnostics: [],
      });
      expect((await complete({ agent_name: "Bad Name" })).status).toBe(400);
    });

    it("only creates the first agent", async () => {
      const other = await complete({ agent_name: "newbie" });
      expect(other.status).toBe(409);
      expect(other.body.error).toBe(
        "This residuum already has an agent ('atlas'). Setup only creates the first agent.",
      );
      const same = await complete({ agent_name: "atlas" });
      expect(same.status).toBe(409);
      expect(String(same.body.error)).toMatch(/^An agent named 'atlas' already exists/);
    });

    it("creates the first agent from the wizard's files and leaves setup mode", async () => {
      harness.hub.agents.clear();
      harness.state.mode = "setup";
      const { status, body } = await complete({
        agent_name: "first",
        config: "a = 1",
        providers: "b = 2",
        mcp_json: '{"mcpServers":{}}',
        hub_config: "c = 3",
      });
      expect(status).toBe(200);
      expect(body).toEqual({ valid: true, diagnostics: [] });
      const created = harness.hub.agents.get("first");
      expect(created?.state).toMatchObject({
        configToml: "a = 1",
        providersToml: "b = 2",
        mcpJson: '{"mcpServers":{}}',
      });
      expect(harness.state.hubConfigToml).toBe("c = 3");
      expect(harness.state.mode).toBe("running");
    });

    it("keeps the agent's example files when the wizard sends none", async () => {
      harness.hub.agents.clear();
      const { status } = await complete({ agent_name: "first" });
      expect(status).toBe(200);
      const created = harness.hub.agents.get("first");
      const examples = createState("first");
      expect(created?.state.configToml).toBe(examples.configToml);
      expect(created?.state.providersToml).toBe(examples.providersToml);
      expect(created?.state.configToml.length).toBeGreaterThan(0);
    });
  });
});
