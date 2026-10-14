/**
 * When the app is usable, and how long the specs wait for it.
 *
 * A page that has loaded is not a page that is ready. The app's code runs
 * after the load event, then it fetches the agent list, opens the hub socket
 * and fetches the overview the socket announces; Settings, the palette and the file view fetch their own code the
 * first time they open; and the fonts arrive last. On a quiet machine all of
 * that is over before a spec's first assertion. On a loaded one it isn't, and a
 * spec that acts at once clicks into a half-built page, or shoots the
 * "Can't reach Residuum" banner that shows while the hub socket connects again.
 *
 * `waitForApp` is the single definition of "ready". The fixtures run it after
 * every `page.goto` and `page.reload` (see `fixtures.ts`), and the screenshot
 * helper runs it before every shot, so a spec never repeats it.
 */
import { expect, type Page } from "@playwright/test";

/**
 * How long a wait may take when it is bound to code the dev server hands out:
 * the app starting, a lazy chunk mounting, the gallery loading. The wait ends
 * the moment its condition holds, so this is only the point at which a page
 * that never comes up is reported. It is longer than Playwright's default
 * because those waits cross a cold module graph, which takes several seconds
 * when other suites share the machine; every other wait keeps the default.
 */
export const LOAD_TIMEOUT = 20_000;

/** What the hub socket's state says (`data-hub` on the shell). `lost` is the state the "Can't reach Residuum" banner shows. */
export type HubState = "connected" | "lost";

/**
 * Whether the running test has cut the hub off from its page: the browser's
 * network is off (`context.setOffline(true)`), or the mock's hub socket is down
 * (`POST /api/mock/hub-socket` with `online: false`). The fixtures keep it, and
 * `waitForApp` then expects the hub to be lost, which is the state the banner
 * shows, instead of waiting for a connection the test took away. A worker runs
 * one test at a time, so one record serves it.
 */
export const hubReach = { lost: false };

/**
 * Where the page's clock (`Date`) stands, when the fixtures hold it still:
 * at the mock's fixed start in a visual project, and at the mock's simulated
 * time once a test is in manual time. `null` while it follows the wall clock.
 * The fixtures keep it, and the screenshot helper checks the page against it.
 */
export const pageClock: { fixedAt: number | null } = { fixedAt: null };

export interface AppReadyOptions {
  /**
   * Where the hub socket should stand. Left out, it follows `hubReach`:
   * `connected` unless the test has cut the hub off.
   */
  hub?: HubState;
}

/** The root of whatever the page shows: the shell, the first-run setup wizard, or the primitives gallery. */
const APP_ROOT = ".shell, .setup-wizard, .gallery";

/**
 * What the app shows while it fetches the code or the data of something it was
 * asked to open: the shell's "Opening" (Settings, the palette), the Settings
 * modal's "Loading settings" and the panel's "Loading the file". The place that
 * opens a lazy component shows one of these until the component has mounted
 * and loaded what it shows.
 */
const LAZY_LOADING = /^(Opening|Loading settings|Loading the file)$/;

/** The part of a page that is being built while the app starts: the app's roots, the hub socket and what it brings, the lazy components the URL asked for. */
async function appRendered(page: Page, hub: HubState): Promise<void> {
  await expect(
    page.locator(APP_ROOT).first(),
    "the app should render its shell, the setup wizard or the gallery",
  ).toBeVisible({ timeout: LOAD_TIMEOUT });

  // Setup and the gallery have no hub socket of their own to wait for.
  const shell = page.locator(".shell");
  if ((await shell.count()) === 0) return;
  await expect(shell, `the hub socket should be ${hub}`).toHaveAttribute("data-hub", hub, {
    timeout: LOAD_TIMEOUT,
  });
  // What the socket brings (the rail's counts, which agents are working) is fetched once it says hello. With the hub out of reach there is nothing to wait for.
  if (hub === "connected") {
    await expect(shell, "the hub's overview should have loaded").toHaveAttribute(
      "data-overview",
      "loaded",
      { timeout: LOAD_TIMEOUT },
    );
  }
  // Between the Settings chunk arriving and the modal mounting, neither "Opening" nor "Loading settings" is up, so a settings URL waits for the modal itself.
  if (new URL(page.url()).searchParams.has("settings")) {
    await expect(
      page.getByRole("dialog", { name: "Settings" }),
      "the Settings modal the URL asks for should be open",
    ).toBeVisible({ timeout: LOAD_TIMEOUT });
  }
  await expect(
    page.getByRole("status").filter({ hasText: LAZY_LOADING }),
    "the code a feature opens with should have loaded",
  ).toHaveCount(0, { timeout: LOAD_TIMEOUT });
}

/** Wait until every font the page is using has loaded, so text doesn't swap faces under a click or a screenshot. */
export async function fontsLoaded(page: Page): Promise<void> {
  await page.waitForFunction(() => document.fonts.status === "loaded", undefined, {
    timeout: LOAD_TIMEOUT,
  });
}

/** Wait until every image on the page has loaded, or failed. */
export async function imagesLoaded(page: Page): Promise<void> {
  await page.waitForFunction(() => Array.from(document.images).every((image) => image.complete), {
    timeout: LOAD_TIMEOUT,
  });
}

/**
 * Wait for every animation that ends to finish. A layer still fading in has
 * its text at part opacity, which axe measures as low contrast, and a
 * screenshot shows mid-flight.
 */
export async function settleAnimations(page: Page): Promise<void> {
  await page.evaluate(() =>
    Promise.all(
      document
        .getAnimations()
        .filter((animation) => animation.effect?.getComputedTiming().endTime !== Infinity)
        .map((animation) => animation.finished.catch(() => undefined)),
    ),
  );
}

/**
 * Wait for the app to be usable: its root rendered, the hub socket connected
 * and its overview loaded, the code behind anything the URL opens mounted, and
 * the fonts loaded.
 * Waiting on these is a condition, never a delay: it returns at once on a page
 * that is already there.
 */
export async function waitForApp(page: Page, options: AppReadyOptions = {}): Promise<void> {
  await appRendered(page, options.hub ?? (hubReach.lost ? "lost" : "connected"));
  await fontsLoaded(page);
}
