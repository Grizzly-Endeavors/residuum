import { defineConfig, devices } from "@playwright/test";
import type { E2EOptions } from "./e2e/support/fixtures";
import { devServer, previewServer } from "./e2e/support/servers";

/**
 * End-to-end, accessibility and visual tests against the mock server.
 * `web/CONTRIBUTING.md` ("Testing") says what each project is for.
 *
 * Specs are sorted into projects by tag:
 * - `@visual`  renders in the Playwright container (`visual-*`),
 * - `@preview` needs the production build (`preview-*`),
 * - everything else runs against the Vite dev server (`desktop`, `phone`, `webkit-phone`).
 *
 * A project never picks up another kind's specs, so a tag changes where a spec
 * runs and nothing else. Tags go on a test or a describe block:
 * `test("...", { tag: "@preview" }, async ({ page }) => { ... })`.
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
 * `exposeNetwork`.
 */
const containerBrowser = process.env.E2E_CONTAINER_WS ?? "";
const inContainer =
  containerBrowser === ""
    ? {}
    : { connectOptions: { wsEndpoint: containerBrowser, exposeNetwork: "<loopback>" } };

/** What makes a screenshot repeatable: the container's browser, the mock's time and no motion. */
const visualUse = {
  ...inContainer,
  frozenClock: true,
  requiresContainer: true,
  reducedMotion: "reduce",
} as const;

const visualTag = /@visual/;
const previewTag = /@preview/;
const eitherTag = /@visual|@preview/;

export default defineConfig<E2EOptions>({
  testDir: "e2e",
  testMatch: "**/*.spec.ts",
  outputDir: "test-results",

  // The mock's state is global to its server, and every test starts by resetting it.
  workers: 1,
  fullyParallel: false,
  forbidOnly: process.env.CI !== undefined,
  retries: 0,

  reporter: [["list"], ["html", { open: "never" }]],

  // Baselines come from the container only, so the file name carries no platform.
  snapshotPathTemplate: "{testDir}/__screenshots__/{testFilePath}/{arg}-{projectName}{ext}",
  expect: {
    toHaveScreenshot: { scale: "css" },
  },

  use: {
    baseURL: devServer.url,
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
      use: desktop,
      grepInvert: eitherTag,
    },
    {
      name: "phone",
      use: phone,
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
      use: { ...desktop, baseURL: previewServer.url },
      grep: previewTag,
      grepInvert: visualTag,
    },
    {
      name: "preview-phone",
      use: { ...phone, baseURL: previewServer.url },
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

  webServer: [
    {
      command: `npm run dev:mock -- --port ${devServer.port} --strictPort`,
      url: devServer.url,
      env: {
        MOCK_DETERMINISTIC: "1",
        MOCK_ARTIFACTS_PORT: String(devServer.artifactsPort),
      },
      // A server left over from another run would carry its state and its code into this one.
      reuseExistingServer: false,
      timeout: 60_000,
    },
    {
      // The build is what the preview server serves, so it is rebuilt every run.
      command: `npm run build && npm run preview:mock -- --port ${previewServer.port} --strictPort`,
      url: previewServer.url,
      env: {
        MOCK_DETERMINISTIC: "1",
        MOCK_ARTIFACTS_PORT: String(previewServer.artifactsPort),
      },
      reuseExistingServer: false,
      timeout: 180_000,
    },
  ],
});
