/**
 * Vite plugin that mocks all Residuum REST endpoints and WebSocket connections.
 * Activated when VITE_MOCK=1 is set (via `npm run dev:mock`).
 *
 * State is held in-memory for the duration of the dev server session.
 * Nothing persists across restarts.
 */

import type { Plugin, ViteDevServer } from "vite";
import type { IncomingMessage, ServerResponse } from "node:http";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { WebSocketServer, WebSocket } from "ws";

// ─── In-memory state ───────────────────────────────────────────────────────────

interface MockState {
  mode: "setup" | "running";
  secrets: Map<string, string>;
  configToml: string;
  providersToml: string;
  mcpJson: string;
  workspaceFiles: Record<string, Array<{ name: string; entry_type: string; size: number | null }>>;
  workspaceFileContents: Record<string, string>;
}

function loadAsset(filename: string): string {
  try {
    return readFileSync(resolve(__dirname, "..", "assets", filename), "utf-8");
  } catch {
    return `# Could not load ${filename}`;
  }
}

function createState(): MockState {
  return {
    mode: process.env.VITE_MOCK_SETUP === "1" ? "setup" : "running",
    secrets: new Map([
      ["anthropic_key", "sk-ant-mock-xxxx"],
      ["openai_key", "sk-mock-xxxx"],
    ]),
    configToml: loadAsset("config.example.toml"),
    providersToml: loadAsset("providers.example.toml"),
    mcpJson: loadAsset("mcp.example.json"),
    workspaceFiles: {
      "": [
        { name: "SOUL.md", entry_type: "file", size: 847 },
        { name: "AGENTS.md", entry_type: "file", size: 523 },
        { name: "USER.md", entry_type: "file", size: 312 },
        { name: "MEMORY.md", entry_type: "file", size: 1205 },
        { name: "ENVIRONMENT.md", entry_type: "file", size: 689 },
        { name: "PRESENCE.toml", entry_type: "file", size: 245 },
        { name: "HEARTBEAT.yml", entry_type: "file", size: 178 },
        { name: "CHANNELS.yml", entry_type: "file", size: 392 },
        { name: "memory", entry_type: "directory", size: null },
        { name: "skills", entry_type: "directory", size: null },
        { name: "projects", entry_type: "directory", size: null },
        { name: "config", entry_type: "directory", size: null },
        { name: "inbox", entry_type: "directory", size: null },
        { name: "subagents", entry_type: "directory", size: null },
        { name: "archive", entry_type: "directory", size: null },
      ],
      "skills": [
        { name: "research", entry_type: "directory", size: null },
        { name: "code-review", entry_type: "directory", size: null },
      ],
      "skills/research": [
        { name: "SKILL.md", entry_type: "file", size: 634 },
        { name: "prompt.md", entry_type: "file", size: 1102 },
      ],
      "skills/code-review": [
        { name: "SKILL.md", entry_type: "file", size: 478 },
      ],
      "projects": [
        { name: "residuum", entry_type: "directory", size: null },
      ],
      "projects/residuum": [
        { name: "context.md", entry_type: "file", size: 2340 },
      ],
      "config": [
        { name: "mcp.json", entry_type: "file", size: 1567 },
        { name: "channels.toml", entry_type: "file", size: 834 },
      ],
      "memory": [
        { name: "observations.jsonl", entry_type: "file", size: 45230 },
        { name: "reflections.jsonl", entry_type: "file", size: 12450 },
      ],
      "inbox": [],
      "subagents": [],
      "archive": [],
    },
    workspaceFileContents: {
      "SOUL.md": "# Soul\n\nI am Residuum, a personal AI agent framework designed for long-running autonomous operation.\n\n## Core Identity\n\n- I maintain persistent memory across conversations\n- I operate with genuine agency, not just reactivity\n- I respect my operator's preferences and working style\n- I am transparent about my capabilities and limitations\n\n## Values\n\n- **Honesty**: I never fabricate information or hide errors\n- **Autonomy**: I take initiative when appropriate\n- **Memory**: I remember and build on past interactions\n- **Craft**: I strive for quality in everything I produce\n",
      "AGENTS.md": "# Agents\n\n## Active Agents\n\n### Observer\nMonitors context window usage and triggers memory extraction.\n- Threshold: 30,000 tokens\n- Frequency: Checked after each turn\n\n### Reflector\nSynthesizes observations into higher-level reflections.\n- Threshold: 40,000 tokens\n- Minimum observations: 5\n\n### Pulse\nRuns periodic system health checks.\n- Interval: 5 minutes\n- Reports: memory stats, token usage, active tasks\n",
      "USER.md": "# User Profile\n\n- **Name**: Bear\n- **Timezone**: America/New_York\n- **Preferred communication**: Direct and concise\n- **Working hours**: Flexible, mostly evenings\n",
      "MEMORY.md": "# Memory Index\n\n## Recent Observations\n- 42 observations stored\n- 8 reflections synthesized\n- Last compaction: 2026-03-09\n\n## Key Topics\n- Notification routing system design\n- Kubernetes deployment pipeline\n- Memory search optimization\n",
      "ENVIRONMENT.md": "# Environment\n\n## System\n- **OS**: Pop!_OS 24.04 LTS\n- **Arch**: x86_64\n- **Shell**: Fish\n\n## Development\n- Rust 1.84.0\n- Node.js 22.x\n- Docker 29.1.3\n",
      "PRESENCE.toml": '[presence]\nstatus = "active"\nlast_seen = "2026-03-10T14:30:00Z"\n\n[presence.channels]\nweb = true\ndiscord = false\ntelegram = true\n',
      "HEARTBEAT.yml": "interval_seconds: 300\nchecks:\n  - memory_usage\n  - token_count\n  - active_tasks\n  - channel_status\nlast_beat: \"2026-03-10T14:30:00Z\"\nstatus: healthy\n",
      "CHANNELS.yml": "channels:\n  web:\n    enabled: true\n    priority: high\n  discord:\n    enabled: false\n    token_ref: \"secret:discord_token\"\n  telegram:\n    enabled: true\n    token_ref: \"secret:telegram_token\"\n    chat_id: \"123456789\"\n",
      "skills/research/SKILL.md": "# Research Skill\n\n## Purpose\nConduct thorough research on topics using available tools and memory.\n\n## Triggers\n- User asks to \"research\" or \"look into\" a topic\n- User asks for comprehensive analysis\n\n## Process\n1. Search memory for existing knowledge\n2. Use web search if available\n3. Synthesize findings\n4. Store key observations\n",
      "skills/research/prompt.md": "You are conducting research on the following topic: {{topic}}\n\n## Guidelines\n- Search memory first for existing knowledge\n- Use web search tools if available\n- Cross-reference multiple sources\n- Note confidence levels for each finding\n- Store important observations for future reference\n\n## Output Format\n- Summary (2-3 sentences)\n- Key findings (bulleted list)\n- Sources and confidence levels\n- Suggested follow-up questions\n",
      "skills/code-review/SKILL.md": "# Code Review Skill\n\n## Purpose\nReview code changes for quality, correctness, and style.\n\n## Triggers\n- User asks for code review\n- PR review requests\n\n## Checklist\n- [ ] Logic correctness\n- [ ] Error handling\n- [ ] Style consistency\n- [ ] Test coverage\n- [ ] Security considerations\n",
      "projects/residuum/context.md": "# Residuum Project Context\n\n## Overview\nPersonal agent framework written in Rust with a web UI.\n\n## Current Focus\n- Workspace file browser implementation\n- Memory search optimization\n- Notification routing system\n\n## Architecture\n- Backend: Rust + Axum\n- Frontend: Svelte 5 + TypeScript\n- Storage: SQLite + file-based workspace\n- LLM: Multi-provider support (Anthropic, OpenAI, Gemini, Ollama)\n",
      "config/mcp.json": '{\n  "servers": {\n    "filesystem": {\n      "command": "mcp-filesystem",\n      "args": ["--root", "/home/user/projects"]\n    }\n  }\n}',
      "config/channels.toml": '[web]\nenabled = true\nport = 3001\n\n[discord]\nenabled = false\ntoken_ref = "secret:discord_token"\n\n[telegram]\nenabled = true\ntoken_ref = "secret:telegram_token"\nchat_id = "123456789"\n',
      "memory/observations.jsonl": '{"text":"User prefers concise communication","timestamp":"2026-03-09T10:00:00Z","score":0.92}\n{"text":"Notification routing: Discord for urgent, Telegram for daily","timestamp":"2026-03-08T14:30:00Z","score":0.89}\n',
      "memory/reflections.jsonl": '{"text":"User is building a personal agent framework focused on genuine autonomy and persistent memory","timestamp":"2026-03-09T12:00:00Z","observations":5}\n',
    },
  };
}

