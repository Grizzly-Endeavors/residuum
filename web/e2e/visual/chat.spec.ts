import type { Locator, Page } from "@playwright/test";
import { sendFromComposer } from "../support/composer";
import { expect, test } from "../support/fixtures";
import { expectClipScreenshot, expectScreenshot } from "../support/screenshot";

const GREETING = "Hi, this is atlas. You are in my conversation, not scout's.";
const PNG = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==",
  "base64",
);

function conversation(page: Page, agent = "atlas"): Locator {
  return page.getByRole("region", { name: `Conversation with ${agent}` });
}

function messageBox(page: Page): Locator {
  return page.getByRole("combobox", { name: "Message atlas" });
}

function turnWith(feed: Locator, content: Locator): Locator {
  return feed.locator("[data-feed-turn]").filter({ has: content });
}

test.describe("chat", { tag: "@visual" }, () => {
  test.skip(({ isMobile }) => isMobile, "The chat's parts are shot at the desktop size.");

  test("a finished turn's activity line, open to a step's details", async ({ page }) => {
    await page.goto("/agent/atlas");
    const feed = conversation(page);
    await expect(feed.getByText(GREETING)).toBeInViewport();
    const activity = page.getByRole("button", { name: "Ran 1 command" });
    await activity.click();
    const step = feed.getByRole("button", { name: "Ran residuum memory stats" });
    await step.click();
    const details = page.locator(`#${(await step.getAttribute("aria-controls")) ?? ""}`);
    await expect(details).toContainText("Last observer run: 3 minutes ago");
    const turn = turnWith(feed, activity);
    await turn.evaluate((el) => {
      el.dispatchEvent(new WheelEvent("wheel", { bubbles: true }));
      el.scrollIntoView({ block: "center" });
    });
    await page.mouse.move(0, 0);
    await expectScreenshot(turn, "chat-turn");
  });

  test("a turn running, with its steps", async ({ page, mock }) => {
    await page.goto("/agent/atlas");
    const feed = conversation(page);
    await expect(feed.getByText(GREETING)).toBeInViewport();
    await mock.manualTime();
    await messageBox(page).fill("Check the wiki index");
    await sendFromComposer(messageBox(page));
    await expect(feed.getByText("Working", { exact: true })).toBeVisible();
    await mock.advance(1_000);
    await expect(feed.getByRole("button", { name: "Read team/wiki/index.md" })).toBeVisible();
    await expect(
      feed.getByRole("button", { name: "Reading team/wiki/projects/residuum.md, running" }),
    ).toBeVisible();
    await expect(page.getByRole("main").locator("[data-working]").first()).toBeAttached();
    const head = page.locator("[data-turn-head]");
    await expect(head.getByText("1s", { exact: true })).toBeVisible();
    await page.mouse.move(0, 0);
    await expectScreenshot(turnWith(feed, head), "chat-turn-live");
  });

  test("a reply's time and Copy, shown on hover", async ({ page }) => {
    await page.setViewportSize({ width: page.viewportSize()?.width ?? 0, height: 420 });
    await page.goto("/agent/atlas");
    const reply = conversation(page).locator(".feed-message").filter({ hasText: GREETING });
    await expect(reply).toBeInViewport();
    await expect(page.getByRole("button", { name: /^Model: Claude Sonnet 4\.6/ })).toBeAttached();
    await reply.hover();
    await expect(reply.locator("time").first()).toHaveCSS("opacity", "1");
    await expectClipScreenshot(
      [reply, reply.getByRole("button", { name: "Copy reply" }), reply.locator("time").first()],
      "message-details",
    );
  });

  test("the composer with a draft and an attached image", async ({ page }) => {
    await page.goto("/agent/atlas");
    await expect(conversation(page).getByText(GREETING)).toBeInViewport();
    await page
      .locator('input[type="file"]')
      .setInputFiles({ name: "shot.png", mimeType: "image/png", buffer: PNG });
    await expect(page.getByRole("img", { name: "Image 1" })).toBeVisible();
    await messageBox(page).fill("Can you read the error in this screenshot?");
    await expect(page.getByRole("button", { name: /^Model: Claude Sonnet 4\.6/ })).toBeAttached();
    await expectScreenshot(page.getByRole("form", { name: "Message atlas" }), "composer");
  });

  test("a Markdown reply: headings, checklist, code and a question", async ({ page }) => {
    await page.goto("/agent/atlas");
    const feed = conversation(page);
    await expect(feed.getByText(GREETING)).toBeVisible();
    await messageBox(page).fill("markdown please");
    await sendFromComposer(messageBox(page));
    const reply = feed.locator(".feed-message").filter({ hasText: "Rollout notes" });
    await expect(reply.getByText("Should parked notifications expire after a week")).toBeVisible();
    await expect(page.getByRole("button", { name: /^Model: Claude Sonnet 4\.6/ })).toBeAttached();
    await page.mouse.move(0, 0);
    await expectScreenshot(reply, "prose");
  });

  test("an agent that couldn't start, with its details open", async ({ page }) => {
    await page.goto("/agent/brittle");
    const failed = page.getByRole("region", { name: "brittle couldn't start" });
    await failed.getByRole("button", { name: "Details" }).click();
    await expect(failed.getByText(/is not offered by provider/)).toBeVisible();
    await expectScreenshot(failed, "state-card");
  });
});
