import type { Locator, Page } from "@playwright/test";
import { expectNoAxeViolations } from "../support/axe";
import { expect, test } from "../support/fixtures";

/**
 * The activity line (design §4): a live turn's steps, timer and Stop, Esc,
 * the line collapsing to its summary, a turn from history opened to a
 * step's details, joining a turn already running, and a session's
 * transcript.
 */

const GREETING = "Hi, this is atlas. You are in my conversation, not scout's.";
const RESEARCH = "Compare fallback strategies for notification delivery";

function conversation(page: Page): Locator {
  return page.getByRole("region", { name: "Conversation with atlas" });
}

function composer(page: Page): Locator {
  return page.getByRole("textbox", { name: "Message atlas" });
}

async function send(page: Page, text: string): Promise<void> {
  await composer(page).fill(text);
  await composer(page).press("Enter");
}

/** The summary of a finished turn's line: a button that opens to its steps. */
function summary(scope: Locator, text: string | RegExp): Locator {
  return scope.getByRole("button", { name: text });
}

async function openAtlas(page: Page): Promise<void> {
  await page.goto("/agent/atlas");
  await expect(conversation(page).getByText(GREETING)).toBeVisible();
}

test.describe("a live turn", () => {
  test.beforeEach(async ({ mock }) => {
    // Steps a few hundred milliseconds apart, so the line is seen while it runs.
    await mock.post("/api/mock/delays", { data: { scale: 6 } });
  });

  test("shows its steps as they run, then collapses to its summary", async ({ page }) => {
    await openAtlas(page);
    await send(page, "fail: check the wiki pages");
    const feed = conversation(page);

    await expect(feed.getByText("Working")).toBeVisible();
    await expect(page.getByRole("button", { name: "Stop the reply" })).toBeVisible();
    await expect(
      feed.getByRole("button", { name: /^Search(ing|ed) memory for “fail: check/ }),
    ).toBeVisible();
    await expect(
      feed.getByRole("button", { name: /^Read(ing)? team\/wiki\/index\.md/ }),
    ).toBeVisible();
    await expect(feed.getByText("Looking through recent notes first.")).toBeVisible();
    await expectNoAxeViolations(page);

    const line = summary(feed, /^Searched memory, read 2 files · \d+s · 1 step failed$/);
    await expect(line).toBeVisible({ timeout: 20_000 });
    await expect(line).toHaveAttribute("aria-expanded", "false");
    await expect(feed.getByText("Working")).toHaveCount(0);
    await expect(feed.getByRole("button", { name: "Read team/wiki/index.md" })).toHaveCount(0);

    await line.click();
    await expect(
      feed.getByRole("button", { name: "Read team/wiki/projects/residuum.md, failed" }),
    ).toBeVisible();
  });

  test("Stop mid-turn ends it, and its line says so", async ({ page }) => {
    await openAtlas(page);
    await send(page, "Tidy the wiki index");
    const feed = conversation(page);
    await expect(
      feed.getByRole("button", { name: /^Reading team\/wiki\/index\.md/ }),
    ).toBeVisible();

    await page.getByRole("button", { name: "Stop the reply" }).click();
    const line = summary(feed, /^Searched memory, read 2 files · \d+s · stopped by you$/);
    await expect(line).toBeVisible();
    await line.click();
    await expect(feed.getByRole("button", { name: /, stopped$/ }).first()).toBeVisible();
    await expect(feed.getByText("I've looked into that and here's what I found:")).toHaveCount(0);
  });

  test("Esc in the composer stops it, and closes an open menu first", async ({ page }) => {
    await openAtlas(page);
    await send(page, "Tidy the wiki index");
    const feed = conversation(page);
    await expect(feed.getByText("Working")).toBeVisible();

    await page.getByRole("button", { name: "More for atlas" }).click();
    await expect(page.getByRole("menu", { name: "More for atlas" })).toBeVisible();
    await composer(page).focus();
    await page.keyboard.press("Escape");
    await expect(page.getByRole("menu", { name: "More for atlas" })).toHaveCount(0);
    await expect(feed.getByText("Working")).toBeVisible();

    await composer(page).focus();
    await page.keyboard.press("Escape");
    await expect(summary(feed, /stopped by you/i)).toBeVisible();
  });
});

