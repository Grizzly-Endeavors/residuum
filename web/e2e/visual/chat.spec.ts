import type { Locator, Page } from "@playwright/test";
import { sendFromComposer } from "../support/composer";
import { expect, test } from "../support/fixtures";
import { expectScreenshot } from "../support/screenshot";

/**
 * The chat's baselines: the latest messages, the summarized past with Jump to
 * latest, the state cards of a stopped and a failed agent, the header's menu,
 * a running turn, a turn working in rounds and the same finished, reasoning
 * opened, a turn that couldn't finish, and a message from Telegram, and the
 * composer with its `/` menu, an attached image, and the model and thinking
 * control open.
 */

const GREETING = "Hi, this is atlas. You are in my conversation, not scout's.";
/** A 1×1 PNG. */
const PNG = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==",
  "base64",
);

function conversation(page: Page, agent = "atlas"): Locator {
  return page.getByRole("region", { name: `Conversation with ${agent}` });
}

/** A running agent's composer has read its model. */
async function chatScreenshot(page: Page, name: string, running = true): Promise<void> {
  if (running) {
    await expect(page.getByRole("button", { name: /^Model: Claude Sonnet 4\.6/ })).toBeAttached();
  }
  await expectScreenshot(page, name);
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
      await expect(feed.getByRole("separator", { name: /, ep-001$/ })).toBeAttached({
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
    await chatScreenshot(page, "chat-stopped", false);
  });

  test("an agent that couldn't start, with its details open", async ({ page }) => {
    await page.goto("/agent/brittle");
    const failed = page.getByRole("region", { name: "brittle couldn't start" });
    await failed.getByRole("button", { name: "Details" }).click();
    await expect(failed.getByText(/is not offered by provider/)).toBeVisible();
    await chatScreenshot(page, "chat-failed", false);
  });

  test("a stopped agent under its conversation", async ({ page }) => {
    await page.request.post("/api/hub/agents/atlas/stop");
    await page.goto("/agent/atlas");
    await expect(page.getByRole("region", { name: "atlas is stopped" })).toBeInViewport();
    await expect(conversation(page).getByText(GREETING)).toBeVisible();
    await chatScreenshot(page, "chat-stopped-below", false);
  });

  test("a turn's activity line, open to a step's details", async ({ page }) => {
    await page.goto("/agent/atlas");
    const feed = conversation(page);
    await expect(feed.getByText(GREETING)).toBeInViewport();
    await feed.getByRole("button", { name: "Ran 1 command" }).click();
    const step = feed.getByRole("button", { name: "Ran residuum memory stats" });
    await step.click();
    const details = page.locator(`#${(await step.getAttribute("aria-controls")) ?? ""}`);
    await expect(details).toContainText("Last observer run: 3 minutes ago");
    await step.evaluate((el) => {
      el.scrollIntoView({ block: "center" });
    });
    await page.mouse.move(0, 0);
    await chatScreenshot(page, "chat-activity");
  });

  test("a turn running, with its steps", async ({ page, mock }) => {
    await page.goto("/agent/atlas");
    const feed = conversation(page);
    await expect(feed.getByText(GREETING)).toBeInViewport();
    await mock.manualTime();
    await page.getByRole("combobox", { name: "Message atlas" }).fill("Check the wiki index");
    await sendFromComposer(page.getByRole("combobox", { name: "Message atlas" }));
    await expect(feed.getByText("Working", { exact: true })).toBeVisible();
    // Time stands still for the shot, at 1s into the turn: one read done (it ends at 0.9s)
    // and the other running (it ends at 1.2s).
    await mock.advance(1_000);
    await expect(feed.getByRole("button", { name: "Read team/wiki/index.md" })).toBeVisible();
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

test.describe("a turn in the chat", { tag: "@visual" }, () => {
  test("working in rounds: runs of steps between what the agent says, the head last", async ({
    page,
    mock,
  }) => {
    // The turn waits with its last edit done, so the layout holds still.
    await mock.post("/api/mock/turn-hold", { data: { held: true } });
    await page.goto("/agent/atlas");
    const feed = conversation(page);
    await expect(feed.getByText(GREETING)).toBeInViewport();
    await page.getByRole("combobox", { name: "Message atlas" }).fill("segments: fix the port");
    await sendFromComposer(page.getByRole("combobox", { name: "Message atlas" }));
    await expect(
      feed.getByRole("button", { name: /^Edited team\/wiki\/config\.toml/ }),
    ).toBeVisible();
    await expect(feed.getByText("Working", { exact: true })).toBeVisible();
    await page.mouse.move(0, 0);
    await chatScreenshot(page, "chat-turn-rounds");
  });

  test("a finished turn: its rounds as lines, and how long it took", async ({ page }) => {
    await page.goto("/agent/atlas");
    const feed = conversation(page);
    await expect(feed.getByText(GREETING)).toBeInViewport();
    await page.getByRole("combobox", { name: "Message atlas" }).fill("segments: fix the port");
    await sendFromComposer(page.getByRole("combobox", { name: "Message atlas" }));
    await expect(feed.getByText("Done. The port is set once now")).toBeVisible();
    await expect(feed.getByText("Working", { exact: true })).toHaveCount(0);
    await page.mouse.move(0, 0);
    await chatScreenshot(page, "chat-turn-done");
  });

  test("reasoning, folded to a line and opened to all of it", async ({ page }) => {
    await page.goto("/agent/atlas");
    const feed = conversation(page);
    await expect(feed.getByText(GREETING)).toBeInViewport();
    await page
      .getByRole("combobox", { name: "Message atlas" })
      .fill("think about the fallback order");
    await sendFromComposer(page.getByRole("combobox", { name: "Message atlas" }));
    await expect(feed.getByText(/^Retry three times with backoff/)).toBeVisible();
    await feed.getByRole("button", { name: "Thought" }).click();
    await expect(feed.getByText(/I should answer with that order/)).toBeVisible();
    await page.mouse.move(0, 0);
    await chatScreenshot(page, "chat-thought");
  });

  test("a turn that couldn't finish, with its details open", async ({ page }) => {
    await page.goto("/agent/atlas");
    const feed = conversation(page);
    await expect(feed.getByText(GREETING)).toBeInViewport();
    await page.getByRole("combobox", { name: "Message atlas" }).fill("error: check the wiki");
    await sendFromComposer(page.getByRole("combobox", { name: "Message atlas" }));
    await expect(feed.getByText("atlas couldn't finish this reply")).toBeVisible();
    // The toast for the same failure would stand over the shot.
    await page
      .getByRole("alert")
      .filter({ hasText: "The model provider didn't answer" })
      .getByRole("button", { name: "Dismiss" })
      .click();
    await feed.getByRole("button", { name: "Details" }).click();
    await expect(feed.getByText(/provider returned 503/)).toBeVisible();
    await page.mouse.move(0, 0);
    await chatScreenshot(page, "chat-turn-failed");
  });

  test("a message from Telegram, and the reply sent back there", async ({ page, mock }) => {
    await page.goto("/agent/atlas");
    const feed = conversation(page);
    await expect(feed.getByText(GREETING)).toBeInViewport();
    await mock.post("/api/mock/telegram-message", { params: { agent: "atlas" } });
    await expect(feed.getByText("Sent to Telegram")).toBeVisible();
    await expect(feed.getByText("Working", { exact: true })).toHaveCount(0);
    await page.mouse.move(0, 0);
    await chatScreenshot(page, "chat-telegram");
  });
});
test.describe("composer", { tag: "@visual" }, () => {
  test("a draft with an image attached, and the / menu open", async ({ page }) => {
    await page.goto("/agent/atlas");
    await expect(conversation(page).getByText(GREETING)).toBeInViewport();
    await page
      .locator('input[type="file"]')
      .setInputFiles({ name: "shot.png", mimeType: "image/png", buffer: PNG });
    await expect(page.getByRole("img", { name: "Image 1" })).toBeVisible();
    const box = page.getByRole("combobox", { name: "Message atlas" });
    await box.click();
    await page.keyboard.type("/");
    await expect(page.getByRole("listbox", { name: "Chat actions" })).toBeVisible();
    await chatScreenshot(page, "composer-menu");
  });

  test("the model and thinking control, a popover or a sheet on a phone", async ({ page }) => {
    await page.goto("/agent/atlas");
    await expect(conversation(page).getByText(GREETING)).toBeInViewport();
    await page.getByRole("button", { name: /^Model: Claude Sonnet 4\.6/ }).click();
    const chooser = page.getByRole("dialog", { name: "Model for atlas" });
    await expect(chooser.getByRole("button", { name: "Claude Haiku 4.5" })).toBeVisible();
    await page.mouse.move(0, 0);
    await expectScreenshot(page, "composer-model");
  });

  test("waiting for the connection", async ({ page }) => {
    await page.routeWebSocket(/\/api\/agents\/atlas\/ws$/, (socket) => {
      void socket.close();
    });
    await page.goto("/agent/atlas");
    await expect(conversation(page).getByText(GREETING)).toBeInViewport();
    const box = page.getByRole("combobox", { name: "Message atlas" });
    await box.fill("Are you there?");
    await sendFromComposer(box);
    await expect(
      page.getByText("Reconnecting — 1 message will send once back online."),
    ).toBeVisible();
    await chatScreenshot(page, "composer-offline");
  });
});
