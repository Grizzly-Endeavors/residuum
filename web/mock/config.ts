import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { parse as parseToml, stringify as stringifyToml } from "smol-toml";
import type { OutboundA2aTaskSummary, RepoStats } from "../src/lib/generated/protocol";
import type {
  A2aAgentCard,
  A2aCardSkill,
  A2aKeysListResponse,
  A2aRemoteAgent,
  A2aStatusResponse,
  AgentKeysListResponse,
  CreateA2aKeyResponse,
  DeleteSecretResponse,
  ModelsResponse,
  SecretResponse,
  SecretsListResponse,
  SetAgentKeyResponse,
  StatusResponse,
  TimezoneResponse,
  ValidateResponse,
} from "../src/lib/types";
import { agentNameProblem } from "./agent-name";
import { WEB_ROOT } from "./assets";
import { MOCK_CLOUD_STATUS, MOCK_FEATURES, MOCK_RESIDUUM_VERSION } from "./constants";
import {
  json,
  parseJsonObject,
  readBody,
  readJsonObject,
  stringField,
  text,
  type JsonObject,
} from "./http";
import { decodedParam, type Route, type RouteContext } from "./routes";
import { byName } from "./util";

const modelsByProvider: Record<string, Array<{ id: string; name: string }>> = {
  anthropic: [
    { id: "claude-opus-4-6", name: "Claude Opus 4.6" },
    { id: "claude-sonnet-4-6", name: "Claude Sonnet 4.6" },
    { id: "claude-haiku-4-5", name: "Claude Haiku 4.5" },
  ],
  openai: [
    { id: "gpt-4o", name: "GPT-4o" },
    { id: "gpt-4o-mini", name: "GPT-4o Mini" },
    { id: "o3", name: "o3" },
    { id: "o4-mini", name: "o4-mini" },
  ],
  gemini: [
    { id: "gemini-2.5-pro", name: "Gemini 2.5 Pro" },
    { id: "gemini-2.5-flash", name: "Gemini 2.5 Flash" },
    { id: "gemini-3.0-flash", name: "Gemini 3.0 Flash" },
  ],
  fireworks: [
    { id: "accounts/fireworks/models/glm-5p3", name: "accounts/fireworks/models/glm-5p3" },
    { id: "accounts/fireworks/models/kimi-k3", name: "accounts/fireworks/models/kimi-k3" },
    {
      id: "accounts/fireworks/routers/glm-flash-latest",
      name: "accounts/fireworks/routers/glm-flash-latest",
    },
  ],
  ollama: [
    { id: "llama3.3:latest", name: "Llama 3.3" },
    { id: "mistral:latest", name: "Mistral" },
    { id: "deepseek-r1:latest", name: "DeepSeek R1" },
    { id: "qwen3:latest", name: "Qwen 3" },
  ],
};

/** The shape agent key names and A2A caller key names both take. */
const KEY_NAME = /^[a-z][a-z0-9_]{0,63}$/;

const VALID: ValidateResponse = { valid: true };

/**
 * Merge a JSON diff (the shape the Settings form's diff builders in
 * `lib/settings-toml.ts` send) into a plain object in place — the mock's
 * stand-in for the real backend's `toml_edit`/JSON-merge patching
 * (`src/config/patch.rs`, `src/workspace/mcp_patch.rs`). Comment
 * preservation doesn't apply here (mock state is never a real file with
 * comments), but the merge semantics match: `null` removes a key, a nested
 * object recurses, and `{"$inline": {...}}` sets the key to that inner
 * object directly.
 */
function applyJsonPatch(target: JsonObject, diff: JsonObject): void {
  for (const [key, val] of Object.entries(diff)) {
    if (val === null) {
      delete target[key];
    } else if (typeof val === "object" && !Array.isArray(val)) {
      const obj = val as JsonObject;
      if ("$inline" in obj) {
        target[key] = obj.$inline;
        continue;
      }
      const existing = target[key];
      const sub =
        typeof existing === "object" && existing !== null && !Array.isArray(existing)
          ? (existing as JsonObject)
          : {};
      target[key] = sub;
      applyJsonPatch(sub, obj);
      if (Object.keys(sub).length === 0) delete target[key];
    } else {
      target[key] = val;
    }
  }
}