test("a turn from history opens to its steps, and a step to its details", async ({ page }) => {
  await openAtlas(page);
  const feed = conversation(page);
  const line = summary(feed, "Ran 1 command");
  await expect(line).toHaveAttribute("aria-expanded", "false");

  await line.click();
  const step = feed.getByRole("button", { name: "Ran residuum memory stats" });
  await expect(step).toBeVisible();
  await step.click();
  await expect(step).toHaveAttribute("aria-expanded", "true");
  const details = page.locator(`#${(await step.getAttribute("aria-controls")) ?? ""}`);
  await expect(details).toContainText("$ residuum memory stats");
  await expect(details).toContainText("Context window: 12,847 / 200,000 tokens (6.4%)");
  await expectNoAxeViolations(page);

  await line.click();
  await expect(step).toHaveCount(0);
});

test("a path a step read opens in the panel", async ({ page }) => {
  await openAtlas(page);
  await send(page, "Read the index");
  const feed = conversation(page);
  await summary(feed, /^Searched memory, read 2 files/).click();
  await feed.getByRole("link", { name: "team/wiki/index.md" }).click();
  await expect(page).toHaveURL(/\/agent\/atlas\?panel=file:team\/wiki\/index\.md$/);
});

test.describe("connecting while a turn runs", () => {
  test("a page opened mid-turn shows the steps it saw, and says earlier ones aren't shown", async ({
    page,
    mock,
  }) => {
    // The reads finish seconds apart, so the second page joins before the last steps.
    await mock.post("/api/mock/delays", { data: { scale: 8 } });
    await openAtlas(page);
    await send(page, "Check the routing doc");
    await expect(
      conversation(page).getByRole("button", { name: /^Reading team\/wiki\/index\.md/ }),
    ).toBeVisible({ timeout: 15_000 });

    const other = await page.context().newPage();
    await other.goto("/agent/atlas");
    const feed = conversation(other);
    await expect(feed.getByText("Earlier steps happened before this page connected")).toBeVisible({
      timeout: 15_000,
    });
    await expect(feed.getByText("Working")).toBeVisible();
    await expectNoAxeViolations(other);

    // It saw no step start, so its line says only that the turn worked before it connected.
    await expect(summary(feed, /^Worked before this page connected/)).toBeVisible({
      timeout: 20_000,
    });
    await other.close();
  });

  test("a turn still running after the connection drops notes the steps it may have missed", async ({
    page,
    mock,
  }) => {
    // Losing and regaining the connection takes real time.
    await mock.post("/api/mock/delays", { data: { scale: 1 } });
    await openAtlas(page);
    await send(page, "drop: keep going");
    const feed = conversation(page);

    await expect(
      feed.getByText("Steps taken while this page was reconnecting may be missing"),
    ).toBeVisible({ timeout: 15_000 });
    await expect(summary(feed, /^Searched memory, read 2 files · \d+s$/)).toBeVisible({
      timeout: 15_000,
    });
  });
});

test("a session's transcript shows its live line too", async ({ page, isMobile, mock }) => {
  await mock.post("/api/mock/delays", { data: { scale: 3 } });
  await page.goto(`/agent/atlas/activity?panel=session:atlas:run-live-research`);
  const panel = page.getByRole(isMobile ? "dialog" : "complementary", { name: RESEARCH });
  await expect(panel.getByText("Starting with what's already in the wiki.")).toBeVisible({
    timeout: 15_000,
  });

  const box = panel.getByRole("textbox", { name: "Message this session" });
  await box.fill("Weigh safety over speed");
  await box.press("Enter");
  await expect(
    panel.getByRole("button", { name: /^Search(ing|ed) memory for “fallback”/ }),
  ).toBeVisible({
    timeout: 15_000,
  });
  if (isMobile) await expectNoAxeViolations(page, { within: "[data-overlay-host]" });
  else await expectNoAxeViolations(page);

  await expect(summary(panel, /^Searched memory · \d+s$/)).toBeVisible({ timeout: 15_000 });
  await expect(panel.getByText('Understood: "Weigh safety over speed".')).toBeVisible();
});
