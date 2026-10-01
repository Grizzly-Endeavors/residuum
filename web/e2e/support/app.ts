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

export interface AppReadyOptions {
  /**
   * Where the hub socket should stand: `connected` unless the test has taken
   * the hub down and is looking at the banner, which waits for `lost`.
   */
  hub?: HubState;
}

/** The root of whatever the page shows: the shell, the first-run setup wizard, or the primitives gallery. */
const APP_ROOT = ".shell, .setup-wizard, .gallery";

/**
 * What the app shows while it fetches the code of something it was asked to
 * open: the shell's "Opening" (Settings, the palette) and the panel's "Loading
 * the file". The place that opens a lazy component shows one of these until the
 * component mounts.
 */
const LAZY_LOADING = /^(Opening|Loading the file)$/;

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
  await appRendered(page, options.hub ?? "connected");
  await fontsLoaded(page);
}
