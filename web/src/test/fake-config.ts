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
  hub: string;
}

export interface FakeAgentConfig {
  handler: FetchHandler;
  requests: ConfigRequest[];
  files: FakeConfigFiles;
  models: { id: string; name: string }[];
}

const FILES = [
  { name: "config", path: "config", format: "toml" },
  { name: "providers", path: "providers", format: "toml" },
  { name: "mcp", path: "mcp", format: "json" },
  { name: "hub", path: "config", format: "toml" },
] as const;

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
      return new Promise<Response>(() => {});
    },
  };
  return fake;
}