// ─── Status & system ───────────────────────────────────────────────────────────

/** A checkpoint repository's size and history, as the status route reports it. */
function repoStats(bytes: number, count: number, daysOld: number): RepoStats {
  return {
    on_disk_bytes: bytes,
    checkpoint_count: count,
    oldest: new Date(Date.now() - daysOld * 86_400_000).toISOString(),
  };
}

/** Plausible stats for the four checkpoint repositories. */
function checkpointStats(): StatusResponse["checkpoints"] {
  return {
    workspace: repoStats(18_874_368, 142, 45),
    team: repoStats(6_291_456, 57, 45),
    agent_config: repoStats(1_048_576, 23, 30),
    hub: repoStats(524_288, 11, 30),
  };
}

const systemRoutes: readonly Route[] = [
  {
    method: "GET",
    pattern: "/api/status",
    handler: ({ res, state }) => {
      json(res, 200, {
        mode: state.mode,
        version: MOCK_RESIDUUM_VERSION,
        features: [...MOCK_FEATURES],
        checkpoints: checkpointStats(),
      } satisfies StatusResponse);
    },
  },
  {
    method: "GET",
    pattern: "/api/system/timezone",
    handler: ({ res }) => {
      json(res, 200, { timezone: "America/New_York" } satisfies TimezoneResponse);
    },
  },
  {
    method: "GET",
    pattern: "/api/cloud/status",
    handler: ({ res }) => {
      json(res, 200, MOCK_CLOUD_STATUS);
    },
  },
  {
    method: "POST",
    pattern: "/api/tracing/bug-report",
    handler: async ({ req, res }) => {
      // Drain the body so the dev server can inspect it if asked.
      await readBody(req);
      json(res, 200, {
        public_id: "RR-MOCK-BUG-01",
        submitted_at: new Date().toISOString(),
      });
    },
  },
  {
    method: "POST",
    pattern: "/api/tracing/feedback",
    handler: async ({ req, res }) => {
      await readBody(req);
      json(res, 200, {
        public_id: "RR-MOCK-FBK-01",
        submitted_at: new Date().toISOString(),
      });
    },
  },
];

// ─── Config documents ──────────────────────────────────────────────────────────

type TomlDocument = "configToml" | "hubConfigToml" | "providersToml";

/**
 * Read, replace, patch and validate one TOML document kept in the state.
 * `afterWrite` runs once a write has been answered, as the hub reloads after
 * its own config changes.
 */
function tomlDocumentRoutes(
  prefix: string,
  field: TomlDocument,
  afterWrite: (ctx: RouteContext) => void = () => {},
): readonly Route[] {
  return [
    {
      method: "GET",
      pattern: `${prefix}/raw`,
      handler: ({ res, state }) => {
        text(res, 200, state[field]);
      },
    },
    {
      method: "PUT",
      pattern: `${prefix}/raw`,
      handler: async (ctx) => {
        ctx.state[field] = await readBody(ctx.req);
        json(ctx.res, 200, VALID);
        afterWrite(ctx);
      },
    },
    {
      method: "PATCH",
      pattern: `${prefix}/patch`,
      handler: async (ctx) => {
        const { state } = ctx;
        const diff = await readJsonObject(ctx.req);
        const doc = state[field].trim() ? parseToml(state[field]) : {};
        applyJsonPatch(doc, diff);
        state[field] = stringifyToml(doc);
        json(ctx.res, 200, VALID);
        afterWrite(ctx);
      },
    },
    {
      method: "POST",
      pattern: `${prefix}/validate`,
      handler: ({ res }) => {
        json(res, 200, VALID);
      },
    },
  ];
}

