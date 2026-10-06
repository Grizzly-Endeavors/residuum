/**
 * The workbench SDK in a real browser, on an artifact page opened straight
 * from the mock's artifacts origin: no Residuum app around it. Each test
 * writes a blank probe artifact, opens it, and drives `window.residuum`.
 */
import type { Page, Request, Route } from "@playwright/test";
import { ARTIFACT_REFUSAL_MESSAGE } from "../../mock/artifacts-origin";
import type { Frame, Sdk } from "../../src/test/workbench-sdk";
import { artifactsOrigin, expect, test, type MockControls } from "../support/fixtures";

/** The page's window, with what the SDK and the tests put on it. */
type ProbeWindow = Window & { residuum: Sdk; seen: Frame[]; marker?: string };

const PROBE = "sdk-probe";
const PROBE_FILE = `team/workbench/${PROBE}.html`;

function probeHtml(title: string): string {
  return `<!doctype html><html><head><title>${title}</title></head><body style="background:#14181f;color:#e6e8ec"><h1>${title}</h1></body></html>`;
}

async function writeProbe(mock: MockControls, title = "SDK probe"): Promise<void> {
  await mock.post("/api/mock/team-file", { data: { path: PROBE_FILE, content: probeHtml(title) } });
}

/** Resolves once the page's live connection to Residuum is up. */
async function connected(page: Page): Promise<void> {
  await page.evaluate(
    () =>
      new Promise<void>((resolve) => {
        (window as unknown as ProbeWindow).residuum.on("connection", (frame) => {
          if (frame.state === "connected") resolve();
        });
      }),
  );
}

/** Open the probe artifact on the artifacts origin, connected to Residuum. */
async function openProbe(page: Page, mock: MockControls): Promise<void> {
  await writeProbe(mock);
  await page.goto(`${artifactsOrigin()}/${PROBE}/`);
  await connected(page);
}

/** The API requests the page sends from now on. */
function apiRequests(page: Page): Request[] {
  const sent: Request[] = [];
  page.on("request", (request) => {
    if (new URL(request.url()).pathname.startsWith("/api/")) sent.push(request);
  });
  return sent;
}

const pathOf = (request: Request): string => new URL(request.url()).pathname;

/** Matches requests to `path` on the artifacts origin, whatever their query. */
const onProbeOrigin =
  (path: string) =>
  (url: URL): boolean =>
    url.origin === artifactsOrigin() && url.pathname === path;

/** What the page's handlers collected in `window.seen`. */
async function seen(page: Page): Promise<Frame[]> {
  return page.evaluate(() => (window as unknown as ProbeWindow).seen);
}

