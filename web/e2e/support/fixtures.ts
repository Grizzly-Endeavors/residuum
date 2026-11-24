import { expect, test as base, webkit, type Page, type Response } from "@playwright/test";
import { FIXED_START_MS, type TimeProgress } from "../../mock/env";
import { hubReach, pageClock, recordFailedRequests, waitForApp } from "./app";
import { serversOf, type MockServerKind } from "./servers";

export { expect };

export interface E2EOptions {
  frozenClock: boolean;
  requiresContainer: boolean;
  mockServer: MockServerKind;
}

export interface MockControls {
  post: (
    path: string,
    options?: { params?: Record<string, string>; data?: Record<string, unknown> },
  ) => Promise<unknown>;
  manualTime: () => Promise<void>;
  advance: (ms: number) => Promise<TimeProgress>;
  stepUntil: (expectation: (timeout: number) => Promise<unknown>) => Promise<void>;
}

const STEP_SETTLE_MS = 2000;

interface TimeReading {
  now: string;
  next: string | null;
}

interface E2EFixtures {
  mock: MockControls;
  containerGuard: undefined;
  webkitGuard: undefined;
}

function inContainer(): boolean {
  return (process.env.E2E_CONTAINER_WS ?? "") !== "";
}

const CONTAINER_SKIP_MESSAGE =
  "Visual specs are skipped: they render in the Playwright container. Run `just web-e2e`, or `just web-e2e-update` to refresh baselines.";

const LOOPBACK_HOSTS = new Set(["localhost", "127.0.0.1", "[::1]"]);

function leavesLoopback(url: URL): boolean {
  const isNetwork = ["http:", "https:", "ws:", "wss:"].includes(url.protocol);
  return isNetwork && !LOOPBACK_HOSTS.has(url.hostname);
}

const noted = new Set<string>();

function noteOnce(message: string): void {
  if (noted.has(message)) return;
  noted.add(message);
  process.stderr.write(`\n${message}\n\n`);
}

function isAppPage(response: Response | null, baseURL: string | undefined): boolean {
  if (response === null || baseURL === undefined) return false;
  const isHtml = (response.headers()["content-type"] ?? "").includes("text/html");
  return isHtml && new URL(response.url()).origin === new URL(baseURL).origin;
}

function waitForAppAfterLoads(page: Page, baseURL: string | undefined): void {
  const goto = page.goto.bind(page);
  const reload = page.reload.bind(page);
  page.goto = async (url, options) => {
    const response = await goto(url, options);
    if (isAppPage(response, baseURL)) await waitForApp(page);
    return response;
  };
  page.reload = async (options) => {
    const response = await reload(options);
    if (isAppPage(response, baseURL)) await waitForApp(page);
    return response;
  };
}

interface InstalledClock {
  builtins: Pick<Window, "requestAnimationFrame" | "cancelAnimationFrame">;
}

function keepBrowserFrames(): void {
  const clock = (globalThis as { __pwClock?: InstalledClock }).__pwClock;
  const frames = {
    requestAnimationFrame: clock?.builtins.requestAnimationFrame ?? window.requestAnimationFrame,
    cancelAnimationFrame: clock?.builtins.cancelAnimationFrame ?? window.cancelAnimationFrame,
  };
  for (const [name, native] of Object.entries(frames)) {
    Object.defineProperty(window, name, {
      configurable: true,
      get: () => native,
      set: () => undefined,
    });
  }
}

function cpuThrottleRate(): number | null {
  const raw = process.env.E2E_CPU_THROTTLE;
  if (raw === undefined || raw === "") return null;
  const rate = Number(raw);
  if (!Number.isInteger(rate) || rate < 1) {
    throw new Error(`E2E_CPU_THROTTLE must be a whole number of at least 1, got "${raw}"`);
  }
  return rate;
}

async function throttlePageCpu(page: Page, rate: number): Promise<void> {
  const session = await page.context().newCDPSession(page);
  await session.send("Emulation.setCPUThrottlingRate", { rate });
}

const USE_CONTAINER_HINT = "or run `just web-e2e-webkit` to use the Playwright container";

/** Why WebKit can't start on this machine, in one line, or `null` when it can. Probed once per worker. */
let webkitProblem: Promise<string | null> | undefined;

async function probeWebkit(): Promise<string | null> {
  try {
    const browser = await webkit.launch();
    await browser.close();
    return null;
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    if (message.includes("missing dependencies")) {
      return `its system libraries are missing (install them with \`npx playwright install-deps webkit\`, ${USE_CONTAINER_HINT})`;
    }
    if (message.includes("Executable doesn't exist")) {
      return `it isn't installed (install it with \`npx playwright install webkit\`, ${USE_CONTAINER_HINT})`;
    }
    return message.split("\n")[0] ?? message;
  }
}

const CSP_VIOLATION_BINDING = "__e2eReportCspViolation";

function reportCspViolations(binding: string): void {
  document.addEventListener("securitypolicyviolation", (event) => {
    const report = (
      window as unknown as Record<string, ((text: string) => Promise<void>) | undefined>
    )[binding];
    void report?.(`${event.effectiveDirective} ${event.blockedURI} on ${event.documentURI}`);
  });
}

