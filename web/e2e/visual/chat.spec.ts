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

/** The hub socket is up: the inbox count comes from the overview it brings. */
async function hubConnected(page: Page): Promise<void> {
  await expect(page.getByRole("link", { name: /^Inbox.*\d+ unread/ }).first()).toBeAttached();
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

  test("a turn's activity line, open to a step's details", async ({ page }) => {
    await page.goto("/agent/atlas");
    const feed = conversation(page);
    await expect(feed.getByText(GREETING)).toBeInViewport();
    await hubConnected(page);
    await feed.getByRole("button", { name: "Ran 1 command" }).click();
    const step = feed.getByRole("button", { name: "Ran residuum memory stats" });
    await step.click();
    await expect(feed.getByText("Context window: 12,847 / 200,000 tokens (6.4%)")).toBeVisible();
    await step.evaluate((el) => {
      el.scrollIntoView({ block: "start" });
    });
    await chatScreenshot(page, "chat-activity");
  });

  test("a turn running, with its steps", async ({ page, mock }) => {
    test.setTimeout(90_000);
    // Seconds between steps, so the line holds still for the shot: one read
    // done and the other running, from 13.5s into the turn to 18s.
    await mock.post("/api/mock/delays", { data: { scale: 15 } });
    await page.goto("/agent/atlas");
    const feed = conversation(page);
    await expect(feed.getByText(GREETING)).toBeInViewport();
    await hubConnected(page);
    await page.getByRole("textbox", { name: "Send a message..." }).fill("Check the wiki index");
    await page.getByRole("textbox", { name: "Send a message..." }).press("Enter");
    await expect(feed.getByRole("button", { name: "Read team/wiki/index.md" })).toBeVisible({
      timeout: 30_000,
    });
    await expect(
      feed.getByRole("button", { name: "Reading team/wiki/projects/residuum.md, running" }),
    ).toBeVisible();
    // The hub has heard the agent is busy: the header's mark is working.
    await expect(page.getByRole("main").locator("[data-working]").first()).toBeAttached();
    await chatScreenshot(page, "chat-live-turn");
  });

  test("the header's menu", async ({ page }) => {
    await page.goto("/agent/atlas");
    await expect(conversation(page).getByText(GREETING)).toBeInViewport();
    await page.getByRole("button", { name: "More for atlas" }).click();
    await expect(page.getByRole("menu", { name: "More for atlas" })).toBeVisible();
    await chatScreenshot(page, "chat-menu");
  });
});
