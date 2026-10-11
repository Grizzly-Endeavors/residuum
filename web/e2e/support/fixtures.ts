/**
 * The `test` every spec imports, in place of Playwright's own.
 *
 * On top of Playwright's fixtures it:
 * - resets the mock before every test, so no test sees what another left behind,
 * - keeps the page on loopback, so a font or script on the internet can't change a run,
 * - freezes the page's clock at the mock's clock when a project asks for it,
 *   and in manual time keeps it with the mock's simulated time (see `MockControls`),
 * - makes `page.goto` and `page.reload` wait until the app is usable (`waitForApp`
 *   in `app.ts`), so a spec's first action never lands on a half-built page,
 * - skips, with the reason, what can't run here: visual specs outside the
 *   Playwright container, and the WebKit project on a machine that can't start WebKit.
 *
 * - points `baseURL`, and with it `page.goto` and the `mock` controls, at the
 *   worker's own dev or preview server (`servers.ts`), so workers never share a mock.
 */
import { expect, test as base, webkit, type Page, type Response } from "@playwright/test";
import { FIXED_START_MS, type TimeProgress } from "../../mock/env";
import { hubReach, pageClock, waitForApp } from "./app";
import { serversOf, type MockServerKind } from "./servers";

export { expect };

/** Options a project sets through `use`. */
export interface E2EOptions {
  /** The page's clock stands at the mock's fixed time, so relative times ("2h ago") render the same on every run. */
  frozenClock: boolean;
  /** Only the Playwright container's browser may render this project's tests. */
  requiresContainer: boolean;
  /** Which of the worker's mock servers the project drives. */
  mockServer: MockServerKind;
}

/** The mock's test-control endpoints (`POST /api/mock/...`), for a spec that stages a situation. */
export interface MockControls {
  /** POST to the mock and return its JSON answer; any status but 2xx fails the test. */
  post: (
    path: string,
    options?: { params?: Record<string, string>; data?: Record<string, unknown> },
  ) => Promise<unknown>;
  /**
   * Stop simulated time: from now on nothing the mock simulates happens until
   * the spec moves time with `advance` or `stepUntil`, so a moment of a turn
   * stays on screen for as long as the spec looks at it. Call it before
   * starting what you want to watch. The fixture's reset turns it off again.
   *
   * The page's clock (`Date`) stops at the mock's time too, and moves only
   * with it, so what the page times itself, such as "Worked for 2s", follows
   * simulated time. Its timers keep running, so a reconnect still happens.
   */
  manualTime: () => Promise<void>;
  /**
   * Move manual time forward by `ms`: every timer due by then runs, in order.
   * The page's clock moves to the end of the span first, so the page reads
   * every frame the move sends at that time: a turn that starts at 0 and ends
   * within `advance(1_500)` took 1.5s. Before moving time again, wait for what
   * this move shows, so its frames are read at this move's time.
   */
  advance: (ms: number) => Promise<TimeProgress>;
  /**
   * Run the mock's timers one at a time until `expectation` holds. It gets the
   * time to allow each try, to pass on to its assertion:
   * `mock.stepUntil((timeout) => expect(card).toBeVisible({ timeout }))`.
   * The page gets that long to show what a timer did before the next one
   * runs, and the steps end when no timer is left, so the bound is the turn's
   * timers, not a wall-clock guess. A timer can close the moment it opens, so
   * for a window that a later timer ends, `advance` to a time inside it. The
   * page's clock is set to each timer's time before it runs, but the page may
   * read its frames after the next step, so assert durations after `advance`.
   */
  stepUntil: (expectation: (timeout: number) => Promise<unknown>) => Promise<void>;
}

/** How long `stepUntil` lets the page show what a timer did before it runs the next. */
const STEP_SETTLE_MS = 2000;

/** What the mock's time controls answer: the time it reads, and when its next timer is due. */
interface TimeReading {
  now: string;
  next: string | null;
}

interface E2EFixtures {
  mock: MockControls;
  containerGuard: undefined;
  webkitGuard: undefined;
}

