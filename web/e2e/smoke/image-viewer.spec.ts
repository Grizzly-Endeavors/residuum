import type { Locator, Page } from "@playwright/test";
import { expectNoAxeViolations } from "../support/axe";
import { sendFromComposer } from "../support/composer";
import { expect, test } from "../support/fixtures";
import { solidPng } from "../support/png";

/**
 * A picture in the conversation opened full size: the images a message was
 * sent with, fitted to the view, in a modal layer that traps focus, closes on
 * Esc and gives focus back, with arrow keys between a message's pictures.
 */

const GREETING = "Hi, this is atlas. You are in my conversation, not scout's.";

const WIDE = solidPng(1600, 900, [40, 90, 160]);
const TALL = solidPng(600, 1200, [60, 140, 90]);

function conversation(page: Page): Locator {
  return page.getByRole("region", { name: "Conversation with atlas" });
}

/** Send a message with `images`, and wait for them to show in the conversation. */
async function sendPictures(page: Page, images: Buffer[]): Promise<void> {
  await page.goto("/agent/atlas");
  await expect(page.getByText(GREETING)).toBeVisible();
  await page.locator('input[type="file"]').setInputFiles(
    images.map((buffer, index) => ({
      name: `picture-${String(index + 1)}.png`,
      mimeType: "image/png",
      buffer,
    })),
  );
  const box = page.getByRole("combobox", { name: "Message atlas" });
  await box.fill("Here are the pictures");
  await sendFromComposer(box);
  await expect(
    conversation(page).getByRole("button", { name: "View full size: Attached image 1" }),
  ).toBeVisible();
}

test("a picture opens full size, fitted to the view, and Esc gives focus back", async ({
  page,
}) => {
  await sendPictures(page, [WIDE]);
  const thumbnail = conversation(page).getByRole("button", {
    name: "View full size: Attached image 1",
  });
  await thumbnail.click();

  const viewer = page.getByRole("dialog", { name: "Attached image 1" });
  const picture = viewer.getByRole("img", { name: "Attached image 1" });
  await expect(picture).toBeVisible();
  await expect.poll(() => picture.evaluate((img: HTMLImageElement) => img.complete)).toBe(true);

  // Far larger than the 160px crop in the message, and inside the view with its margin.
  const size = await picture.boundingBox();
  const view = page.viewportSize();
  if (size === null || view === null) throw new Error("the picture or the view has no size");
  expect(size.width).toBeGreaterThan(300);
  expect(size.x).toBeGreaterThanOrEqual(0);
  expect(size.y).toBeGreaterThanOrEqual(0);
  expect(size.x + size.width).toBeLessThanOrEqual(view.width);
  expect(size.y + size.height).toBeLessThanOrEqual(view.height);
  // Its own shape, not cropped.
  expect(size.width / size.height).toBeCloseTo(1600 / 900, 1);

  // One picture has nothing to step to.
  await expect(viewer.getByRole("button", { name: "Next image" })).toHaveCount(0);
  await expectNoAxeViolations(page);

  await page.keyboard.press("Escape");
  await expect(viewer).toHaveCount(0);
  await expect(thumbnail).toBeFocused();
});

test("a tall picture fits the view as well", async ({ page }) => {
  await sendPictures(page, [TALL]);
  await conversation(page)
    .getByRole("button", { name: "View full size: Attached image 1" })
    .click();
  const picture = page.getByRole("dialog").getByRole("img");
  await expect.poll(() => picture.evaluate((img: HTMLImageElement) => img.complete)).toBe(true);
  const size = await picture.boundingBox();
  const view = page.viewportSize();
  if (size === null || view === null) throw new Error("the picture or the view has no size");
  expect(size.y + size.height).toBeLessThanOrEqual(view.height);
  expect(size.height).toBeGreaterThan(view.height / 2);
  expect(size.width / size.height).toBeCloseTo(600 / 1200, 1);
});

test("the arrow keys and the buttons move between a message's pictures, wrapping at the ends", async ({
  page,
  isMobile,
}) => {
  await sendPictures(page, [WIDE, TALL]);
  await conversation(page)
    .getByRole("button", { name: "View full size: Attached image 2" })
    .click();
  const viewer = page.getByRole("dialog");
  await expect(viewer).toHaveAccessibleName("Attached image 2");
  await expect(viewer.locator(".viewer-place")).toHaveText("2 of 2");

  if (isMobile) {
    await viewer.getByRole("button", { name: "Next image" }).click();
  } else {
    await page.keyboard.press("ArrowRight");
  }
  await expect(viewer).toHaveAccessibleName("Attached image 1");
  await expect(viewer.locator(".viewer-place")).toHaveText("1 of 2");

  if (isMobile) {
    await viewer.getByRole("button", { name: "Previous image" }).click();
  } else {
    await page.keyboard.press("ArrowLeft");
  }
  await expect(viewer).toHaveAccessibleName("Attached image 2");
  await expectNoAxeViolations(page);

  // Back closes it on a phone, Esc on a desktop; the close button does for both.
  await viewer.getByRole("button", { name: "Close" }).click();
  await expect(viewer).toHaveCount(0);
});

test("focus stays in the viewer while Tab is pressed, and the page behind it can't be reached", async ({
  page,
}) => {
  await sendPictures(page, [WIDE]);
  await conversation(page)
    .getByRole("button", { name: "View full size: Attached image 1" })
    .click();
  await expect(page.getByRole("dialog")).toBeVisible();

  // Only the viewer's close button can take focus, with a tooltip showing on it.
  const inViewer = (): Promise<boolean> =>
    page.evaluate(() => document.activeElement?.closest("[data-overlay-host]") != null);
  for (let press = 0; press < 3; press += 1) {
    await page.keyboard.press("Tab");
    expect(await inViewer()).toBe(true);
  }
  await expect(page.getByRole("button", { name: "Close" })).toBeFocused();
  // The page behind it is inert.
  expect(
    await page
      .locator(".shell")
      .evaluate((shell: HTMLElement) => shell.closest("[inert]") !== null),
  ).toBe(true);
});