// ─── Sample data ───────────────────────────────────────────────────────────────

function sampleChatHistory() {
  const now = new Date().toISOString();
  return [
    {
      role: "system",
      content: "Session started. Loaded 42 memory observations.",
      timestamp: new Date(Date.now() - 600_000).toISOString(),
      project_context: "default",
      visibility: "user",
    },
    {
      role: "user",
      content: "What did we discuss yesterday about the notification system?",
      timestamp: new Date(Date.now() - 300_000).toISOString(),
      project_context: "default",
      visibility: "user",
    },
    {
      role: "assistant",
      content:
        "Let me search my memory for our previous discussion about notifications.",
      tool_calls: [
        {
          id: "tc_mock_1",
          name: "memory_search",
          arguments: JSON.stringify({
            query: "notification system discussion",
            limit: 5,
          }),
        },
      ],
      timestamp: new Date(Date.now() - 299_000).toISOString(),
      project_context: "default",
      visibility: "user",
    },
    {
      role: "tool",
      content: JSON.stringify([
        {
          text: "Discussed routing notifications to Discord for high-priority items and Telegram for daily summaries.",
          score: 0.89,
          timestamp: "2026-03-08T14:30:00Z",
        },
        {
          text: "User wants notification channels configurable per-project context.",
          score: 0.82,
          timestamp: "2026-03-08T14:35:00Z",
        },
      ]),
      tool_call_id: "tc_mock_1",
      timestamp: new Date(Date.now() - 298_000).toISOString(),
      project_context: "default",
      visibility: "user",
    },
    {
      role: "assistant",
      content:
        "Yesterday we discussed the **notification routing system**. Here's a summary:\n\n" +
        "1. **Channel routing** — High-priority notifications go to Discord, daily summaries to Telegram\n" +
        "2. **Per-project config** — Each project context can have its own notification preferences\n" +
        "3. **Priority levels** — We defined three tiers: `urgent`, `normal`, and `low`\n\n" +
        "We also talked about adding a webhook endpoint for external integrations. " +
        "Would you like to continue working on any of these?",
      timestamp: new Date(Date.now() - 295_000).toISOString(),
      project_context: "default",
      visibility: "user",
    },
    {
      role: "user",
      content: "Can you check the current memory stats?",
      timestamp: new Date(Date.now() - 120_000).toISOString(),
      project_context: "default",
      visibility: "user",
    },
    {
      role: "assistant",
      content:
        "Let me look at the memory subsystem status.",
      tool_calls: [
        {
          id: "tc_mock_2",
          name: "server_command",
          arguments: JSON.stringify({ name: "context" }),
        },
      ],
      timestamp: new Date(Date.now() - 119_000).toISOString(),
      project_context: "default",
      visibility: "user",
    },
    {
      role: "tool",
      content:
        "Context window: 12,847 / 200,000 tokens (6.4%)\nMemory observations: 42\nReflections: 8\nLast observer run: 3 minutes ago",
      tool_call_id: "tc_mock_2",
      timestamp: new Date(Date.now() - 118_000).toISOString(),
      project_context: "default",
      visibility: "user",
    },
    {
      role: "assistant",
      content:
        "Here are the current memory stats:\n\n" +
        "- **Context window**: 12,847 / 200,000 tokens (6.4%)\n" +
        "- **Observations**: 42 stored\n" +
        "- **Reflections**: 8 synthesized\n" +
        "- **Last observer run**: 3 minutes ago\n\n" +
        "The context is well within limits. The observer will run again once we hit the 30k token threshold.",
      timestamp: new Date(Date.now() - 117_000).toISOString(),
      project_context: "default",
      visibility: "user",
    },
  ];
}

