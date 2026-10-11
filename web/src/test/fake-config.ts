/**
 * A stand-in for the config endpoints of one agent and the hub, for tests: the
 * files live in memory, `PATCH` merges a diff the way the server does, and the
 * test can change a file the way something outside the page would.
 */
import { applyPatch } from "./apply-patch";
import { jsonResponse, type FetchHandler } from "./component";

export interface ConfigRequest {
  method: string;
  url: string;
  body: string | undefined;
}

export interface FakeConfigFiles {
  config: string;
  providers: string;
  mcp: string;
  /** The hub's `config.toml`. */
  hub: string;
}

export interface FakeAgentConfig {
  handler: FetchHandler;
  /** Every request the page made, in order. */
  requests: ConfigRequest[];
  /** What the files hold now. A test changes one to change it from outside. */
  files: FakeConfigFiles;
  /** The models the provider lists. */
  models: { id: string; name: string }[];
}

/** Where each file is served, under `/api/agents/{agent}` or, for the hub's, `/api/hub`. */
const FILES = [
  { name: "config", path: "config", format: "toml" },
  { name: "providers", path: "providers", format: "toml" },
  { name: "mcp", path: "mcp", format: "json" },
  { name: "hub", path: "config", format: "toml" },
] as const;

/**
 * The config endpoints of `agent` and the hub, answering from `files`. Give
 * each test its own agent name: the app's config coordinator remembers what it
 * has told subscribers about each agent's files.
 */
export function fakeAgentConfig(
  agent: string,
  initial: Partial<FakeConfigFiles> = {},
): FakeAgentConfig {
  const fake: FakeAgentConfig = {
    requests: [],
    files: { config: "", providers: "", mcp: '{"mcpServers":{}}', hub: "", ...initial },
    models: [
      { id: "claude-a", name: "Claude A" },
      { id: "claude-b", name: "Claude B" },
    ],
    handler: (url, init) => {
      const method = init?.method ?? "GET";
      const body = typeof init?.body === "string" ? init.body : undefined;
      fake.requests.push({ method, url, body });
      for (const { name, path, format } of FILES) {
        const base = name === "hub" ? "/api/hub" : `/api/agents/${agent}`;
        if (method === "GET" && url === `${base}/${path}/raw`) {
          return new Response(fake.files[name], { status: 200 });
        }
        if (method === "PUT" && url === `${base}/${path}/raw`) {
          fake.files[name] = body ?? "";
          return jsonResponse({ valid: true });
        }
        if (method === "PATCH" && url === `${base}/${path}/patch`) {
          const diff = JSON.parse(body ?? "{}") as Record<string, unknown>;
          fake.files[name] = applyPatch(fake.files[name], diff, format);
          return jsonResponse({ valid: true, checkpoint_id: "cp" });
        }
      }
      if (method === "POST" && url === `/api/agents/${agent}/providers/models`) {
        return jsonResponse({ models: fake.models });
      }
      // Whatever else the page asks for, such as the chat history the agent
      // socket loads, never answers.
      return new Promise<Response>(() => {});
    },
  };
  return fake;
}