test.describe("residuum.fetch", () => {
  test("reaches the hub, the team and a named agent on the page's own origin", async ({
    page,
    mock,
  }) => {
    const sent = apiRequests(page);
    await openProbe(page, mock);
    const answers = await page.evaluate(async () => {
      const { residuum } = window as unknown as ProbeWindow;
      const statusOf = async (path: string): Promise<number> => (await residuum.fetch(path)).status;
      const artifacts = (await (await residuum.fetch("/api/team/workbench/artifacts")).json()) as {
        name: string;
      }[];
      return {
        hub: await statusOf("/api/hub/status"),
        agent: await statusOf("/api/agents/atlas/sessions"),
        unscopedHub: await statusOf("/api/system/timezone"),
        artifacts: artifacts.map((a) => a.name),
      };
    });
    expect(answers).toEqual({
      hub: 200,
      agent: 200,
      unscopedHub: 200,
      artifacts: expect.arrayContaining([PROBE, "tip-splitter"]) as unknown,
    });
    const calls = sent.filter((r) => pathOf(r) !== "/api/hub/ws");
    expect(calls.map(pathOf).sort()).toEqual([
      "/api/agents/atlas/sessions",
      "/api/hub/status",
      "/api/hub/system/timezone",
      "/api/team/workbench/artifacts",
    ]);
    for (const request of calls) {
      expect(new URL(request.url()).origin).toBe(artifactsOrigin());
      expect(request.headers()["x-residuum-artifact"]).toBe(PROBE);
    }
  });

  test("answers an agent path that names no agent with 400 and sends nothing", async ({
    page,
    mock,
  }) => {
    const sent = apiRequests(page);
    await openProbe(page, mock);
    const answer = await page.evaluate(async () => {
      const resp = await (window as unknown as ProbeWindow).residuum.fetch("/api/status");
      return { status: resp.status, body: (await resp.json()) as { error: string } };
    });
    expect(answer.status).toBe(400);
    expect(answer.body.error).toContain("/api/agents/<name>/status");
    expect(sent.map(pathOf)).not.toContain("/api/status");
  });

  test("gets 403 from a route on the block list", async ({ page, mock }) => {
    await openProbe(page, mock);
    const answers = await page.evaluate(async () => {
      const { residuum } = window as unknown as ProbeWindow;
      const refused = async (path: string): Promise<[number, unknown]> => {
        const resp = await residuum.fetch(path, { method: "POST" });
        return [resp.status, await resp.json()];
      };
      return [await refused("/api/hub/shutdown"), await refused("/api/shutdown")];
    });
    expect(answers).toEqual([
      [403, { error: ARTIFACT_REFUSAL_MESSAGE }],
      [403, { error: ARTIFACT_REFUSAL_MESSAGE }],
    ]);
  });

  test("runs at most 8 ordinary requests and 4 model calls at once", async ({ page, mock }) => {
    await openProbe(page, mock);
    const ordinary: Route[] = [];
    const modelCalls: Route[] = [];
    await page.route(onProbeOrigin("/api/hub/status"), (route) => {
      ordinary.push(route);
    });
    await page.route(onProbeOrigin("/api/agents/atlas/model/complete"), (route) => {
      modelCalls.push(route);
    });
    await page.evaluate(() => {
      const { residuum } = window as unknown as ProbeWindow;
      for (let i = 0; i < 10; i += 1) void residuum.fetch(`/api/hub/status?n=${String(i)}`);
      for (let i = 0; i < 6; i += 1) {
        void residuum.ask(`Question ${String(i)}`, { agent: "atlas" }).catch(() => undefined);
      }
    });
    await expect.poll(() => ordinary.length).toBe(8);
    await expect.poll(() => modelCalls.length).toBe(4);

    // Finishing a model call lets the next one out, and by then the queued
    // ordinary requests would have gone too if their lane let them.
    await modelCalls[0]?.fulfill({ json: { content: "ok" } });
    await expect.poll(() => modelCalls.length).toBe(5);
    expect(ordinary).toHaveLength(8);

    await ordinary[0]?.fulfill({ json: {} });
    await expect.poll(() => ordinary.length).toBe(9);
    expect(new URL(ordinary[8]?.request().url() ?? "").searchParams.get("n")).toBe("8");

    for (const route of [...ordinary.slice(1), ...modelCalls.slice(1)]) {
      await route.fulfill({ json: {} });
    }
  });

  test("retries the relay's overloaded 503, and gives up after three retries", async ({
    page,
    mock,
  }) => {
    await page.clock.install();
    await openProbe(page, mock);
    let overloaded = 2;
    let calls = 0;
    await page.route(onProbeOrigin("/api/agents/atlas/status"), async (route) => {
      calls += 1;
      if (overloaded > 0) {
        overloaded -= 1;
        await route.fulfill({ status: 503, body: "agent overloaded" });
      } else {
        await route.continue();
      }
    });
    const status = (): Promise<number> =>
      page.evaluate(
        async () =>
          (await (window as unknown as ProbeWindow).residuum.fetch("/api/agents/atlas/status"))
            .status,
      );

    const recovered = status();
    for (const attempts of [1, 2]) {
      await expect.poll(() => calls).toBe(attempts);
      await page.clock.runFor(2500);
    }
    expect(await recovered).toBe(200);
    expect(calls).toBe(3);

    overloaded = Infinity;
    calls = 0;
    const gaveUp = status();
    for (const attempts of [1, 2, 3]) {
      await expect.poll(() => calls).toBe(attempts);
      await page.clock.runFor(5000);
    }
    expect(await gaveUp).toBe(503);
    expect(calls).toBe(4);
  });
});

