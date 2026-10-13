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
import { expect, type Locator, type Page } from "@playwright/test";
import { fontsLoaded, imagesLoaded, pageClock, settleAnimations, waitForApp } from "./app";

export interface ScreenshotOptions {
  /** Regions to paint over in this screenshot. */
  mask?: readonly Locator[];
}

/** A rectangle of the window, in CSS pixels from its top left corner. */
export interface Clip {
  x: number;
  y: number;
  width: number;
  height: number;
}

/**
 * Wait for the page to be usable for a shot: the clock frozen, the app up, the
 * animations that end done, and the fonts and images the page uses loaded.
 */
async function settle(page: Page): Promise<void> {
  const now = await page.evaluate(() => Date.now());
  expect(
    now,
    "the page's clock must stand at the mock's time for screenshots: run the spec in a visual project",
  ).toBe(pageClock.fixedAt);

  // The socket can drop between the spec's last wait and here, which shows the
  // "Can't reach Residuum" banner until it reconnects.
  await waitForApp(page);
  await settleAnimations(page);
  // What the spec opened since the page loaded can draw glyphs from a font file that has not loaded yet.
  await fontsLoaded(page);
  await imagesLoaded(page);
}

/**
 * Compare `target` with the baseline called `name`. Wait for the component to
 * reach the state under test first: the helper only waits for the app to be
 * usable, the animations that end, and the fonts and images the page uses.
 */
export async function expectScreenshot(
  target: Locator,
  name: string,
  options: ScreenshotOptions = {},
): Promise<void> {
  await settle(target.page());
  await expect(target).toHaveScreenshot(`${name}.png`, {
    animations: "disabled",
    caret: "hide",
    mask: options.mask ? [...options.mask] : [],
  });
}

/**
 * Compare a rectangle of the window with the baseline called `name`, for a
 * component whose parts do not share one element: a reply's hover details sit
 * outside the reply's box, so no locator's box holds both. The spec builds the
 * rectangle from the boxes of the component's own parts.
 */
export async function expectClipScreenshot(page: Page, clip: Clip, name: string): Promise<void> {
  await settle(page);
  await expect(page).toHaveScreenshot(`${name}.png`, {
    animations: "disabled",
    caret: "hide",
    clip,
  });
}
