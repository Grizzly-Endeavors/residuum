import type { Locator, Page } from "@playwright/test";
import { sendFromComposer } from "../support/composer";
import { expect, test } from "../support/fixtures";
import { solidPng } from "../support/png";
import { expectScreenshot } from "../support/screenshot";

/**
 * The details a message keeps quiet, shown: a reply's time and Copy on hover or
 * a tap, a user message's time, and a picture opened full size, alone in the
 * view and as one of a message's set.
 */

const GREETING = "Hi, this is atlas. You are in my conversation, not scout's.";
const WIDE = solidPng(1600, 900, [40, 90, 160]);
const TALL = solidPng(600, 1200, [60, 140, 90]);

function conversation(page: Page): Locator {
  return page.getByRole("region", { name: "Conversation with atlas" });
}

/** Show a message's quiet details the way the device does: a hover with a pointer, a tap on a touch screen. */
async function reveal(message: Locator, isMobile: boolean): Promise<void> {
  if (isMobile) await message.locator(".prose, .user-bubble").first().tap();
  else await message.hover();
  await expect(message.locator("time").first()).toHaveCSS("opacity", "1");
}

test.describe("message details", { tag: "@visual" }, () => {
  test("a reply's time and Copy", async ({ page, isMobile }) => {
    await page.goto("/agent/atlas");
    const reply = conversation(page).locator(".feed-message").filter({ hasText: GREETING });
    await expect(reply).toBeInViewport();
    await reveal(reply, isMobile);
    await expectScreenshot(page, "chat-reply-details");
  });

  test("a user message's time", async ({ page, isMobile }) => {
    await page.goto("/agent/atlas");
    const message = conversation(page)
      .locator(".feed-message")
      .filter({ has: page.locator(".user-message") })
      .last();
    await expect(message).toBeInViewport();
    await reveal(message, isMobile);
    await expectScreenshot(page, "chat-user-message-details");
  });
});

test.describe("image viewer", { tag: "@visual" }, () => {
  test("a message's pictures, opened full size", async ({ page }) => {
    await page.goto("/agent/atlas");
    await expect(conversation(page).getByText(GREETING)).toBeVisible();
    await page.locator('input[type="file"]').setInputFiles([
      { name: "wide.png", mimeType: "image/png", buffer: WIDE },
      { name: "tall.png", mimeType: "image/png", buffer: TALL },
    ]);
    const box = page.getByRole("combobox", { name: "Message atlas" });
    await box.fill("Here are the two pictures");
    await sendFromComposer(box);
    const first = conversation(page).getByRole("button", {
      name: "View full size: Attached image 1",
    });
    await expect(first).toBeVisible();
    // The turn the message started has ended, so nothing changes behind the viewer.
    await expect(page.getByRole("button", { name: "Stop the reply" })).toHaveCount(0);

    await first.click();
    const viewer = page.getByRole("dialog", { name: "Attached image 1" });
    await expect(viewer.getByRole("img")).toBeVisible();
    await expectScreenshot(page, "chat-image-viewer");

    await viewer.getByRole("button", { name: "Next image" }).click();
    await expect(page.getByRole("dialog", { name: "Attached image 2" })).toBeVisible();
    await expectScreenshot(page, "chat-image-viewer-set");
  });
});
