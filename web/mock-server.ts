/**
 * Vite plugin that mocks all Residuum REST endpoints and WebSocket connections.
 * Activated when VITE_MOCK=1 is set (via `npm run dev:mock`).
 *
 * State is held in-memory for the duration of the dev server session.
 * Nothing persists across restarts.
 *
 * The mock serves the multi-agent hub HTTP contract
 * (docs/systems-usage/hub-http.md): `/api/agents/{name}/...`,
 * `/api/hub/...` (lifecycle, hub config, secrets, and `/api/hub/ws`) and
 * `/api/team/...`. Each agent has its own state and its own WebSocket. The
 * hub-level and team-level data (secrets, hub config, team files, workbench)
 * live in one shared state.
 *
 * The sessions and config endpoints are route tables in `mock/` (see
 * `mock/routes.ts`); this file routes requests to them, and holds the hub,
 * agent sockets, chat, files, workbench, inbox and test controls.
 */

import type { Plugin, ViteDevServer } from "vite";
import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { WebSocketServer, WebSocket } from "ws";
import { agentNameProblem } from "./mock/agent-name";
import { configRoutes } from "./mock/config";
import { MOCK_CLOUD_STATUS, MOCK_FEATURES, MOCK_RESIDUUM_VERSION } from "./mock/constants";
import { json, readBody, text } from "./mock/http";
import { dispatchRoute } from "./mock/routes";
import { sendSessionMessage, sessionRoutes, spawnSession, stopSession } from "./mock/sessions";
import { createState, type MockAgent, type MockHub, type MockState } from "./mock/state";
import { byName } from "./mock/util";

// ─── Delays ────────────────────────────────────────────────────────────────────

/**
 * Model calls are slowed down so they're visibly "in flight" in an
 * artifact's activity panel for a moment, long enough to exercise Cancel
 * calls and Stop page by hand.
 */
const MODEL_CALL_DELAY_MS = 3000;

/**
 * Serve an artifact page the way the artifacts listener does: SDK injected,
 * with the artifact's name, the mock version, and the mock feature list
 * embedded for `residuum.artifact`, `residuum.version`, and `residuum.features`.
 */
function workbenchPage(html: string, artifactName: string): string {
  const sdk = readFileSync(resolve(__dirname, "..", "assets", "workbench", "sdk.js"), "utf-8");
  const context =
    `const __RESIDUUM_ARTIFACT__=${JSON.stringify(artifactName)};` +
    `const __RESIDUUM_VERSION__=${JSON.stringify(MOCK_RESIDUUM_VERSION)};` +
    `const __RESIDUUM_FEATURES__=${JSON.stringify(MOCK_FEATURES)};`;
  return html.replace("<head>", `<head><script>${context}${sdk}</script>`);
}

/**
 * A second origin for artifacts, like the gateway's artifacts listener:
 * `/{artifact}/` serves the artifact's page. Listens on any free port and
 * records it.
 */
function startMockArtifactsListener(state: MockState) {
  const server = createServer((req, res) => {
    const match = /^\/([a-z0-9-]+)\/(\?.*)?$/.exec(req.url ?? "");
    const name = match?.[1] ?? "";
    const artifact = match ? state.workbenchArtifacts.get(name) : undefined;
    if (!artifact) {
      res.writeHead(404, { "Content-Type": "text/plain" });
      res.end("There's no workbench artifact here.");
      return;
    }
    res.writeHead(200, { "Content-Type": "text/html; charset=utf-8", "Cache-Control": "no-store" });
    res.end(workbenchPage(artifact.html, name));
  });
  server.listen(0, "127.0.0.1", () => {
    const address = server.address();
    state.workbenchPort = typeof address === "object" && address !== null ? address.port : null;
    console.log(`  [mock] Workbench artifacts on http://localhost:${state.workbenchPort}`);
  });
}

// ─── Sample data ───────────────────────────────────────────────────────────────

// Produce a Date a given number of calendar days before "now" at a specific
// local hour, so the live recent messages span several days and exercise the
// day-divider logic in the frontend feed store.
function daysAgoAt(daysAgo: number, hour: number, minute = 0): string {
  const d = new Date();
  d.setDate(d.getDate() - daysAgo);
  d.setHours(hour, minute, 0, 0);
  return d.toISOString();
}

