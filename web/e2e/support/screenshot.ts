/**
 * Visual comparison: one component against a committed baseline.
 *
 * Rendering is only repeatable inside the Playwright container, so visual specs
 * run in the `visual-*` projects, which skip everywhere else (see
 * `fixtures.ts`). Those projects also freeze the page's clock at the mock's
 * clock (moving it only with the mock's manual time), and the helper disables
 * animations and hides the caret on top of it.
 *
 * The shot is of the element a spec passes, so a baseline changes only when
 * that component does. Pass `mask` for a region inside it that changes from
 * run to run.
 */
import { expect, type Locator } from "@playwright/test";
import {
  fontsLoaded,
  imagesLoaded,
  pageClock,
  settleAnimations,
  waitForApp,
  type AppReadyOptions,
} from "./app";

export interface ScreenshotOptions extends AppReadyOptions {
  /** Regions to paint over in this screenshot. */
  mask?: readonly Locator[];
}

/**
 * Compare `target` with the baseline called `name`. Wait for the component to
 * reach the state under test first: the helper only waits for the app, the hub
 * socket (lost, when the test has taken the hub down to shoot the banner), the
 * animations that end, and the fonts and images the page uses.
 */
export async function expectScreenshot(
  target: Locator,
  name: string,
  options: ScreenshotOptions = {},
): Promise<void> {
  const page = target.page();
  const now = await page.evaluate(() => Date.now());
  expect(
    now,
    "the page's clock must stand at the mock's time for screenshots: run the spec in a visual project",
  ).toBe(pageClock.fixedAt);

  // The socket can drop between the spec's last wait and here, which shows the
  // "Can't reach Residuum" banner until it reconnects.
  await waitForApp(page, { hub: options.hub });
  await settleAnimations(page);
  // What the spec opened since the page loaded can draw glyphs from a font file that has not loaded yet.
  await fontsLoaded(page);
  await imagesLoaded(page);

  await expect(target).toHaveScreenshot(`${name}.png`, {
    animations: "disabled",
    caret: "hide",
    mask: options.mask ? [...options.mask] : [],
  });
}