test.describe("residuum.on and residuum.watch", () => {
  test("a top-level on of an agent's frame type throws, pointing at agent(name).on", async ({
    page,
    mock,
  }) => {
    await openProbe(page, mock);
    const thrown = await page.evaluate(() => {
      try {
        (window as unknown as ProbeWindow).residuum.on("turn_started", () => undefined);
        return null;
      } catch (err) {
        return { name: (err as Error).name, message: (err as Error).message };
      }
    });
    expect(thrown?.name).toBe("TypeError");
    expect(thrown?.message).toContain('residuum.agent("<name>").on("turn_started", handler)');
  });

  test("a top-level watch of a path outside the team throws, pointing at agent(name).watch", async ({
    page,
    mock,
  }) => {
    await openProbe(page, mock);
    const thrown = await page.evaluate(() => {
      try {
        (window as unknown as ProbeWindow).residuum.watch("notes", () => undefined);
        return null;
      } catch (err) {
        return { name: (err as Error).name, message: (err as Error).message };
      }
    });
    expect(thrown?.name).toBe("TypeError");
    expect(thrown?.message).toContain('residuum.agent("<name>").watch("notes", handler)');
  });

  test("watch follows a team prefix with no agent involved", async ({ page, mock }) => {
    await openProbe(page, mock);
    await page.evaluate(() => {
      const w = window as unknown as ProbeWindow;
      w.seen = [];
      w.residuum.watch("team/notes", (frame) => w.seen.push(frame));
    });
    // The watch reaches the hub over the socket, so edit until it has.
    let edit = 0;
    await expect
      .poll(async () => {
        edit += 1;
        await mock.post("/api/mock/team-file", {
          data: { path: "team/notes/plan.md", content: `edit ${String(edit)}` },
        });
        return (await seen(page)).length;
      })
      .toBeGreaterThan(0);
    await mock.post("/api/mock/team-file", {
      data: { path: "team/wiki/a.md", content: "elsewhere" },
    });
    await mock.post("/api/mock/team-file", {
      data: { path: "team/notes/plan.md", content: "last edit" },
    });
    await expect
      .poll(async () => (await seen(page)).at(-1))
      .toEqual({
        type: "workspace_changed",
        changes: [{ path: "team/notes/plan.md", kind: "modified" }],
      });
    const paths = (await seen(page)).flatMap((f) =>
      (f.changes as { path: string }[]).map((c) => c.path),
    );
    expect(paths.every((path) => path.startsWith("team/notes"))).toBe(true);
  });

  test("agent(name).on hears that agent's turns, tool calls included", async ({ page, mock }) => {
    const sockets: string[] = [];
    page.on("websocket", (ws) => sockets.push(new URL(ws.url()).pathname));
    await openProbe(page, mock);
    await page.evaluate(
      () =>
        new Promise<void>((resolve) => {
          const w = window as unknown as ProbeWindow;
          w.seen = [];
          const atlas = w.residuum.agent("atlas");
          atlas.on("*", (frame) => w.seen.push(frame));
          atlas.on("connection", (frame) => {
            if (frame.state === "connected") resolve();
          });
        }),
    );
    expect(sockets).toContain("/api/agents/atlas/ws");
    // Someone else talks to atlas over a connection of their own.
    await page.evaluate(
      () =>
        new Promise<void>((resolve) => {
          const other = new WebSocket(`ws://${location.host}/api/agents/atlas/ws`);
          other.onopen = () => {
            other.send(
              JSON.stringify({ type: "send_message", id: "m-1", content: "How's the wiki?" }),
            );
            resolve();
          };
        }),
    );
    await expect
      .poll(async () => (await seen(page)).map((frame) => frame.type))
      .toEqual([
        "connection",
        "turn_started",
        "broadcast_response",
        "tool_call",
        "tool_result",
        "turn_usage",
        "tool_call",
        "tool_call",
        "tool_result",
        "tool_result",
        "turn_usage",
        "response",
        "turn_ended",
      ]);
  });

  test("agent(name).watch follows a prefix in that agent's workspace", async ({ page, mock }) => {
    await openProbe(page, mock);
    await page.evaluate(() => {
      const w = window as unknown as ProbeWindow;
      w.seen = [];
      w.residuum.agent("atlas").watch("notes", (frame) => w.seen.push(frame));
    });
    let edit = 0;
    await expect
      .poll(async () => {
        edit += 1;
        await mock.post("/api/mock/agent-file", {
          params: { agent: "atlas" },
          data: { path: "notes/plan.md", content: `edit ${String(edit)}` },
        });
        return (await seen(page)).length;
      })
      .toBeGreaterThan(0);
    await mock.post("/api/mock/agent-file", {
      params: { agent: "atlas" },
      data: { path: "memory/elsewhere.md", content: "elsewhere" },
    });
    await mock.post("/api/mock/agent-file", {
      params: { agent: "atlas" },
      data: { path: "notes/plan.md", content: "last edit" },
    });
    await expect
      .poll(async () => (await seen(page)).at(-1))
      .toEqual({
        type: "workspace_changed",
        changes: [{ path: "notes/plan.md", kind: "modified" }],
      });
    const paths = (await seen(page)).flatMap((f) =>
      (f.changes as { path: string }[]).map((c) => c.path),
    );
    expect(paths.every((path) => path.startsWith("notes"))).toBe(true);
  });
});