const mcpRoutes: readonly Route[] = [
  {
    method: "GET",
    pattern: "/api/mcp/raw",
    handler: ({ res, state }) => {
      text(res, 200, state.mcpJson);
    },
  },
  {
    method: "PUT",
    pattern: "/api/mcp/raw",
    handler: async ({ req, res, state }) => {
      state.mcpJson = await readBody(req);
      json(res, 200, VALID);
    },
  },
  {
    method: "PATCH",
    pattern: "/api/mcp/patch",
    handler: async ({ req, res, state }) => {
      const diff = await readJsonObject(req);
      const doc = state.mcpJson.trim() ? parseJsonObject(state.mcpJson) : { mcpServers: {} };
      applyJsonPatch(doc, diff);
      doc.mcpServers ??= {};
      state.mcpJson = JSON.stringify(doc, null, 2);
      json(res, 200, VALID);
    },
  },
  {
    method: "GET",
    pattern: "/api/mcp-catalog",
    handler: ({ res }) => {
      try {
        const catalog = readFileSync(resolve(WEB_ROOT, "public", "mcp-catalog.json"), "utf-8");
        res.writeHead(200, { "Content-Type": "application/json" });
        res.end(catalog);
      } catch {
        json(res, 200, []);
      }
    },
  },
];

const providerRoutes: readonly Route[] = [
  ...tomlDocumentRoutes("/api/providers", "providersToml"),
  {
    method: "POST",
    pattern: "/api/providers/models",
    handler: async ({ req, res }) => {
      const body = await readJsonObject(req);
      const provider = (stringField(body, "provider") ?? "").toLowerCase();

      // Match against known provider types
      let providerType = provider;
      for (const key of Object.keys(modelsByProvider)) {
        if (provider.includes(key)) {
          providerType = key;
          break;
        }
      }

      const models = modelsByProvider[providerType] ?? [
        { id: `${provider}/default-model`, name: "Default Model" },
      ];
      json(res, 200, { models } satisfies ModelsResponse);
    },
  },
];

// ─── Setup ─────────────────────────────────────────────────────────────────────

/** Finish first-run setup: create the first agent from the wizard's files. */
async function completeSetup({ req, res, hub, state }: RouteContext): Promise<void> {
  const body = await readJsonObject(req);
  const name = stringField(body, "agent_name") ?? "";
  const nameProblem = agentNameProblem(name);
  if (nameProblem !== null) {
    json(res, 400, {
      valid: false,
      error: nameProblem,
      diagnostics: [],
    } satisfies ValidateResponse);
    return;
  }
  // Setup only creates the first agent (`refuse_when_agents_exist`).
  if (hub.agents.size > 0) {
    const existing = [...hub.agents.keys()].sort(byName);
    json(res, 409, {
      valid: false,
      error: existing.includes(name)
        ? `An agent named '${name}' already exists. Choose a different name, or change the existing agent from its settings.`
        : `This residuum already has an agent ('${existing.join("', '")}'). Setup only creates the first agent.`,
      diagnostics: [],
    } satisfies ValidateResponse);
    return;
  }
  state.hubConfigToml = stringField(body, "hub_config") ?? state.hubConfigToml;
  hub.reloadHubConfig();
  const agent = hub.createAgent(name, { role: null });
  agent.state.configToml = stringField(body, "config") ?? agent.state.configToml;
  agent.state.providersToml = stringField(body, "providers") ?? agent.state.providersToml;
  const mcpJson = stringField(body, "mcp_json");
  if (mcpJson) {
    agent.state.mcpJson = mcpJson;
  }
  state.mode = "running";
  json(res, 200, { valid: true, diagnostics: [] } satisfies ValidateResponse);
}

// ─── Agent keys ────────────────────────────────────────────────────────────────

