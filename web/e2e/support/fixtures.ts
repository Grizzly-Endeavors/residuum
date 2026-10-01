/**
 * The `test` every spec imports, in place of Playwright's own.
 *
 * On top of Playwright's fixtures it:
 * - resets the mock before every test, so no test sees what another left behind,
 * - keeps the page on loopback, so a font or script on the internet can't change a run,
 * - freezes the page's clock at the mock's clock when a project asks for it,
 * - makes `page.goto` and `page.reload` wait until the app is usable (`waitForApp`
 *   in `app.ts`), so a spec's first action never lands on a half-built page,
 * - skips, with the reason, what can't run here: visual specs outside the
 *   Playwright container, and the WebKit project on a machine that can't start WebKit.
 *
 * All tests share one mock server, whose state is global, so the suite runs on
 * one worker (see `playwright.config.ts`).
 */
import { expect, test as base, webkit, type Page, type Response } from "@playwright/test";
import { FIXED_START_MS } from "../../mock/env";
import { waitForApp } from "./app";

export { expect };

/** Options a project sets through `use`. */
export interface E2EOptions {
  /** The page's clock stands at the mock's fixed time, so relative times ("2h ago") render the same on every run. */
  frozenClock: boolean;
  /** Only the Playwright container's browser may render this project's tests. */
  requiresContainer: boolean;
}

/** The mock's test-control endpoints (`POST /api/mock/...`), for a spec that stages a situation. */
export interface MockControls {
  /** POST to the mock and return its JSON answer; any status but 2xx fails the test. */
  post: (
    path: string,
    options?: { params?: Record<string, string>; data?: Record<string, unknown> },
  ) => Promise<unknown>;
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

  // Automatic fixtures are set up before the test's own, so the reset lands
  // before the page opens and connects.
  mock: [
    async ({ request }, use) => {
      const controls: MockControls = {
        post: async (path, options) => {
          const response = await request.post(path, options);
          expect(response.ok(), `POST ${path} answered ${response.status()}`).toBe(true);
          return (await response.json()) as unknown;
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
    if (frozenClock) await context.clock.setFixedTime(FIXED_START_MS);
    await use(context);
  },

  page: async ({ page, baseURL }, use) => {
    waitForAppAfterLoads(page, baseURL);
    await use(page);
  },
});