const cannedResponses = [
  "I've looked into that and here's what I found:\n\n" +
    "## Key Points\n\n" +
    "1. **Configuration** — The settings are stored in `config.toml` under the `[memory]` section\n" +
    "2. **Thresholds** — Observer triggers at 30k tokens, reflector at 40k\n" +
    "3. **Search** — Hybrid BM25 + vector search with configurable weights\n\n" +
    "```toml\n[memory]\nobserver_threshold_tokens = 30000\nreflector_threshold_tokens = 40000\n```\n\n" +
    "Would you like me to adjust any of these values?",

  "Great question! Let me break that down:\n\n" +
    "The notification system supports **three channels**:\n\n" +
    "- **Discord** — Real-time alerts via bot DM\n" +
    "- **Telegram** — Daily digest summaries\n" +
    "- **Webhook** — Custom HTTP POST for external integrations\n\n" +
    "Each channel can be configured independently per project context. " +
    "The priority routing rules determine which channel receives which notifications.\n\n" +
    "> **Tip**: Use `secret:discord_token` syntax in your config to reference encrypted secrets.",

  "I've completed the analysis. Here's a summary:\n\n" +
    "### Performance Metrics\n\n" +
    "| Metric | Value | Status |\n" +
    "|--------|-------|--------|\n" +
    "| Response time | 1.2s avg | Good |\n" +
    "| Memory usage | 45MB | Normal |\n" +
    "| Token throughput | 850/s | Optimal |\n\n" +
    "Everything looks healthy. The memory subsystem is operating within expected parameters. " +
    "Let me know if you'd like a deeper dive into any specific area.",
];