const agentKeyRoutes: readonly Route[] = [
  {
    method: "GET",
    pattern: "/api/agent-keys",
    handler: ({ res, state }) => {
      const keys = [...state.agentKeys.entries()]
        .sort(([a], [b]) => a.localeCompare(b))
        .map(([name, k]) => ({
          name,
          env_var: name.toUpperCase(),
          description: k.description,
          created_by: k.created_by,
        }));
      json(res, 200, { keys } satisfies AgentKeysListResponse);
    },
  },
  {
    method: "POST",
    pattern: "/api/agent-keys",
    handler: async ({ req, res, state }) => {
      const body = await readJsonObject(req);
      const name = stringField(body, "name") ?? "";
      const value = stringField(body, "value") ?? "";
      if (!KEY_NAME.test(name) || value.length < 8) {
        text(res, 400, "key name or value is invalid");
        return;
      }
      state.agentKeys.set(name, {
        value,
        description: stringField(body, "description") ?? "",
        created_by: "user",
      });
      json(res, 200, { name, env_var: name.toUpperCase() } satisfies SetAgentKeyResponse);
    },
  },
  {
    method: "DELETE",
    pattern: /^\/api\/agent-keys\/(.+)$/,
    handler: (ctx) => {
      const name = decodedParam(ctx, 0);
      if (!ctx.state.agentKeys.delete(name)) {
        text(ctx.res, 404, `no agent key named '${name}'`);
        return;
      }
      // The mock doesn't keep a checkpoint repository, so there is no id
      // for Undo to restore. A null id hides the button instead of offering
      // a restore that would 404.
      json(ctx.res, 200, { deleted: true, checkpoint_id: null });
    },
  },
];

// ─── A2A ───────────────────────────────────────────────────────────────────────

/** Answer a stop or stop-watching request for one outbound task. */
function closeOutboundTask(ctx: RouteContext): void {
  const { res, state } = ctx;
  const taskId = ctx.params[0] ?? "";
  const action = ctx.params[1];
  const task = state.outboundTasks.find((t) => t.task_id === decodeURIComponent(taskId));
  if (!task) {
    json(res, 404, {
      error: `Task ${taskId} isn't running anymore, so there's nothing to stop.`,
      code: "not_open",
    });
    return;
  }
  if (action === "stop" && task.unreachable_since) {
    json(res, 502, {
      error: `Couldn't reach ${task.agent} to cancel the task. You can stop watching it instead; it may keep running on their side.`,
      code: "unreachable",
    });
    return;
  }
  state.outboundTasks = state.outboundTasks.filter((t) => t !== task);
  const closed: OutboundA2aTaskSummary = {
    ...task,
    state: "canceled",
    open: false,
    unreachable_since: null,
  };
  state.broadcast({ type: "session_outbound_a2a_task", task: closed });
  json(res, 200, closed);
}

/** The port of the mock's A2A listener. */
const A2A_PORT = 7702;