// Recent live messages — span today, yesterday, and the day before so the
// frontend inserts a day divider between each run. These correspond to the
// contents of recent_messages.json on the backend.
function sampleRecentMessages() {
  return [
    // Main's reply to the relay that closes ep-003: the turn began in that
    // episode, so it's shown once the episode loads.
    {
      role: "assistant",
      content: "Noted. The observer notes are in; I'll fold them into the memory doc.",
      timestamp: daysAgoAt(2, 9, 0),
      visibility: "background",
    },
    {
      role: "user",
      content: "Did the observer flag anything odd in last night's batch?",
      timestamp: daysAgoAt(2, 9, 12),
      visibility: "user",
    },
    {
      role: "assistant",
      content:
        "Nothing unusual. The observer compressed 14 messages into `ep-003` " +
        "around 02:00 local time and logged two fresh reflections. Memory " +
        "utilization is holding at ~38% of the context window.",
      timestamp: daysAgoAt(2, 9, 13),
      visibility: "user",
    },
    {
      role: "user",
      content: "Can you check the current memory stats?",
      timestamp: daysAgoAt(1, 14, 20),
      visibility: "user",
    },
    {
      role: "assistant",
      content: "Let me look at the memory subsystem status.",
      tool_calls: [
        {
          id: "tc_mock_stats",
          name: "server_command",
          arguments: JSON.stringify({ name: "context" }),
        },
      ],
      timestamp: daysAgoAt(1, 14, 20),
      visibility: "user",
    },
    {
      role: "tool",
      content:
        "Context window: 12,847 / 200,000 tokens (6.4%)\n" +
        "Memory observations: 42\n" +
        "Reflections: 8\n" +
        "Last observer run: 3 minutes ago",
      tool_call_id: "tc_mock_stats",
      timestamp: daysAgoAt(1, 14, 21),
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
        "The context is well within limits. The observer will run again " +
        "once we cross the 30k token threshold.",
      timestamp: daysAgoAt(1, 14, 22),
      visibility: "user",
    },
    {
      role: "user",
      content: "Good. Let's keep iterating on the notification routing doc.",
      timestamp: daysAgoAt(0, 10, 5),
      visibility: "user",
    },
    {
      role: "assistant",
      content:
        "Picking up where we left off. I've got the three-tier priority " +
        "model (`urgent`, `normal`, `low`) and the per-context channel " +
        "overrides drafted. Next up: the fallback behaviour when a channel " +
        "is unreachable. Want me to start there?",
      timestamp: daysAgoAt(0, 10, 6),
      visibility: "user",
    },
    // The owner pasting an agent header by hand: stays their own message.
    {
      role: "user",
      content:
        "[Agent Message from spawned-research-3f9a (spawned)]\n" +
        "Pasting this header myself to see what the UI does with it.",
      timestamp: daysAgoAt(0, 10, 10),
      visibility: "user",
    },
    // Background noise from before sessions existed: stays hidden.
    {
      role: "user",
      content: "Pulse check: inbox_check. Review the inbox for anything urgent.",
      timestamp: daysAgoAt(0, 10, 30),
      visibility: "background",
    },
    {
      role: "assistant",
      content: "HEARTBEAT_OK",
      timestamp: daysAgoAt(0, 10, 30),
      visibility: "background",
    },
    // A spawned session's relayed result and main's reply: shown.
    {
      role: "user",
      content:
        "[Agent Message from spawned-research-3f9a (spawned)]\n" +
        "Found three fallback strategies worth comparing:\n\n" +
        "1. **Retry with backoff** on the same channel, capped at 3 attempts.\n" +
        "2. **Cascade** to the next channel in the priority list.\n" +
        "3. **Park** the notification in the inbox and surface it on next contact.\n\n" +
        "Cascade is what most setups expect; parking is the safest default when every channel is down. " +
        "Sources and notes are in `team/wiki/notification-fallbacks.md`.",
      timestamp: daysAgoAt(0, 10, 41),
      visibility: "background",
      agent_sender: { address: "spawned-research-3f9a", category: "spawned" },
    },
    {
      role: "assistant",
      content:
        "The research session came back: cascade first, then park in the inbox if every " +
        "channel is down. I'll draft the fallback section that way.",
      timestamp: daysAgoAt(0, 10, 42),
      visibility: "background",
    },
  ];
}

// Sample episodes — older compressed history that the frontend fetches
// lazily as the user scrolls to the top. The cursor chain is
// ep-003 → ep-002 → ep-001 → null.
interface SampleEpisode {
  id: string;
  date: string; // YYYY-MM-DD
  messages: Array<{
    role: string;
    content: string;
    tool_calls?: Array<{ id: string; name: string; arguments: string }>;
    tool_call_id?: string;
    timestamp: string;
    visibility: string;
    agent_sender?: { address: string; category: string };
  }>;
}

function isoDateDaysAgo(daysAgo: number): string {
  const d = new Date();
  d.setDate(d.getDate() - daysAgo);
  return d.toISOString().slice(0, 10);
}

function sampleEpisodes(): SampleEpisode[] {
  return [
    {
      id: "ep-003",
      date: isoDateDaysAgo(3),
      messages: [
        {
          role: "user",
          content: "Walk me through what the observer actually stores vs. what it drops.",
          timestamp: `${isoDateDaysAgo(3)}T00:00:00.000Z`,
          visibility: "user",
        },
        {
          role: "assistant",
          content:
            "The observer keeps three things for each compression pass:\n\n" +
            "1. **Observations** — atomic facts extracted from the chat, stored in the memory index.\n" +
            "2. **Reflections** — higher-order patterns it synthesises across observations.\n" +
            "3. **Episode transcript** — the raw JSONL of the messages it compressed, tagged with the episode id.\n\n" +
            "What it drops is the _surface wording_ of the messages — it " +
            "remembers the substance but won't be able to quote verbatim.",
          timestamp: `${isoDateDaysAgo(3)}T00:00:00.000Z`,
          visibility: "user",
        },
        // Episodes don't record visibility; the structured sender marks this
        // as a session's message. Main's reply opens the recent segment.
        {
          role: "user",
          content:
            "[Agent Message from scheduled-observer-audit-2c41 (scheduled)]\n" +
            "Observer audit done: nothing was dropped that should have been kept.",
          timestamp: `${isoDateDaysAgo(3)}T00:00:00.000Z`,
          visibility: "user",
          agent_sender: { address: "scheduled-observer-audit-2c41", category: "scheduled" },
        },
      ],
    },
    {
      id: "ep-002",
      date: isoDateDaysAgo(5),
      messages: [
        {
          role: "user",
          content: "How do I direct you at a specific episode when we talk?",
          timestamp: `${isoDateDaysAgo(5)}T00:00:00.000Z`,
          visibility: "user",
        },
        {
          role: "assistant",
          content:
            "Reference the episode id directly — e.g. `ep-002` — and I'll " +
            "pull the relevant observations from memory. You can also scope " +
            "by date, which is often easier if you don't remember the id.",
          timestamp: `${isoDateDaysAgo(5)}T00:00:00.000Z`,
          visibility: "user",
        },
        {
          role: "user",
          content: "That's perfect. Let's make it visible in the UI too.",
          timestamp: `${isoDateDaysAgo(5)}T00:00:00.000Z`,
          visibility: "user",
        },
      ],
    },
    {
      id: "ep-001",
      date: isoDateDaysAgo(8),
      messages: [
        {
          role: "user",
          content: "First conversation of the week — let's set goals.",
          timestamp: `${isoDateDaysAgo(8)}T00:00:00.000Z`,
          visibility: "user",
        },
        {
          role: "assistant",
          content:
            "Three things on the board:\n\n" +
            "- Finish the lazy-loaded chat history feature.\n" +
            "- Tighten the notification routing doc.\n" +
            "- Revisit the observer thresholds once we have a week of data.\n\n" +
            "Anything missing?",
          timestamp: `${isoDateDaysAgo(8)}T00:00:00.000Z`,
          visibility: "user",
        },
      ],
    },
  ];
}

