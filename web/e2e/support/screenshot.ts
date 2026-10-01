/**
 * Visual comparison: the page against a committed baseline.
 *
 * Rendering is only repeatable inside the Playwright container, so visual specs
 * run in the `visual-*` projects, which skip everywhere else (see
 * `fixtures.ts`). Those projects also freeze the page's clock at the mock's
 * clock, and the helper disables animations and hides the caret on top of it.
 *
 * Put `data-visual-mask` on an element whose content legitimately changes from
 * run to run and every screenshot paints over it. Pass `mask` for a region one
 * screenshot needs covered.
 */
import { expect, type Locator, type Page } from "@playwright/test";
import { FIXED_START_MS } from "../../mock/env";

export const MASK_ATTRIBUTE = "data-visual-mask";

export interface ScreenshotOptions {
  /** Regions to paint over in this screenshot, beyond the `data-visual-mask` ones. */
  mask?: readonly Locator[];
  /** Capture the whole scrollable page instead of the viewport. */
  fullPage?: boolean;
}

/**
 * Compare `page` with the baseline called `name`. Wait for the page to reach
 * the state under test first: the helper only waits for fonts.
 */
export async function expectScreenshot(
  page: Page,
  name: string,
  options: ScreenshotOptions = {},
): Promise<void> {
  const now = await page.evaluate(() => Date.now());
  expect(
    now,
    "the page's clock must be frozen for screenshots: run the spec in a visual project",
  ).toBe(FIXED_START_MS);

  await page.evaluate(async () => {
    await document.fonts.ready;
  });

  await expect(page).toHaveScreenshot(`${name}.png`, {
    animations: "disabled",
    caret: "hide",
    fullPage: options.fullPage ?? false,
    mask: [page.locator(`[${MASK_ATTRIBUTE}]`), ...(options.mask ?? [])],
  });
}