const modelsByProvider: Record<
  string,
  Array<{ id: string; name: string }>
> = {
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
  ollama: [
    { id: "llama3.3:latest", name: "Llama 3.3" },
    { id: "mistral:latest", name: "Mistral" },
    { id: "deepseek-r1:latest", name: "DeepSeek R1" },
    { id: "qwen3:latest", name: "Qwen 3" },
  ],
};

// ─── Helpers ───────────────────────────────────────────────────────────────────

function readBody(req: IncomingMessage): Promise<string> {
  return new Promise((resolve) => {
    let data = "";
    req.on("data", (chunk: Buffer) => {
      data += chunk.toString();
    });
    req.on("end", () => resolve(data));
  });
}

function json(res: ServerResponse, status: number, body: unknown) {
  res.writeHead(status, { "Content-Type": "application/json" });
  res.end(JSON.stringify(body));
}

function text(res: ServerResponse, status: number, body: string) {
  res.writeHead(status, { "Content-Type": "text/plain" });
  res.end(body);
}

// ─── REST middleware ───────────────────────────────────────────────────────────

function setupRestMiddleware(server: ViteDevServer, state: MockState) {
  server.middlewares.use(async (req: IncomingMessage, res: ServerResponse, next: () => void) => {
    const url = req.url ?? "";
    const method = req.method ?? "GET";

    // Only intercept /api/* routes
    if (!url.startsWith("/api")) {
      next();
      return;
    }

    // Strip query string for matching
    const path = url.split("?")[0];

    try {
      // ── Status & system ────────────────────────────────────────────────
      if (path === "/api/status" && method === "GET") {
        json(res, 200, { mode: state.mode });
        return;
      }

      if (path === "/api/system/timezone" && method === "GET") {
        json(res, 200, { timezone: "America/New_York" });
        return;
      }

      // ── Chat ───────────────────────────────────────────────────────────
      if (path === "/api/chat/history" && method === "GET") {
        json(res, 200, sampleChatHistory());
        return;
      }

      // ── Config ─────────────────────────────────────────────────────────
      if (path === "/api/config/raw" && method === "GET") {
        text(res, 200, state.configToml);
        return;
      }

      if (path === "/api/config/raw" && method === "PUT") {
        state.configToml = await readBody(req);
        json(res, 200, { valid: true });
        return;
      }

      if (path === "/api/config/validate" && method === "POST") {
        json(res, 200, { valid: true });
        return;
      }

      if (path === "/api/config/complete-setup" && method === "POST") {
        const body = JSON.parse(await readBody(req));
        state.configToml = body.config ?? state.configToml;
        state.providersToml = body.providers ?? state.providersToml;
        if (body.mcp_json) {
          state.mcpJson = body.mcp_json;
        }
        state.mode = "running";
        json(res, 200, { valid: true });
        return;
      }

      // ── Providers ──────────────────────────────────────────────────────
      if (path === "/api/providers/raw" && method === "GET") {
        text(res, 200, state.providersToml);
        return;
      }

      if (path === "/api/providers/raw" && method === "PUT") {
        state.providersToml = await readBody(req);
        json(res, 200, { valid: true });
        return;
      }

      if (path === "/api/providers/validate" && method === "POST") {
        json(res, 200, { valid: true });
        return;
      }

      if (path === "/api/providers/models" && method === "POST") {
        const body = JSON.parse(await readBody(req));
        const provider = (body.provider ?? "").toLowerCase();

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
        json(res, 200, { models });
        return;
      }

      // ── MCP ────────────────────────────────────────────────────────────
      if (path === "/api/mcp/raw" && method === "GET") {
        text(res, 200, state.mcpJson);
        return;
      }

      if (path === "/api/mcp/raw" && method === "PUT") {
        state.mcpJson = await readBody(req);
        json(res, 200, { valid: true });
        return;
      }

      if (path === "/api/mcp-catalog" && method === "GET") {
        try {
          const catalog = readFileSync(
            resolve(__dirname, "public", "mcp-catalog.json"),
            "utf-8",
          );
          res.writeHead(200, { "Content-Type": "application/json" });
          res.end(catalog);
        } catch {
          json(res, 200, []);
        }
        return;
      }

      // ── Secrets ────────────────────────────────────────────────────────
      if (path === "/api/secrets" && method === "GET") {
        json(res, 200, { names: [...state.secrets.keys()] });
        return;
      }

      if (path === "/api/secrets" && method === "POST") {
        const body = JSON.parse(await readBody(req));
        state.secrets.set(body.name, body.value);
        json(res, 200, { reference: `secret:${body.name}` });
        return;
      }

      // DELETE /api/secrets/:name
      const deleteMatch = path.match(/^\/api\/secrets\/(.+)$/);
      if (deleteMatch && method === "DELETE") {
        const name = decodeURIComponent(deleteMatch[1]);
        state.secrets.delete(name);
        json(res, 200, { deleted: true });
        return;
      }

      // ── Workspace ─────────────────────────────────────────────────────
      if (path === "/api/workspace/files" && method === "GET") {
        const urlObj = new URL(url, "http://localhost");
        const wsPath = urlObj.searchParams.get("path") ?? "";
        const entries = state.workspaceFiles[wsPath];
        if (entries) {
          json(res, 200, entries);
        } else {
          json(res, 200, []);
        }
        return;
      }

      if (path === "/api/workspace/file" && method === "GET") {
        const urlObj = new URL(url, "http://localhost");
        const filePath = urlObj.searchParams.get("path") ?? "";
        const content = state.workspaceFileContents[filePath];
        if (content !== undefined) {
          text(res, 200, content);
        } else {
          text(res, 404, "file not found");
        }
        return;
      }

      if (path === "/api/workspace/file" && method === "PUT") {
        const body = JSON.parse(await readBody(req));
        state.workspaceFileContents[body.path] = body.content;
        json(res, 200, { saved: true });
        return;
      }

      // ── Fallthrough ────────────────────────────────────────────────────
      json(res, 404, { error: `mock: unknown endpoint ${method} ${path}` });
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      json(res, 500, { error: `mock server error: ${message}` });
    }
  });
}