const a2aRoutes: readonly Route[] = [
  {
    method: "GET",
    pattern: "/api/a2a/status",
    handler: ({ res, state }) => {
      // The mock has no relay and no address of the user's own: the agent is reachable locally.
      json(res, 200, {
        enabled: true,
        port: A2A_PORT,
        visibility: "public",
        public_url: null,
        local_url: `http://127.0.0.1:${A2A_PORT}/agents/${state.agentName}`,
        relay_access: false,
        relay_access_note:
          "Reachable locally. Connect to the Residuum relay in Cloud settings to make it reachable from other places, or set an address of your own below if you run your own tunnel.",
        listener_running: true,
        card_error: null,
      } satisfies A2aStatusResponse);
    },
  },
  {
    method: "GET",
    pattern: "/api/a2a/card",
    handler: ({ res, state }) => {
      const card = parseJsonObject(state.workspaceFileContents["config/agent-card.json"] ?? "{}");
      json(res, 200, {
        name: stringField(card, "name") ?? "Residuum agent",
        description: stringField(card, "description") ?? "",
        skills: Array.isArray(card.skills) ? (card.skills as A2aCardSkill[]) : [],
      } satisfies A2aAgentCard);
    },
  },
  {
    method: "GET",
    pattern: "/api/a2a/keys",
    handler: ({ res, state }) => {
      const keys = [...state.a2aKeys.entries()]
        .sort(([a], [b]) => a.localeCompare(b))
        .map(([name, k]) => ({ name, description: k.description, created_at: k.created_at }));
      json(res, 200, { keys } satisfies A2aKeysListResponse);
    },
  },
  {
    method: "POST",
    pattern: "/api/a2a/keys",
    handler: async ({ req, res, state }) => {
      const body = await readJsonObject(req);
      const name = stringField(body, "name") ?? "";
      if (!KEY_NAME.test(name)) {
        json(res, 400, { error: `caller key name '${name}' is invalid` });
        return;
      }
      if (state.a2aKeys.has(name)) {
        json(res, 409, { error: `an A2A caller key named '${name}' already exists` });
        return;
      }
      state.a2aKeys.set(name, {
        description: stringField(body, "description") ?? "",
        created_at: new Date().toISOString(),
      });
      json(res, 200, {
        name,
        token: `rsdm_a2a_mock${Math.random().toString(36).slice(2, 10)}`,
      } satisfies CreateA2aKeyResponse);
    },
  },
  {
    method: "DELETE",
    pattern: /^\/api\/a2a\/keys\/(.+)$/,
    handler: (ctx) => {
      const name = decodedParam(ctx, 0);
      if (!ctx.state.a2aKeys.delete(name)) {
        json(ctx.res, 404, { error: `no A2A caller key named '${name}'` });
        return;
      }
      json(ctx.res, 200, { revoked: true, checkpoint_id: null });
    },
  },
  {
    method: "GET",
    pattern: "/api/a2a/agents",
    handler: ({ res }) => {
      json(res, 200, [
        {
          name: "research-buddy",
          url: "https://example.com/a2a/research-buddy",
          source: "config",
          status: "ok",
          error: null,
          card: {
            name: "Research Buddy",
            description: "Digs through papers and reports back with sources.",
            skills: [{ id: "lit-review", name: "Literature review" }],
          },
        },
        {
          name: "laptop",
          url: "https://example.com/a2a/laptop",
          source: "sibling",
          status: "pending",
          error: null,
          card: null,
        },
      ] satisfies A2aRemoteAgent[]);
    },
  },
  {
    method: "GET",
    pattern: "/api/a2a/outbound",
    handler: ({ res, state }) => {
      json(res, 200, state.outboundTasks);
    },
  },
  {
    method: "POST",
    pattern: /^\/api\/a2a\/outbound\/([^/]+)\/(stop|stop-watching)$/,
    handler: closeOutboundTask,
  },
  {
    method: "GET",
    pattern: "/api/a2a/agents/raw",
    handler: ({ res, state }) => {
      // The backend serves the file as it is, labeled JSON.
      res.writeHead(200, { "Content-Type": "application/json" });
      res.end(state.a2aAgentsJson);
    },
  },
  {
    method: "PUT",
    pattern: "/api/a2a/agents/raw",
    handler: async ({ req, res, state }) => {
      state.a2aAgentsJson = await readBody(req);
      json(res, 200, VALID);
    },
  },
];

// ─── Secrets ───────────────────────────────────────────────────────────────────

const secretRoutes: readonly Route[] = [
  {
    method: "GET",
    pattern: "/api/secrets",
    handler: ({ res, state }) => {
      json(res, 200, { names: [...state.secrets.keys()] } satisfies SecretsListResponse);
    },
  },
  {
    method: "POST",
    pattern: "/api/secrets",
    handler: async ({ req, res, state }) => {
      const body = await readJsonObject(req);
      const name = stringField(body, "name") ?? "";
      state.secrets.set(name, stringField(body, "value") ?? "");
      json(res, 200, { reference: `secret:${name}` } satisfies SecretResponse);
    },
  },
  {
    method: "DELETE",
    pattern: /^\/api\/secrets\/(.+)$/,
    handler: (ctx) => {
      ctx.state.secrets.delete(decodedParam(ctx, 0));
      json(ctx.res, 200, { deleted: true } satisfies DeleteSecretResponse);
    },
  },
];

/**
 * Status, config documents, providers, MCP, setup completion, agent keys,
 * A2A, secrets and tracing, in the unscoped `/api/...` spelling. Agent
 * requests run against that agent's state; hub requests (`/api/hub/config/...`,
 * secrets) against the hub's.
 */
export const configRoutes: readonly Route[] = [
  ...systemRoutes,
  ...tomlDocumentRoutes("/api/config", "configToml"),
  ...tomlDocumentRoutes("/api/hub/config", "hubConfigToml", ({ hub }) => {
    hub.reloadHubConfig();
  }),
  { method: "POST", pattern: "/api/hub/config/complete-setup", handler: completeSetup },
  ...providerRoutes,
  ...mcpRoutes,
  ...agentKeyRoutes,
  ...a2aRoutes,
  ...secretRoutes,
];