test.describe("residuum.sessions", () => {
  test("start follows a session on an agent the page has no socket to, from its first frame", async ({
    page,
    mock,
  }) => {
    const sockets: string[] = [];
    page.on("websocket", (ws) => sockets.push(new URL(ws.url()).pathname));
    await openProbe(page, mock);
    const handle = await page.evaluate(async () => {
      const w = window as unknown as ProbeWindow;
      w.seen = [];
      const session = await w.residuum.sessions.start({
        agent: "atlas",
        prompt: "Tidy the wiki index.",
      });
      session.on("*", (frame) => w.seen.push(frame));
      return { agent: session.agent, address: session.address };
    });
    expect(handle.agent).toBe("atlas");
    await expect
      .poll(async () => (await seen(page)).map((frame) => frame.type))
      .toEqual(
        expect.arrayContaining(["session_started", "session_tool_call", "session_response"]),
      );
    const frames = await seen(page);
    expect(frames[0]).toMatchObject({
      type: "session_started",
      session: { address: handle.address, source_label: `artifact:${PROBE}` },
    });
    expect(sockets).toEqual(["/api/hub/ws"]);
  });

  test("start rejects after 10 seconds when the hub socket is refused, and starts nothing", async ({
    page,
    mock,
  }) => {
    await page.routeWebSocket(/\/api\/hub\/ws$/, (ws) => {
      void ws.close();
    });
    await page.clock.install();
    await writeProbe(mock);
    const sent = apiRequests(page);
    await page.goto(`${artifactsOrigin()}/${PROBE}/`);
    const state = await page.evaluate(
      () =>
        new Promise<unknown>((resolve) => {
          (window as unknown as ProbeWindow).residuum.on("connection", (frame) => {
            resolve(frame.state);
          });
        }),
    );
    expect(state).toBe("disconnected");

    const outcome = page.evaluate(() =>
      (window as unknown as ProbeWindow).residuum.sessions
        .start({ agent: "atlas", prompt: "Tidy the wiki index." })
        .then(
          () => "started",
          (err: unknown) => {
            const { code, message } = err as Error & { code?: string };
            return `${code ?? ""}: ${message}`;
          },
        ),
    );
    await page.clock.runFor(9_000);
    await page.clock.runFor(1_000);
    expect(await outcome).toMatch(/^no_live_connection: .*live connection isn't available/);
    expect(sent.filter((r) => r.method() === "POST")).toEqual([]);
  });

  test("a handle hears resync after the relay lags", async ({ page, mock }) => {
    await openProbe(page, mock);
    const address = await page.evaluate(async () => {
      const w = window as unknown as ProbeWindow;
      w.seen = [];
      const session = await w.residuum.sessions.start({
        agent: "atlas",
        prompt: "Watch the wiki.",
      });
      session.on("resync", (frame) => w.seen.push(frame));
      return session.address;
    });
    expect(await mock.post("/api/mock/session-relay-lag")).toEqual({ notified: 1 });
    await expect
      .poll(() => seen(page))
      .toEqual([
        {
          type: "resync",
          session: expect.objectContaining({
            address,
            source_label: `artifact:${PROBE}`,
          }) as unknown,
        },
      ]);
  });
});

test.describe("live reload", () => {
  test("the page reloads when its artifact changes", async ({ page, mock }) => {
    await openProbe(page, mock);
    await page.evaluate(() => {
      (window as unknown as ProbeWindow).marker = "first load";
    });
    await writeProbe(mock, "SDK probe, edited");
    await expect(page).toHaveTitle("SDK probe, edited");
    expect(await page.evaluate(() => (window as unknown as ProbeWindow).marker)).toBeUndefined();
  });

  test("a page with its own artifact_updated handler is told instead of reloaded", async ({
    page,
    mock,
  }) => {
    await openProbe(page, mock);
    await page.evaluate(() => {
      const w = window as unknown as ProbeWindow;
      w.seen = [];
      w.marker = "first load";
      w.residuum.on("artifact_updated", (frame) => w.seen.push(frame));
    });
    await writeProbe(mock, "SDK probe, edited");
    await expect.poll(() => seen(page)).toEqual([{ type: "artifact_updated", name: PROBE }]);
    await expect(page).toHaveTitle("SDK probe");
    expect(await page.evaluate(() => (window as unknown as ProbeWindow).marker)).toBe("first load");
  });
});