// ─── WebSocket handler ─────────────────────────────────────────────────────────

function setupWebSocket(server: ViteDevServer) {
  const httpServer = server.httpServer;
  if (!httpServer) return;

  const wss = new WebSocketServer({ noServer: true });
  let responseIndex = 0;

  httpServer.on("upgrade", (req, socket, head) => {
    const url = req.url ?? "";

    // Only handle /ws upgrades — Vite's HMR uses /__vite_hmr or /
    if (url === "/ws" || url.startsWith("/ws?")) {
      wss.handleUpgrade(req, socket, head, (ws) => {
        wss.emit("connection", ws, req);
      });
    }
    // Let other upgrades (Vite HMR) pass through — don't call socket.destroy()
  });

  wss.on("connection", (ws: WebSocket) => {
    ws.on("message", (raw: Buffer) => {
      let msg: { type: string; [key: string]: unknown };
      try {
        msg = JSON.parse(raw.toString());
      } catch {
        ws.send(JSON.stringify({ type: "error", reply_to: null, message: "invalid JSON" }));
        return;
      }

      switch (msg.type) {
        case "ping":
          ws.send(JSON.stringify({ type: "pong" }));
          break;

        case "send_message":
          simulateConversation(ws, msg);
          break;

        case "set_verbose":
          // Silent acknowledge — no response needed
          break;

        case "reload":
          ws.send(JSON.stringify({ type: "reloading" }));
          setTimeout(() => {
            ws.send(
              JSON.stringify({
                type: "notice",
                message: "Configuration reloaded successfully.",
              }),
            );
          }, 1000);
          break;

        case "server_command":
          ws.send(
            JSON.stringify({
              type: "notice",
              message: `Command '${msg.name}' executed. (mock)`,
            }),
          );
          break;

        case "inbox_add":
          ws.send(
            JSON.stringify({
              type: "notice",
              message: `Inbox item added: "${String(msg.body).slice(0, 50)}..."`,
            }),
          );
          break;

        default:
          ws.send(
            JSON.stringify({
              type: "error",
              reply_to: null,
              message: `unknown message type: ${msg.type}`,
            }),
          );
      }
    });
  });

  function simulateConversation(
    ws: WebSocket,
    msg: { type: string; [key: string]: unknown },
  ) {
    const replyTo = String(msg.id ?? "unknown");

    // 1. turn_started (immediate)
    ws.send(JSON.stringify({ type: "turn_started", reply_to: replyTo }));

    // 2. tool_call (300ms)
    const toolCallId = `tc_mock_${Date.now()}`;
    setTimeout(() => {
      if (ws.readyState !== WebSocket.OPEN) return;
      ws.send(
        JSON.stringify({
          type: "tool_call",
          id: toolCallId,
          name: "memory_search",
          arguments: JSON.stringify({
            query: String(msg.content).slice(0, 100),
            limit: 5,
          }),
        }),
      );
    }, 300);

    // 3. tool_result (800ms)
    setTimeout(() => {
      if (ws.readyState !== WebSocket.OPEN) return;
      ws.send(
        JSON.stringify({
          type: "tool_result",
          tool_call_id: toolCallId,
          name: "memory_search",
          output: JSON.stringify([
            {
              text: "Found 3 relevant observations from recent conversations.",
              score: 0.87,
              timestamp: new Date().toISOString(),
            },
          ]),
          is_error: false,
        }),
      );
    }, 800);

    // 4. response (1500ms)
    setTimeout(() => {
      if (ws.readyState !== WebSocket.OPEN) return;
      const response = cannedResponses[responseIndex % cannedResponses.length];
      responseIndex++;
      ws.send(
        JSON.stringify({
          type: "response",
          reply_to: replyTo,
          content: response,
        }),
      );
    }, 1500);
  }
}

// ─── Plugin export ─────────────────────────────────────────────────────────────

export function mockServerPlugin(): Plugin {
  return {
    name: "residuum-mock-server",
    configureServer(server) {
      const state = createState();

      setupRestMiddleware(server, state);
      setupWebSocket(server);

      const modeLabel = state.mode === "setup" ? "setup" : "running";
      console.log("");
      console.log("  [mock] API mock server active");
      console.log(
        `  [mock] Mode: ${modeLabel} (set VITE_MOCK_SETUP=1 for setup wizard)`,
      );
      console.log("  [mock] WebSocket echo server on /ws");
      console.log("");
    },
  };
}