/** `scripts/with-playwright-container.sh` sets `E2E_CONTAINER_WS` to the container's browser server while it runs. */
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

/**
 * The list reporter names no reason for a skipped test, so say it once per
 * worker, where the person running the suite will read it.
 */
const noted = new Set<string>();

function noteOnce(message: string): void {
  if (noted.has(message)) return;
  noted.add(message);
  process.stderr.write(`\n${message}\n\n`);
}

/**
 * Whether a navigation landed on a page of the app under test, as opposed to
 * an artifact's page on its own origin or something that isn't a page. Those
 * have no shell to wait for.
 */
function isAppPage(response: Response | null, baseURL: string | undefined): boolean {
  if (response === null || baseURL === undefined) return false;
  const isHtml = (response.headers()["content-type"] ?? "").includes("text/html");
  return isHtml && new URL(response.url()).origin === new URL(baseURL).origin;
}

/** Make `page.goto` and `page.reload` return once the app is usable, not once the document has loaded. */
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

export const test = base.extend<E2EFixtures & E2EOptions>({
  frozenClock: [false, { option: true }],
  requiresContainer: [false, { option: true }],
  mockServer: ["dev", { option: true }],

  baseURL: async ({ mockServer }, use, testInfo) => {
    await use(serversOf(testInfo.parallelIndex)[mockServer].url);
  },

  // Automatic fixtures are set up before the test's own, so the reset lands
  // before the page opens and connects.
  mock: [
    async ({ request, context }, use) => {
      const post: MockControls["post"] = async (path, options) => {
        const response = await request.post(path, options);
        expect(response.ok(), `POST ${path} answered ${response.status()}`).toBe(true);
        // A page loaded while the hub is out of reach is ready when it shows the banner (see `hubReach`).
        if (path === "/api/mock/hub-socket") hubReach.lost = options?.data?.online === false;
        if (path === "/api/mock/reset") hubReach.lost = false;
        return (await response.json()) as unknown;
      };
      // In manual time, what the mock last said its clock reads; the page's clock follows it.
      let reading: TimeReading | undefined;
      const pageClockAt = async (ms: number): Promise<void> => {
        await context.clock.setFixedTime(ms);
        pageClock.fixedAt = ms;
      };
      const controls: MockControls = {
        post,
        manualTime: async () => {
          reading = (await post("/api/mock/time", { data: { mode: "manual" } })) as TimeReading;
          await pageClockAt(Date.parse(reading.now));
        },
        advance: async (ms) => {
          if (reading !== undefined) await pageClockAt(Date.parse(reading.now) + ms);
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
              if (reading?.next != null) await pageClockAt(Date.parse(reading.next));
              const stepped = (await post("/api/mock/time/step")) as TimeProgress & TimeReading;
              reading = stepped;
              // No timer left to run: the expectation's own failure says what never showed.
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
      // In the container the browser is the container's, so this machine's WebKit doesn't matter.
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

  context: async ({ context, frozenClock }, use) => {
    await context.route(leavesLoopback, (route) => route.abort("blockedbyclient"));
    pageClock.fixedAt = frozenClock ? FIXED_START_MS : null;
    if (frozenClock) await context.clock.setFixedTime(FIXED_START_MS);
    // With the network off the hub can't be reached, so a page that loads then is ready showing the banner (see `hubReach`).
    const setOffline = context.setOffline.bind(context);
    context.setOffline = async (offline) => {
      hubReach.lost = offline;
      await setOffline(offline);
    };
    await use(context);
  },

  page: async ({ page, baseURL }, use) => {
    waitForAppAfterLoads(page, baseURL);
    await use(page);
  },
});

/**
 * The origin of the running test's artifacts listener, on the worker's server
 * the test runs against (its project's `mockServer`). Call it from inside a test.
 */
export function artifactsOrigin(): string {
  const info = test.info();
  const kind = (info.project.use as Partial<E2EOptions>).mockServer ?? "dev";
  return `http://localhost:${String(serversOf(info.parallelIndex)[kind].artifactsPort)}`;
}
