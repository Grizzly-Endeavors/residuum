import { defineConfig, devices } from "@playwright/test";
import type { E2EOptions } from "./e2e/support/fixtures";
import { launchOf, workerCount } from "./e2e/support/servers";

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

const containerBrowser = process.env.E2E_CONTAINER_WS ?? "";
const inContainer =
  containerBrowser === ""
    ? {}
    : { connectOptions: { wsEndpoint: containerBrowser, exposeNetwork: "<loopback>" } };

const allInContainer = process.env.E2E_ALL_IN_CONTAINER === "1";
if (allInContainer && containerBrowser === "") {
  throw new Error(
    "E2E_ALL_IN_CONTAINER=1 needs the Playwright container's browser, and E2E_CONTAINER_WS is not set. " +
      "Run through scripts/with-playwright-container.sh, or unset E2E_ALL_IN_CONTAINER to use this machine's Chromium.",
  );
}
const chromiumBrowser = allInContainer ? inContainer : {};

const smokeOnBuild = process.env.CI !== undefined || process.env.E2E_SMOKE_ON_BUILD === "1";
const smokeServer = smokeOnBuild
  ? ({ mockServer: "preview", serviceWorkers: "block" } as const)
  : ({} as const);

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
const notSmoke = /@visual|@preview|@dev/;

export default defineConfig<E2EOptions>({
  testDir: "e2e",
  testMatch: "**/*.spec.ts",
  outputDir: "test-results",

  globalSetup: "./e2e/support/warmup.ts",

  workers: workerCount,
  fullyParallel: true,
  forbidOnly: process.env.CI !== undefined,
  retries: 0,
  timeout: 180_000,

  reporter: [["list"], ["html", { open: "never" }]],

  snapshotPathTemplate: "{testDir}/__screenshots__/{testFilePath}/{arg}-{projectName}{ext}",
  expect: {
    toHaveScreenshot: { scale: "css" },
  },

  use: {
    locale: "en-US",
    timezoneId: "UTC",
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

  webServer: [
    ...Array.from({ length: workerCount }, (_, index) => {
      const launch = launchOf("dev", index);
      return {
        name: launch.name,
        command: `npm run dev:mock -- ${launch.listenArgs}`,
        wait: { stdout: launch.listening },
        env: {
          MOCK_DETERMINISTIC: "1",
          MOCK_ARTIFACTS_PORT: String(launch.artifactsPort),
          VITE_CACHE_DIR: `node_modules/.vite-e2e/${String(index)}`,
        },
        timeout: 60_000,
      };
    }),
    ...Array.from({ length: workerCount }, (_, index) => {
      const launch = launchOf("preview", index);
      return {
        name: launch.name,
        command: `${index === 0 ? "npm run build && " : ""}npm run preview:mock -- ${launch.listenArgs}`,
        wait: { stdout: launch.listening },
        env: {
          MOCK_DETERMINISTIC: "1",
          MOCK_ARTIFACTS_PORT: String(launch.artifactsPort),
        },
        timeout: index === 0 ? 180_000 : 60_000,
      };
    }),
  ],
});