export const test = base.extend<E2EFixtures & E2EOptions>({
  frozenClock: [false, { option: true }],
  requiresContainer: [false, { option: true }],
  mockServer: ["dev", { option: true }],

  baseURL: async ({ mockServer }, use, testInfo) => {
    await use(serversOf(testInfo.parallelIndex)[mockServer].url);
  },

  mock: [
    async ({ request, context }, use) => {
      const post: MockControls["post"] = async (path, options) => {
        const response = await request.post(path, options);
        expect(response.ok(), `POST ${path} answered ${response.status()}`).toBe(true);
        if (path === "/api/mock/hub-socket") hubReach.lost = options?.data?.online === false;
        if (path === "/api/mock/reset") hubReach.lost = false;
        return (await response.json()) as unknown;
      };
      let reading: TimeReading | undefined;
      let pageAt = 0;
      const pageClockTo = async (ms: number): Promise<void> => {
        if (ms < pageAt) {
          throw new Error(
            `the page's clock is at ${pageAt} ms and can't move back to ${ms} ms; the mock's time and the page's have come apart`,
          );
        }
        await context.clock.runFor(ms - pageAt);
        pageAt = ms;
        pageClock.fixedAt = ms;
      };
      const controls: MockControls = {
        post,
        manualTime: async () => {
          reading = (await post("/api/mock/time", { data: { mode: "manual" } })) as TimeReading;
          const now = Date.parse(reading.now);
          await context.clock.setFixedTime(now);
          await context.clock.pauseAt(now);
          await context.clock.setSystemTime(now);
          pageAt = now;
          pageClock.fixedAt = now;
        },
        advance: async (ms) => {
          if (reading !== undefined) await pageClockTo(Date.parse(reading.now) + ms);
          const moved = (await post("/api/mock/time/advance", { data: { ms } })) as TimeProgress &
            TimeReading;
          reading = moved;
          return moved;
        },
        stepUntil: async (expectation) => {
          for (;;) {
            try {
              await expectation(STEP_SETTLE_MS);
              return;
            } catch (notYet) {
              if (reading?.next != null) await pageClockTo(Date.parse(reading.next));
              const stepped = (await post("/api/mock/time/step")) as TimeProgress & TimeReading;
              reading = stepped;
              if (stepped.fired === 0) throw notYet;
            }
          }
        },
      };
      await controls.post("/api/mock/reset");
      await use(controls);
    },
    { auto: true },
  ],

  containerGuard: [
    async ({ requiresContainer }, use, testInfo) => {
      const skip = requiresContainer && !inContainer();
      if (skip) noteOnce(CONTAINER_SKIP_MESSAGE);
      testInfo.skip(skip, CONTAINER_SKIP_MESSAGE);
      await use(undefined);
    },
    { auto: true },
  ],

  webkitGuard: [
    async ({ browserName }, use, testInfo) => {
      if (browserName === "webkit" && !inContainer()) {
        webkitProblem ??= probeWebkit();
        const problem = await webkitProblem;
        if (problem !== null) {
          noteOnce(
            `The WebKit project is skipped: WebKit can't start on this machine, because ${problem}.`,
          );
        }
        testInfo.skip(problem !== null, `WebKit can't start on this machine: ${problem ?? ""}.`);
      }
      await use(undefined);
    },
    { auto: true },
  ],

  context: async ({ context, frozenClock, baseURL, browserName }, use) => {
    const throttle = cpuThrottleRate();
    if (throttle !== null && browserName !== "chromium") {
      throw new Error(
        `E2E_CPU_THROTTLE needs Chromium's CPU throttling, and this project drives ${browserName}`,
      );
    }
    const throttles: Promise<void>[] = [];
    context.on("page", (page) => {
      waitForAppAfterLoads(page, baseURL);
      recordFailedRequests(page, leavesLoopback);
      if (throttle !== null) throttles.push(throttlePageCpu(page, throttle));
    });
    await context.route(leavesLoopback, (route) => route.abort("blockedbyclient"));
    pageClock.fixedAt = frozenClock ? FIXED_START_MS : null;
    if (frozenClock) await context.clock.setFixedTime(FIXED_START_MS);
    else await context.clock.install();
    await context.addInitScript(keepBrowserFrames);
    const violations: string[] = [];
    await context.exposeBinding(CSP_VIOLATION_BINDING, (_source, report: string) => {
      violations.push(report);
    });
    await context.addInitScript(reportCspViolations, CSP_VIOLATION_BINDING);
    const setOffline = context.setOffline.bind(context);
    context.setOffline = async (offline) => {
      hubReach.lost = offline;
      await setOffline(offline);
    };
    await use(context);
    await Promise.all(throttles);
    if (violations.length > 0) {
      throw new Error(
        `The app shell's Content-Security-Policy (web/app-shell-csp.txt) refused: ${violations.join("; ")}`,
      );
    }
  },
});

export function artifactsOrigin(): string {
  const info = test.info();
  const kind = (info.project.use as Partial<E2EOptions>).mockServer ?? "dev";
  return serversOf(info.parallelIndex)[kind].artifactsUrl;
}
