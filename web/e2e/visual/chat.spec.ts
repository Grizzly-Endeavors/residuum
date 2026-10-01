import type { Locator, Page } from "@playwright/test";
import { expect, test } from "../support/fixtures";
import { expectScreenshot } from "../support/screenshot";

/**
 * The chat feed's baselines: the latest messages, the summarized past with
 * Jump to latest, the state cards of a stopped and a failed agent, and the
 * header's menu. The composer and the running-turn line are legacy views,
 * painted over until their units give them baselines.
 */

const GREETING = "Hi, this is atlas. You are in my conversation, not scout's.";

function conversation(page: Page, agent = "atlas"): Locator {
  return page.getByRole("region", { name: `Conversation with ${agent}` });
}

async function chatScreenshot(page: Page, name: string): Promise<void> {
  await expectScreenshot(page, name, { mask: [page.locator("[data-legacy-view]")] });
}

test.describe("chat feed", { tag: "@visual" }, () => {
  test("the latest messages", async ({ page }) => {
    await page.goto("/agent/atlas");
    await expect(conversation(page).getByText(GREETING)).toBeInViewport();
    await chatScreenshot(page, "chat-feed");
  });

  test("the summarized past, with Jump to latest", async ({ page }) => {
    await page.goto("/agent/atlas");
    const feed = conversation(page);
    await expect(feed.getByText(GREETING)).toBeInViewport();
    // Every episode first, so nothing loads under the shot.
    await expect(async () => {
      await feed.evaluate((el) => {
        el.scrollTop = 0;
      });
      await expect(feed.getByRole("separator", { name: /^ep-001 · / })).toBeAttached({
        timeout: 1000,
      });
    }).toPass();
    await expect(feed.getByText("Loading earlier messages…")).toHaveCount(0);

    await feed.getByRole("note").evaluate((el) => {
      el.scrollIntoView({ block: "center" });
    });
    await expect(page.getByRole("button", { name: "Jump to latest" })).toBeVisible();
    await chatScreenshot(page, "chat-history");
  });

  test("a stopped agent with no conversation", async ({ page }) => {
    await page.goto("/agent/drifter");
    await expect(page.getByRole("region", { name: "drifter is stopped" })).toBeVisible();
    await chatScreenshot(page, "chat-stopped");
  });

  test("an agent that couldn't start, with its details open", async ({ page }) => {
    await page.goto("/agent/brittle");
    const failed = page.getByRole("region", { name: "brittle couldn't start" });
    await failed.getByRole("button", { name: "Details" }).click();
    await expect(failed.getByText(/is not offered by provider/)).toBeVisible();
    await chatScreenshot(page, "chat-failed");
  });

  test("a stopped agent under its conversation", async ({ page }) => {
    await page.request.post("/api/hub/agents/atlas/stop");
    await page.goto("/agent/atlas");
    await expect(page.getByRole("region", { name: "atlas is stopped" })).toBeInViewport();
    await expect(conversation(page).getByText(GREETING)).toBeVisible();
    await chatScreenshot(page, "chat-stopped-below");
  });

  test("the header's menu", async ({ page }) => {
    await page.goto("/agent/atlas");
    await expect(conversation(page).getByText(GREETING)).toBeInViewport();
    await page.getByRole("button", { name: "More for atlas" }).click();
    await expect(page.getByRole("menu", { name: "More for atlas" })).toBeVisible();
    await chatScreenshot(page, "chat-menu");
  });
});
