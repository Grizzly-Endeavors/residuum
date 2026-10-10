import { defineConfig, devices } from "@playwright/test";
import type { E2EOptions } from "./e2e/support/fixtures";
import { workerCount, workerServers } from "./e2e/support/servers";

/**
 * End-to-end, accessibility and visual tests against the mock server.
 * `web/CONTRIBUTING.md` ("Testing") says what each project is for.
 *
 * Specs are sorted into projects by tag:
 * - `@visual`  renders in the Playwright container (`visual-*`),
 * - `@preview` needs the production build (`preview-*`),
 * - `@dev` needs the dev server: the component gallery, which builds leave
 *   out, or the dev server's own state, such as having no service worker
 *   (`dev-*`),
 * - everything else runs against the Vite dev server (`desktop`, `phone`, `webkit-phone`),
 *   or on CI against the production build (see `smokeServer` below).
 *
 * A project never picks up another kind's specs, so a tag changes where a spec
 * runs and nothing else. Tags go on a test or a describe block:
 * `test("...", { tag: "@preview" }, async ({ page }) => { ... })`.
 *
 * Which browser a project drives is a separate question: see `containerBrowser` below.
 */

const desktop = {
  ...devices["Desktop Chrome"],
  viewport: { width: 1440, height: 900 },
  screen: { width: 1440, height: 900 },
};

const phone = {
  ...devices["Pixel 7"],
  viewport: { width: 390, height: 844 },
  screen: { width: 390, height: 844 },
  deviceScaleFactor: 2,
};

const webkitPhone = {
  ...devices["iPhone 13"],
  viewport: { width: 390, height: 844 },
};

/**
 * `scripts/with-playwright-container.sh` starts the Playwright container with
 * a browser server and sets this to its address. The `visual-*` projects and
 * `webkit-phone` drive that browser, and reach the mock on this machine through
 * `exposeNetwork`. `E2E_ALL_IN_CONTAINER` (below) adds the other projects.
 */
const containerBrowser = process.env.E2E_CONTAINER_WS ?? "";
const inContainer =
  containerBrowser === ""
    ? {}
    : { connectOptions: { wsEndpoint: containerBrowser, exposeNetwork: "<loopback>" } };

/**
 * `E2E_ALL_IN_CONTAINER=1` sends `desktop`, `phone` and the `preview-*` projects
 * to the container's browser too, so a machine with no Chromium of its own (the
 * CI runners) needs none. Without it they launch this machine's Chromium. The
 * variable only works under the container wrapper; with no container browser it
 * is an error, because a quiet fall-back to a host browser would hide that the
 * run isn't the one that was asked for.
 */
const allInContainer = process.env.E2E_ALL_IN_CONTAINER === "1";
if (allInContainer && containerBrowser === "") {
  throw new Error(
    "E2E_ALL_IN_CONTAINER=1 needs the Playwright container's browser, and E2E_CONTAINER_WS is not set. " +
      "Run through scripts/with-playwright-container.sh, or unset E2E_ALL_IN_CONTAINER to use this machine's Chromium.",
  );
}
const chromiumBrowser = allInContainer ? inContainer : {};

/**
 * On CI (or with `E2E_SMOKE_ON_BUILD=1`) the `desktop` and `phone` specs run
 * against the production build, as the `preview-*` ones do. The dev server
 * serves every module as its own request, and a shared runner queues each of
 * them, so a page load there takes seconds and a spec with a few of them
 * reaches its timeout; the build loads in a handful of requests. Its service
 * worker is blocked, so these specs see the app as the dev server serves it,
 * and the `preview-*` specs cover the worker. Locally they keep the dev
 * server, which needs no build to try a change.
 */
const smokeOnBuild = process.env.CI !== undefined || process.env.E2E_SMOKE_ON_BUILD === "1";
const smokeServer = smokeOnBuild
  ? ({ mockServer: "preview", serviceWorkers: "block" } as const)
  : ({} as const);

/** What makes a screenshot repeatable: the container's browser, the mock's time and no motion. */
const visualUse = {
  ...inContainer,
  frozenClock: true,
  requiresContainer: true,
  reducedMotion: "reduce",
} as const;

