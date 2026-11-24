import { expect, type Locator, type Page } from "@playwright/test";
import { fontsLoaded, imagesLoaded, pageClock, settleAnimations, waitForApp } from "./app";

export interface ScreenshotOptions {
  mask?: readonly Locator[];
}

async function settle(page: Page): Promise<void> {
  const now = await page.evaluate(() => Date.now());
  expect(
    now,
    "the page's clock must stand at the mock's time for screenshots: run the spec in a visual project",
  ).toBe(pageClock.fixedAt);

  await waitForApp(page);
  await settleAnimations(page);
  await fontsLoaded(page);
  await imagesLoaded(page);
}

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

export async function expectClipScreenshot(
  parts: readonly [Locator, ...Locator[]],
  name: string,
  options: ScreenshotOptions = {},
): Promise<void> {
  const [first] = parts;
  const page = first.page();
  await settle(page);
  const boxes = await Promise.all(parts.map((part) => part.boundingBox()));
  const laidOut = boxes.filter((box) => box !== null);
  expect(laidOut, "every part must be laid out").toHaveLength(parts.length);
  const left = Math.floor(Math.min(...laidOut.map((box) => box.x)));
  const top = Math.floor(Math.min(...laidOut.map((box) => box.y)));
  const right = Math.ceil(Math.max(...laidOut.map((box) => box.x + box.width)));
  const bottom = Math.ceil(Math.max(...laidOut.map((box) => box.y + box.height)));
  await expect(page).toHaveScreenshot(`${name}.png`, {
    animations: "disabled",
    caret: "hide",
    clip: { x: left, y: top, width: right - left, height: bottom - top },
    mask: options.mask ? [...options.mask] : [],
  });
}