// Build a ChatHistorySegment envelope matching the Rust backend's
// `ChatHistorySegment` tagged union (see gateway/web/config.rs).
function sampleChatHistorySegment(state: MockState, cursor: string | null) {
  const episodes = sampleEpisodes();

  if (state.compressedAt !== null) {
    if (cursor === null) {
      return {
        kind: "recent",
        messages: state.extraRecent.slice(state.compressedAt),
        next_cursor: "ep-004",
      };
    }
    if (cursor === "ep-004") {
      const compressed = [
        ...sampleRecentMessages(),
        ...state.extraRecent.slice(0, state.compressedAt),
      ];
      return {
        kind: "episode",
        episode_id: "ep-004",
        date: isoDateDaysAgo(0),
        // Episodes don't record visibility.
        messages: compressed.map((m) => ({ ...m, visibility: "user" })),
        next_cursor: episodes[0]?.id ?? null,
      };
    }
  }

  if (cursor === null) {
    return {
      kind: "recent",
      messages: [...sampleRecentMessages(), ...state.extraRecent],
      next_cursor: episodes[0]?.id ?? null,
    };
  }

  const idx = episodes.findIndex((ep) => ep.id === cursor);
  if (idx === -1) {
    return null;
  }
  const ep = episodes[idx];
  const next = episodes[idx + 1]?.id ?? null;
  return {
    kind: "episode",
    episode_id: ep.id,
    date: ep.date,
    messages: ep.messages,
    next_cursor: next,
  };
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
    "Each channel can be configured independently. " +
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

// ─── Helpers ───────────────────────────────────────────────────────────────────

/** A stand-in for the file version token (`ETag`) the workspace API reports. */
function mockFileVersion(content: string): string {
  let hash = 0;
  for (let i = 0; i < content.length; i++) hash = (hash * 31 + content.charCodeAt(i)) >>> 0;
  return `"${hash.toString(16)}-${content.length}"`;
}

/** A workspace file read: the content, with its version as the `ETag` header. */
function fileRead(res: ServerResponse, content: string) {
  res.writeHead(200, { "Content-Type": "text/plain", ETag: mockFileVersion(content) });
  res.end(content);
}

// ─── REST middleware ───────────────────────────────────────────────────────────

function setupRestMiddleware(server: ViteDevServer, hub: MockHub) {
  server.middlewares.use(async (req: IncomingMessage, res: ServerResponse, next: () => void) => {
    const url = req.url ?? "";
    const method = req.method ?? "GET";

    // Only intercept /api/* routes
    if (!url.startsWith("/api")) {
      next();
      return;
    }

    // Strip query string for matching, but keep params for handlers that need them.
    const [rawPath = "", rawQuery = ""] = url.split("?");
    const query = new URLSearchParams(rawQuery);

    try {
      // Scope the request: an agent's routes run against that agent's state, hub
      // and team routes against the shared state, and each is handled below as
      // the unscoped route it used to be.
      const routed = await routeScoped(hub, req, res, rawPath, method);
      if (routed === "handled") return;
      const { state, path } = routed;

      // Areas whose routes live in `mock/` modules.
      const request = { req, res, hub, state, method, path, query };
      if (await dispatchRoute([...sessionRoutes, ...configRoutes], request)) return;

      // ── Chat ───────────────────────────────────────────────────────────
      if (path === "/api/chat/history" && method === "GET") {
        const cursor = query.get("episode");
        const segment = sampleChatHistorySegment(state, cursor);
        if (segment === null) {
          json(res, 404, { error: "episode not found" });
          return;
        }
        json(res, 200, segment);
        return;
      }

      // Simulate a session's result reaching main while the page is
      // disconnected: record it (and main's reply) in history, then drop
      // the sockets. The page should show it once it reconnects.
      if (path === "/api/mock/missed-relay" && method === "POST") {
        const now = new Date().toISOString();
        state.extraRecent.push(
          {
            role: "user",
            content:
              "[Agent Message from spawned-research-3f9a (spawned)]\nMissed while you were away: the fallback doc is drafted.",
            timestamp: now,
            visibility: "background",
            agent_sender: { address: "spawned-research-3f9a", category: "spawned" },
          },
          {
            role: "assistant",
            content: "The research session finished the fallback doc while you were disconnected.",
            timestamp: now,
            visibility: "background",
          },
        );
        state.dropSockets();
        json(res, 200, { ok: true });
        return;
      }

      // A teammate messages an agent (`?agent=atlas`): the message lands in its
      // main conversation, and the hub reports it unread until the web UI
      // opens that agent's socket.
      if (path === "/api/mock/teammate-message" && method === "POST") {
        const agent = hub.agents.get(query.get("agent") ?? "");
        if (!agent) {
          json(res, 404, { error: "mock: name an agent with ?agent=" });
          return;
        }
        const now = new Date().toISOString();
        const from = query.get("from") ?? "scout";
        const reply = `${from} asked me to check the wiki index. On it.`;
        agent.state.extraRecent.push(
          {
            role: "user",
            content: `[Message from ${from}]\nCan you look over the wiki index when you get a chance?`,
            timestamp: now,
            visibility: "user",
          },
          { role: "assistant", content: reply, timestamp: now, visibility: "user" },
        );
        agent.state.broadcast({ type: "response", reply_to: "teammate", content: reply });
        if (agent.connectedClients() === 0) hub.addUnread(agent);
        json(res, 200, { ok: true });
        return;
      }

      // ── Workbench ─────────────────────────────────────────────────────
      if (path === "/api/workbench/artifacts" && method === "GET") {
        json(
          res,
          200,
          [...state.workbenchArtifacts].map(([name, artifact]) => ({
            name,
            title: /<title>([^<]*)<\/title>/i.exec(artifact.html)?.[1]?.trim() || name,
            modified_at: artifact.modifiedAt,
            size: artifact.html.length,
          })),
        );
        return;
      }

      if (path === "/api/workbench/info" && method === "GET") {
        json(res, 200, {
          port: state.workbenchPort,
          unavailable_reason:
            state.workbenchPort === null ? "The mock artifacts listener isn't up yet." : null,
          relay: null,
        });
        return;
      }

      const artifactMatch = path.match(/^\/api\/workbench\/artifacts\/([^/]+)$/);
      if (artifactMatch) {
        const name = decodeURIComponent(artifactMatch[1] ?? "");
        if (method === "DELETE") {
          if (!state.workbenchArtifacts.delete(name)) {
            text(res, 404, "That artifact no longer exists. It may already have been deleted.");
            return;
          }
          json(res, 200, { removed: [`${name}.html`] });
          return;
        }
      }

      // ── Model calls ──────────────────────────────────────────────────
      if (path === "/api/model/complete" && method === "POST") {
        const body = JSON.parse(await readBody(req));
        const prompt: string = body.prompt ?? body.messages?.at(-1)?.content ?? "";
        if (!prompt.trim() && !body.messages?.length) {
          json(res, 400, { error: 'A model call needs a "prompt" or "messages".' });
          return;
        }
        const content = `Mock model reply to: ${prompt.slice(0, 200)}`;
        // A brief artificial delay, so a call is visibly "in flight" in the
        // artifact activity panel long enough to see and, if wanted, cancel.
        await new Promise((done) => setTimeout(done, MODEL_CALL_DELAY_MS));
        json(res, 200, {
          content,
          model: "mock/small",
          usage: { input_tokens: prompt.length, output_tokens: content.length },
        });
        return;
      }

      // ── Inbox ─────────────────────────────────────────────────────────
      if (path === "/api/inbox" && method === "GET") {
        json(res, 200, state.inboxItems);
        return;
      }

      const readMatch = path.match(/^\/api\/inbox\/(.+)\/read$/);
      if (readMatch && method === "PUT") {
        const id = decodeURIComponent(readMatch[1]);
        const item = state.inboxItems.find((i) => i.id === id);
        if (item) {
          item.read = true;
          json(res, 200, item);
        } else {
          json(res, 404, { error: "not found" });
        }
        return;
      }

      const archiveMatch = path.match(/^\/api\/inbox\/(.+)\/archive$/);
      if (archiveMatch && method === "POST") {
        const id = decodeURIComponent(archiveMatch[1]);
        state.inboxItems = state.inboxItems.filter((i) => i.id !== id);
        json(res, 200, {});
        return;
      }

      // ── Workspace ─────────────────────────────────────────────────────
      if (path === "/api/workspace/files" && method === "GET") {
        const urlObj = new URL(url, "http://localhost");
        const wsPath = urlObj.searchParams.get("path") ?? "";
        const entries = filesFor(hub, state, wsPath).workspaceFiles[wsPath];
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
        const content = filesFor(hub, state, filePath).workspaceFileContents[filePath];
        if (content !== undefined) {
          fileRead(res, content);
        } else {
          text(res, 404, "file not found");
        }
        return;
      }

      if (path === "/api/workspace/file" && method === "PUT") {
        const body = JSON.parse(await readBody(req));
        filesFor(hub, state, body.path).workspaceFileContents[body.path] = body.content;
        json(res, 200, { saved: true, version: mockFileVersion(String(body.content)) });
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

function setupWebSocket(server: ViteDevServer, hub: MockHub, agent: MockAgent) {
  const state = agent.state;
  const httpServer = server.httpServer;
  if (!httpServer) return;

  const wss = new WebSocketServer({ noServer: true });
  let responseIndex = 0;
  const wsPath = `/api/agents/${agent.name}/ws`;

  httpServer.on("upgrade", (req, socket, head) => {
    const url = req.url ?? "";

    // Only handle this agent's socket — Vite's HMR uses /__vite_hmr or /
    if (url === wsPath || url.startsWith(`${wsPath}?`)) {
      if (agent.runState !== "running") {
        const body = JSON.stringify({
          error: `${agent.name} is ${agent.runState}`,
          state: agent.runState,
        });
        socket.end(
          `HTTP/1.1 409 Conflict\r\nContent-Type: application/json\r\nContent-Length: ${Buffer.byteLength(body)}\r\nConnection: close\r\n\r\n${body}`,
        );
        return;
      }
      wss.handleUpgrade(req, socket, head, (ws) => {
        wss.emit("connection", ws, req);
      });
    }
    // Let other upgrades (Vite HMR) pass through — don't call socket.destroy()
  });

  const broadcast = (frame: Record<string, unknown>) => {
    const data = JSON.stringify(frame);
    for (const client of wss.clients) {
      if (client.readyState === WebSocket.OPEN) client.send(data);
    }
  };
  state.dropSockets = () => {
    for (const client of wss.clients) client.terminate();
  };
  state.broadcast = broadcast;

  agent.connectedClients = () => wss.clients.size;

  wss.on("connection", (ws: WebSocket) => {
    // Opening the agent's socket is what shows its messages.
    hub.clearUnread(agent);
    ws.on("message", (raw: Buffer) => {
      let msg: { type: string; [key: string]: unknown };
      try {
        msg = JSON.parse(raw.toString());
      } catch {
        ws.send(
          JSON.stringify({ type: "error", reply_to: null, message: "invalid JSON", details: null }),
        );
        return;
      }

      switch (msg.type) {
        case "ping":
          ws.send(JSON.stringify({ type: "pong" }));
          break;

        case "send_message":
          if (String(msg.content).toLowerCase().startsWith("spawn")) {
            spawnSession(state, String(msg.content).replace(/^spawn\s*/i, "") || "Look into something");
          }
          simulateConversation(msg);
          break;

        case "set_verbose":
          // Silent acknowledge — no response needed
          break;

        case "watch_workspace":
          // The mock has no workspace to watch, so no change frames follow.
          break;

        case "session_send_message":
          sendSessionMessage(
            state,
            (frame) => ws.send(JSON.stringify(frame)),
            String(msg.id),
            String(msg.address),
            String(msg.content),
          );
          break;

        case "session_stop":
          stopSession(
            state,
            (frame) => ws.send(JSON.stringify(frame)),
            String(msg.id),
            String(msg.address),
          );
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
              details: null,
            }),
          );
      }
    });
  });

  /**
   * Run a main-agent turn: live frames to every connected page, then the
   * whole turn recorded in history when it ends, as the real gateway does.
   *
   * A message starting with "drop" loses the connection mid-turn:
   * - "drop finish": the turn ends while disconnected; history has it on reconnect.
   * - "drop compress": as "drop", and the observer compresses history into a
   *   new episode meanwhile, so the page has to reload history.
   * - "drop" (anything else): the turn is still running at reconnect and
   *   finishes live afterwards.
   */
  function simulateConversation(msg: { type: string; [key: string]: unknown }) {
    const replyTo = String(msg.id ?? "unknown");
    const content = String(msg.content ?? "");
    const lower = content.toLowerCase();
    const drop = lower.startsWith("drop");
    const finishWhileDown = lower.startsWith("drop finish");
    const compress = lower.startsWith("drop compress");
    const toolCallId = `tc_mock_${Date.now()}`;
    const toolArgs = { query: content.slice(0, 100), limit: 5 };
    const toolOutput = JSON.stringify([
      {
        text: "Found 3 relevant observations from recent conversations.",
        score: 0.87,
        timestamp: new Date().toISOString(),
      },
    ]);
    const response = cannedResponses[responseIndex % cannedResponses.length];
    responseIndex++;
    // Live frames stop while the connection is down.
    let down = false;
    const send = (frame: Record<string, unknown>) => {
      if (!down) broadcast(frame);
    };

    send({ type: "turn_started", reply_to: replyTo });
    hub.setBusy(agent, true);
    setTimeout(() => {
      send({ type: "broadcast_response", content: "Looking through recent notes first." });
      send({
        type: "tool_call",
        id: toolCallId,
        name: "memory_search",
        arguments: JSON.stringify(toolArgs),
      });
    }, 300);

    if (drop) {
      setTimeout(() => {
        down = true;
        if (compress) state.compressedAt = state.extraRecent.length;
        state.dropSockets();
      }, 600);
      // Reconnected by the time a still-running turn finishes.
      if (!finishWhileDown) {
        setTimeout(() => {
          down = false;
        }, 3500);
      }
    }

    const finishAt = drop ? (finishWhileDown ? 900 : 4000) : 1500;
    setTimeout(() => {
      send({
        type: "tool_result",
        tool_call_id: toolCallId,
        name: "memory_search",
        output: toolOutput,
        is_error: false,
      });
      send({ type: "response", reply_to: replyTo, content: response });
      send({ type: "turn_ended", reply_to: replyTo });
      hub.setBusy(agent, false);
      if (wss.clients.size === 0) hub.addUnread(agent);
      const now = new Date().toISOString();
      state.extraRecent.push(
        { role: "user", content, timestamp: now, visibility: "user" },
        {
          role: "assistant",
          content: "Looking through recent notes first.",
          tool_calls: [{ id: toolCallId, name: "memory_search", arguments: toolArgs }],
          timestamp: now,
          visibility: "user",
        },
        {
          role: "tool",
          content: toolOutput,
          tool_call_id: toolCallId,
          timestamp: now,
          visibility: "user",
        },
        { role: "assistant", content: response, timestamp: now, visibility: "user" },
      );
    }, finishAt);
  }
}

// ─── Hub: agents, lifecycle, and scoped routing ────────────────────────────────

/** The backend's error for a body it can't use (`parse_body` in `src/hub/http/lifecycle.rs`). */
function mockBadBody(err: unknown): string {
  return `the request body isn't valid for this route: ${err instanceof Error ? err.message : String(err)}`;
}

/**
 * Routes that still work on a stopped or failed agent, so the user can repair
 * it: the same list as the backend's repair router (`src/hub/http/dispatch.rs`).
 */
const REPAIRABLE_ROUTES = /^\/(config|providers|mcp|workspace|checkpoints)(\/|$)/;

function mockAgentSummary(agent: MockAgent): Record<string, unknown> {
  return {
    name: agent.name,
    state: agent.runState,
    last_error: agent.runState === "failed" ? agent.lastError : null,
    autostart: agent.autostart,
    role: agent.role,
    a2a_visibility: agent.visibility,
  };
}

function createHub(server: ViteDevServer): MockHub {
  const agents = new Map<string, MockAgent>();
  const hubState = createState("hub");
  const hubClients = new Set<WebSocket>();

  const broadcast = (frame: Record<string, unknown>) => {
    const data = JSON.stringify(frame);
    for (const client of hubClients) {
      if (client.readyState === WebSocket.OPEN) client.send(data);
    }
  };

  const sortedSummaries = () =>
    [...agents.values()].sort((a, b) => byName(a.name, b.name)).map(mockAgentSummary);

  const hub: MockHub = {
    agents,
    deleted: new Map(),
    hubState,
    broadcast,
    summary: mockAgentSummary,
    createAgent(name, options = {}) {
      const agent: MockAgent = {
        name,
        runState: options.runState ?? "running",
        lastError:
          options.lastError === undefined
            ? null
            : { message: options.lastError, at: new Date().toISOString() },
        autostart: options.runState !== "stopped",
        role: options.role === undefined ? null : options.role,
        visibility: "private",
        busy: false,
        unread: 0,
        state: createState(name),
        connectedClients: () => 0,
      };
      if (name !== "scout") {
        agent.state.extraRecent.push({
          role: "assistant",
          content: `Hi, this is ${name}. You are in my conversation, not scout's.`,
          timestamp: new Date().toISOString(),
          visibility: "user",
        });
      }
      agents.set(name, agent);
      setupWebSocket(server, hub, agent);
      return agent;
    },
    setBusy(agent, busy) {
      agent.busy = busy;
      broadcast({ type: "agent_activity", name: agent.name, busy, unread: agent.unread });
    },
    addUnread(agent) {
      agent.unread += 1;
      broadcast({
        type: "agent_activity",
        name: agent.name,
        busy: agent.busy,
        unread: agent.unread,
      });
    },
    clearUnread(agent) {
      if (agent.unread === 0) return;
      agent.unread = 0;
      broadcast({ type: "agent_activity", name: agent.name, busy: agent.busy, unread: 0 });
    },
    transition(agent, runState) {
      agent.runState = runState;
      if (runState !== "running") agent.state.dropSockets();
      broadcast({ type: "agent_state", agent: mockAgentSummary(agent) });
    },
  };

  // The hub WebSocket: server to client only.
  const hubWss = new WebSocketServer({ noServer: true });
  server.httpServer?.on("upgrade", (req, socket, head) => {
    const url = req.url ?? "";
    if (url === "/api/hub/ws" || url.startsWith("/api/hub/ws?")) {
      hubWss.handleUpgrade(req, socket, head, (ws) => hubWss.emit("connection", ws, req));
    }
  });
  hubWss.on("connection", (ws: WebSocket) => {
    hubClients.add(ws);
    ws.on("close", () => hubClients.delete(ws));
    ws.send(JSON.stringify({ type: "agents_snapshot", agents: sortedSummaries() }));
    for (const agent of agents.values()) {
      if (agent.busy || agent.unread > 0) {
        ws.send(
          JSON.stringify({
            type: "agent_activity",
            name: agent.name,
            busy: agent.busy,
            unread: agent.unread,
          }),
        );
      }
    }
    // The only client message: which team prefixes (`team` or `team/...`) to
    // send changes for. The mock has no team files changing, so no change
    // frames follow. Like the backend, it refuses a message it can't use with
    // a warning notice.
    ws.on("message", (raw) => {
      let refusal: string | null = null;
      try {
        const msg = JSON.parse(String(raw));
        const prefixes: unknown = msg?.type === "watch_team" ? msg.prefixes : undefined;
        if (!Array.isArray(prefixes) || prefixes.some((p) => typeof p !== "string")) {
          refusal = "That message isn't one the hub understands, so it was ignored.";
        } else if (prefixes.some((p) => p !== "team" && !p.startsWith("team/"))) {
          refusal =
            "Team watch paths are `team` or start with `team/`, so the request was ignored.";
        }
      } catch {
        refusal = "That message isn't one the hub understands, so it was ignored.";
      }
      if (refusal !== null && ws.readyState === WebSocket.OPEN) {
        ws.send(JSON.stringify({ type: "notice", level: "warn", message: refusal }));
      }
    });
  });

  return hub;
}

/** The state holding the file at `path`: `team/` lives in the shared state. */
function filesFor(hub: MockHub, agentState: MockState, path: string): MockState {
  return path === "team" || path.startsWith("team/") ? hub.hubState : agentState;
}

function notice(
  hub: MockHub,
  level: "info" | "warn" | "error",
  message: string,
  agent?: string,
): void {
  hub.broadcast({ type: "notice", level, message, ...(agent ? { agent } : {}) });
}

const STARTUP_MS = 400;

async function handleLifecycle(
  hub: MockHub,
  req: IncomingMessage,
  res: ServerResponse,
  path: string,
  method: string,
): Promise<void> {
  if (path === "/api/hub/agents" && method === "GET") {
    const list = [...hub.agents.values()].sort((a, b) => byName(a.name, b.name)).map(hub.summary);
    json(res, 200, { agents: list });
    return;
  }

  // GET /api/hub/agents/deleted — newest deletion first, like the backend.
  if (path === "/api/hub/agents/deleted" && method === "GET") {
    const list = [...hub.deleted.values()]
      .sort((a, b) => byName(b.deletedAt, a.deletedAt) || byName(a.agent.name, b.agent.name))
      .map((gone) => ({
        name: gone.agent.name,
        deleted_at: gone.deletedAt,
        checkpoint_id: gone.checkpointId,
      }));
    json(res, 200, { agents: list });
    return;
  }

  // POST /api/hub/agents/restore — `{ name, checkpoint_id? }`; answers 201 with the summary.
  if (path === "/api/hub/agents/restore" && method === "POST") {
    let body: Record<string, unknown>;
    try {
      body = JSON.parse(await readBody(req));
      if (typeof body.name !== "string") throw new Error("missing field `name`");
    } catch (err) {
      json(res, 400, { error: mockBadBody(err) });
      return;
    }
    const name = String(body.name);
    const nameProblem = agentNameProblem(name);
    if (nameProblem !== null) {
      json(res, 400, { error: nameProblem });
      return;
    }
    if (hub.agents.has(name)) {
      json(res, 409, { error: `an agent named '${name}' already exists` });
      return;
    }
    const gone = hub.deleted.get(name);
    if (!gone) {
      json(res, 404, { error: `there is no deleted agent named '${name}' to restore` });
      return;
    }
    if (typeof body.checkpoint_id === "string" && body.checkpoint_id !== gone.checkpointId) {
      json(res, 400, {
        error: `${name} has no checkpoint '${body.checkpoint_id}' to restore from`,
      });
      return;
    }
    hub.deleted.delete(name);
    const agent = gone.agent;
    agent.runState = agent.autostart ? "running" : "stopped";
    hub.agents.set(name, agent);
    hub.broadcast({ type: "agent_restored", agent: hub.summary(agent), by: "user" });
    json(res, 201, hub.summary(agent));
    return;
  }

  if (path === "/api/hub/agents" && method === "POST") {
    // `CreateAgentRequest`: `name` is required; the rest may be absent or null.
    let body: Record<string, unknown>;
    try {
      body = JSON.parse(await readBody(req));
      if (typeof body.name !== "string") throw new Error("missing field `name`");
    } catch (err) {
      json(res, 400, { error: mockBadBody(err) });
      return;
    }
    const name = String(body.name);
    const nameProblem = agentNameProblem(name);
    if (nameProblem !== null) {
      json(res, 400, { error: nameProblem });
      return;
    }
    if (hub.agents.has(name)) {
      json(res, 409, { error: `an agent named '${name}' already exists` });
      return;
    }
    const modelsFrom = typeof body.models_from === "string" ? body.models_from : null;
    if (modelsFrom !== null && !hub.agents.has(modelsFrom)) {
      json(res, 400, { error: `no agent named '${modelsFrom}'` });
      return;
    }
    if (modelsFrom === null && typeof body.providers_toml !== "string") {
      json(res, 400, { error: "give models_from or providers_toml" });
      return;
    }
    hub.deleted.delete(name);
    const agent = hub.createAgent(name, {
      role: typeof body.description === "string" ? body.description : null,
    });
    if (body.a2a_visibility === "public") agent.visibility = "public";
    hub.broadcast({ type: "agent_created", agent: hub.summary(agent), by: "user" });
    json(res, 201, hub.summary(agent));
    return;
  }

  if (path === "/api/hub/status" && method === "GET") {
    const counts = { starting: 0, running: 0, stopped: 0, failed: 0 };
    for (const agent of hub.agents.values()) counts[agent.runState]++;
    json(res, 200, {
      version: MOCK_RESIDUUM_VERSION,
      uptime_secs: Math.floor(process.uptime()),
      tunnel: MOCK_CLOUD_STATUS,
      agents: counts,
    });
    return;
  }

  // POST /api/hub/stop-all — stops every running or starting agent and leaves
  // the hub running. Answers `{ stopped, failed }`.
  if (path === "/api/hub/stop-all" && method === "POST") {
    const stopped: Record<string, unknown>[] = [];
    for (const agent of [...hub.agents.values()].sort((a, b) => byName(a.name, b.name))) {
      if (agent.runState !== "running" && agent.runState !== "starting") continue;
      hub.transition(agent, "stopped");
      stopped.push(hub.summary(agent));
    }
    json(res, 200, { stopped, failed: [] });
    return;
  }

  const match = /^\/api\/hub\/agents\/([^/]+)(?:\/(start|stop|restart))?$/.exec(path);
  const agent = match ? hub.agents.get(decodeURIComponent(match[1] ?? "")) : undefined;
  if (!match || !agent) {
    if (match) {
      json(res, 404, { error: `no agent named '${decodeURIComponent(match[1] ?? "")}'` });
    } else {
      json(res, 404, { error: `mock: unknown endpoint ${method} ${path}` });
    }
    return;
  }
  const action = match[2];

  if (!action && method === "DELETE") {
    agent.state.dropSockets();
    agent.runState = "stopped";
    agent.busy = false;
    hub.agents.delete(agent.name);
    const checkpointId = `ckpt-${agent.name}-${Date.now()}`;
    hub.deleted.set(agent.name, {
      agent,
      deletedAt: new Date().toISOString(),
      checkpointId,
    });
    hub.broadcast({ type: "agent_deleted", name: agent.name, by: "user" });
    json(res, 200, { deleted: true, checkpoint_id: checkpointId });
    return;
  }

  if (!action && method === "PATCH") {
    let body: Record<string, unknown>;
    try {
      body = JSON.parse(await readBody(req));
    } catch (err) {
      json(res, 400, { error: mockBadBody(err) });
      return;
    }
    const hasVisibility = body.a2a_visibility === "public" || body.a2a_visibility === "private";
    if (typeof body.autostart !== "boolean" && !hasVisibility) {
      json(res, 400, {
        error: "the request must set at least one of autostart or a2a_visibility",
      });
      return;
    }
    if (typeof body.autostart === "boolean") agent.autostart = body.autostart;
    if (hasVisibility) agent.visibility = body.a2a_visibility as "public" | "private";
    hub.broadcast({ type: "agent_state", agent: hub.summary(agent) });
    json(res, 200, hub.summary(agent));
    return;
  }

  if (action && method === "POST") {
    if (action === "stop") {
      hub.transition(agent, "stopped");
      json(res, 200, hub.summary(agent));
      return;
    }
    if (action === "start" && agent.runState === "running") {
      json(res, 200, hub.summary(agent));
      return;
    }
    hub.transition(agent, "starting");
    await new Promise((done) => setTimeout(done, STARTUP_MS));
    if (agent.name === "brittle") {
      agent.lastError = {
        message: "providers.toml: model 'gpt-9' is not offered by provider 'openai'",
        at: new Date().toISOString(),
      };
      hub.transition(agent, "failed");
    } else {
      hub.transition(agent, "running");
    }
    json(res, 200, hub.summary(agent));
    return;
  }

  json(res, 404, { error: `mock: unknown endpoint ${method} ${path}` });
}

/** Team files: the shared `team/` tree, addressed relative to `team/`. */
async function handleTeamWorkspace(
  hub: MockHub,
  req: IncomingMessage,
  res: ServerResponse,
  path: string,
  method: string,
): Promise<void> {
  const state = hub.hubState;
  const rest = path.slice("/api/team/workspace".length);
  const query = new URL(req.url ?? "", "http://localhost").searchParams;
  const inTeam = (p: string) => (p === "" ? "team" : `team/${p}`);

  if (rest === "/files" && method === "GET") {
    json(res, 200, state.workspaceFiles[inTeam(query.get("path") ?? "")] ?? []);
    return;
  }
  if (rest === "/file" && method === "GET") {
    const content = state.workspaceFileContents[inTeam(query.get("path") ?? "")];
    if (content === undefined) text(res, 404, "file not found");
    else fileRead(res, content);
    return;
  }
  if (rest === "/file" && method === "PUT") {
    const body = JSON.parse(await readBody(req));
    state.workspaceFileContents[inTeam(String(body.path))] = String(body.content);
    json(res, 200, { saved: true, version: mockFileVersion(String(body.content)) });
    return;
  }
  if (rest === "/file" && method === "DELETE") {
    delete state.workspaceFileContents[inTeam(query.get("path") ?? "")];
    json(res, 200, { deleted: true, checkpoint_id: null });
    return;
  }
  if (rest === "/validate" && method === "POST") {
    await readBody(req);
    json(res, 200, { diagnostics: [] });
    return;
  }
  if (rest === "/move" && method === "POST") {
    const body = JSON.parse(await readBody(req));
    const from = inTeam(String(body.from));
    const content = state.workspaceFileContents[from];
    if (content !== undefined) {
      state.workspaceFileContents[inTeam(String(body.to))] = content;
      delete state.workspaceFileContents[from];
    }
    json(res, 200, {
      moved: true,
      version: content === undefined ? null : mockFileVersion(content),
    });
    return;
  }
  json(res, 404, { error: `mock: unknown endpoint ${method} ${path}` });
}

/**
 * Resolve a request to the state and unscoped path the handlers below expect,
 * or answer it here. The unscoped `/api/...` routes no longer exist: only the
 * contract's scoped routes are served, so a call that skips the scope fails
 * here the way it would against the real backend.
 */
async function routeScoped(
  hub: MockHub,
  req: IncomingMessage,
  res: ServerResponse,
  path: string,
  method: string,
): Promise<"handled" | { state: MockState; path: string }> {
  if (path.startsWith("/api/mock/")) {
    const target = new URL(req.url ?? "", "http://localhost").searchParams.get("agent");
    const agent =
      (target ? hub.agents.get(target) : undefined) ??
      [...hub.agents.values()].find((a) => a.runState === "running");
    return { state: agent?.state ?? hub.hubState, path };
  }

  const agentMatch = /^\/api\/agents\/([^/]+)(\/.*)?$/.exec(path);
  if (agentMatch) {
    const name = decodeURIComponent(agentMatch[1] ?? "");
    const sub = agentMatch[2] ?? "";
    const agent = hub.agents.get(name);
    if (!agent) {
      json(res, 404, { error: `no agent named '${name}'` });
      return "handled";
    }
    if (agent.runState !== "running" && !REPAIRABLE_ROUTES.test(sub)) {
      json(res, 409, { error: `${name} is ${agent.runState}`, state: agent.runState });
      return "handled";
    }
    return { state: agent.state, path: `/api${sub}` };
  }

  if (
    path === "/api/hub/status" ||
    path === "/api/hub/stop-all" ||
    path === "/api/hub/agents" ||
    path.startsWith("/api/hub/agents/")
  ) {
    await handleLifecycle(hub, req, res, path, method);
    return "handled";
  }
  // Hub config keeps its path; every other hub route is a hub-level route.
  if (path.startsWith("/api/hub/config/")) return { state: hub.hubState, path };
  if (path.startsWith("/api/hub/")) {
    return { state: hub.hubState, path: `/api${path.slice("/api/hub".length)}` };
  }

  if (path.startsWith("/api/team/workspace/")) {
    await handleTeamWorkspace(hub, req, res, path, method);
    return "handled";
  }
  if (path.startsWith("/api/team/")) {
    return { state: hub.hubState, path: `/api${path.slice("/api/team".length)}` };
  }

  json(res, 404, {
    error: `mock: ${path} is not a hub, team, or agent route; the contract scopes every /api path`,
  });
  return "handled";
}

// ─── Plugin export ─────────────────────────────────────────────────────────────

export function mockServerPlugin(): Plugin {
  return {
    name: "residuum-mock-server",
    configureServer(server) {
      const hub = createHub(server);
      const setup = process.env.VITE_MOCK_SETUP === "1";

      // With no agents the web UI shows the setup wizard, and finishing it
      // creates the first one.
      if (!setup) {
        hub.createAgent("scout", { role: "Digs through the web and the wiki, then reports back" });
        hub.createAgent("atlas", { role: "Keeps the team wiki tidy" });
        hub.createAgent("drifter", {
          runState: "stopped",
          role: "Sleeps until something needs it",
        });
        hub.createAgent("brittle", {
          runState: "failed",
          role: "Has a broken model config",
          lastError: "providers.toml: model 'gpt-9' is not offered by provider 'openai'",
        });
      }

      setupRestMiddleware(server, hub);
      startMockArtifactsListener(hub.hubState);

      const modeLabel = setup ? "setup" : "running";
      console.log("");
      console.log("  [mock] API mock server active");
      console.log(`  [mock] Mode: ${modeLabel} (set VITE_MOCK_SETUP=1 for setup wizard)`);
      console.log("  [mock] Agents: scout, atlas (running), drifter (stopped), brittle (failed)");
      console.log(
        "  [mock] Hub WebSocket on /api/hub/ws, agent WebSockets on /api/agents/{name}/ws",
      );
      console.log("");
    },
  };
}