const visualTag = /@visual/;
const previewTag = /@preview/;
const devTag = /@dev/;
const eitherTag = /@visual|@preview/;
/** What the `desktop` and `phone` projects leave to the others. */
const notSmoke = /@visual|@preview|@dev/;

export default defineConfig<E2EOptions>({
  testDir: "e2e",
  testMatch: "**/*.spec.ts",
  outputDir: "test-results",

  // Compiles the dev server's modules once, so no spec pays for the first compile of a page or a lazy chunk.
  globalSetup: "./e2e/support/warmup.ts",

  // Every worker drives its own mock servers (`e2e/support/servers.ts`), and every test starts by resetting its worker's mock, so any test can run on any worker.
  workers: workerCount,
  fullyParallel: true,
  forbidOnly: process.env.CI !== undefined,
  retries: 0,

  reporter: [["list"], ["html", { open: "never" }]],

  // Baselines come from the container only, so the file name carries no platform.
  snapshotPathTemplate: "{testDir}/__screenshots__/{testFilePath}/{arg}-{projectName}{ext}",
  expect: {
    toHaveScreenshot: { scale: "css" },
  },

  use: {
    // `baseURL` is the worker's own dev or preview server, set by the fixtures from `mockServer`.
    locale: "en-US",
    timezoneId: "UTC",
    // A selector that matches nothing fails here, well before the test's own timeout.
    actionTimeout: 10_000,
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
    video: "off",
  },

  projects: [
    {
      name: "desktop",
      use: { ...desktop, ...chromiumBrowser, ...smokeServer },
      grepInvert: notSmoke,
    },
    {
      name: "phone",
      use: { ...phone, ...chromiumBrowser, ...smokeServer },
      grepInvert: notSmoke,
    },
    {
      name: "dev-desktop",
      use: { ...desktop, ...chromiumBrowser },
      grep: devTag,
      grepInvert: eitherTag,
    },
    {
      name: "dev-phone",
      use: { ...phone, ...chromiumBrowser },
      grep: devTag,
      grepInvert: eitherTag,
    },
    {
      // Local only: the release workflow doesn't run it. It uses this machine's WebKit, or the
      // container's when the container wrapper runs it, and skips itself when neither starts.
      name: "webkit-phone",
      use: { ...webkitPhone, ...inContainer },
      grepInvert: eitherTag,
    },
    {
      name: "preview-desktop",
      use: { ...desktop, ...chromiumBrowser, mockServer: "preview" },
      grep: previewTag,
      grepInvert: visualTag,
    },
    {
      name: "preview-phone",
      use: { ...phone, ...chromiumBrowser, mockServer: "preview" },
      grep: previewTag,
      grepInvert: visualTag,
    },
    {
      name: "visual-desktop",
      use: { ...desktop, ...visualUse },
      grep: visualTag,
      grepInvert: previewTag,
    },
    {
      name: "visual-phone",
      use: { ...phone, ...visualUse },
      grep: visualTag,
      grepInvert: previewTag,
    },
  ],

  // Playwright starts these one after another, in this order, so the first preview server's build is in place before the others serve it.
  webServer: [
    ...workerServers.map(({ dev }) => ({
      command: `npm run dev:mock -- --port ${dev.port} --strictPort`,
      url: dev.url,
      env: {
        MOCK_DETERMINISTIC: "1",
        MOCK_ARTIFACTS_PORT: String(dev.artifactsPort),
      },
      // A server left over from another run would carry its state and its code into this one.
      reuseExistingServer: false,
      timeout: 60_000,
    })),
    ...workerServers.map(({ preview }, index) => ({
      // The build is what the preview servers serve, so it is rebuilt every run, once.
      command: `${index === 0 ? "npm run build && " : ""}npm run preview:mock -- --port ${preview.port} --strictPort`,
      url: preview.url,
      env: {
        MOCK_DETERMINISTIC: "1",
        MOCK_ARTIFACTS_PORT: String(preview.artifactsPort),
      },
      reuseExistingServer: false,
      timeout: index === 0 ? 180_000 : 60_000,
    })),
  ],
});
