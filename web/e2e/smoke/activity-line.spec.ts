import type { Locator, Page } from "@playwright/test";
import { expectNoAxeViolations } from "../support/axe";
import { sendFromComposer } from "../support/composer";
import { expect, test } from "../support/fixtures";

/**
 * The activity lines: a live turn's steps, head with its timer and Stop, Esc,
 * a line collapsing to its summary once the agent has said more, a turn from
 * history opened to a step's details, joining a turn already running, and a
 * session's transcript.
 */

const GREETING = "Hi, this is atlas. You are in my conversation, not scout's.";
const RESEARCH = "Compare fallback strategies for notification delivery";

function conversation(page: Page): Locator {
  return page.getByRole("region", { name: "Conversation with atlas" });
}

function composer(page: Page): Locator {
  return page.getByRole("combobox", { name: "Message atlas" });
}

async function send(page: Page, text: string): Promise<void> {
  await composer(page).fill(text);
  await sendFromComposer(composer(page));
}

/** The summary of a run of steps that is over: a button that opens to its steps. */
function summary(scope: Locator, text: string | RegExp): Locator {
  return scope.getByRole("button", { name: text, exact: true });
}

/** How many pages have atlas's chat socket open, by the mock's count. */
async function connectedPages(page: Page): Promise<number> {
  const answer = (await (
    await page.request.get("/api/mock/connected-pages?agent=atlas")
  ).json()) as {
    pages: number;
  };
  return answer.pages;
}

async function openAtlas(page: Page): Promise<void> {
  await page.goto("/agent/atlas");
  await expect(conversation(page).getByText(GREETING)).toBeVisible();
}

test.describe("a live turn", () => {
  test("shows its steps as they run, then collapses to its summary", async ({ page, mock }) => {
    // The turn stays running until the checks of its live line are done.
    await mock.post("/api/mock/turn-hold", { data: { held: true } });
    await openAtlas(page);
    await mock.manualTime();
    await send(page, "fail: check the wiki pages");
    const feed = conversation(page);

    await expect(feed.getByText("Working")).toBeVisible();
    await expect(page.getByRole("button", { name: "Stop the reply" })).toBeVisible();
    // The search is sent at 300ms, the two reads at 600ms and their results at 900ms and 1.2s.
    await mock.advance(1_300);
    await expect(
      feed.getByRole("button", { name: /^Search(ing|ed) memory for “fail: check/ }),
    ).toBeVisible();
    await expect(
      feed.getByRole("button", { name: /^Read(ing)? team\/wiki\/index\.md/ }),
    ).toBeVisible();
    await expect(feed.getByText("Looking through recent notes first.")).toBeVisible();
    await expectNoAxeViolations(page);

    await mock.post("/api/mock/turn-hold", { data: { held: false } });
    await mock.advance(60_000);
    // The page read every step's start and end in the move to 1.3s, so the
    // steps took no time it can count, and the line names none.
    const line = summary(feed, /^Searched memory, read 2 files · 1 step failed$/);
    await expect(line).toBeVisible();
    await expect(line).toHaveAttribute("aria-expanded", "false");
    await expect(feed.getByText("Working")).toHaveCount(0);
    // The turn did work worth timing, so its close says how long: from 0 to
    // the move that reached 61.3s.
    await expect(feed.getByText("Worked for 1m 01s", { exact: true })).toBeVisible();
    await expect(feed.getByRole("button", { name: "Read team/wiki/index.md" })).toHaveCount(0);

    await line.click();
    await expect(
      feed.getByRole("button", { name: "Read team/wiki/projects/residuum.md, failed" }),
    ).toBeVisible();
  });

  test("Stop mid-turn ends it, and its line says so", async ({ page, mock }) => {
    await openAtlas(page);
    await mock.manualTime();
    await send(page, "Tidy the wiki index");
    const feed = conversation(page);
    await expect(feed.getByText("Working")).toBeVisible();
    // The reads are sent at 600ms and finish at 900ms and 1.2s, so stop them while the first runs.
    await mock.advance(700);
    await expect(
      feed.getByRole("button", { name: /^Reading team\/wiki\/index\.md/ }),
    ).toBeVisible();

    await page.getByRole("button", { name: "Stop the reply" }).click();
    const line = summary(feed, /^Searched memory, read 2 files/);
    await expect(line).toBeVisible();
    await expect(feed.getByText(/^Stopped by you/)).toBeVisible();
    await line.click();
    await expect(feed.getByRole("button", { name: /, stopped$/ }).first()).toBeVisible();
    await expect(feed.getByText("I've looked into that and here's what I found:")).toHaveCount(0);
  });

  test("Esc in the composer closes an open menu first, then stops it on a second press", async ({
    page,
    mock,
  }) => {
    await openAtlas(page);
    await mock.manualTime();
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
    await expect(
      page.getByRole("status").filter({ hasText: "Press Esc again to stop" }),
    ).toBeVisible();
    await expect(feed.getByText("Working")).toBeVisible();

    await page.keyboard.press("Escape");
    await expect(feed.getByText(/^Stopped by you/)).toBeVisible();
    await expect(page.getByText("Press Esc again to stop")).toHaveCount(0);
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
  test("a page opened mid-turn shows the steps taken before it connected", async ({
    page,
    mock,
  }) => {
    // The turn sits with its two file reads running, sending nothing more, so the
    // second page can finish connecting however slowly it loads.
    await mock.post("/api/mock/turn-hold", { data: { held: "steps" } });
    await openAtlas(page);
    await mock.manualTime();
    await send(page, "Check the routing doc");
    await expect(conversation(page).getByText("Working")).toBeVisible();
    // The reads are sent at 600ms; the hold keeps them running.
    await mock.advance(700);
    await expect(
      conversation(page).getByRole("button", { name: /^Reading team\/wiki\/index\.md/ }),
    ).toBeVisible();

    const other = await page.context().newPage();
    await other.goto("/agent/atlas");
    const feed = conversation(other);
    await expect(feed.getByText(GREETING)).toBeVisible();
    await expect.poll(() => connectedPages(page)).toBe(2);

    // The agent sent it the turn so far: the message, the note and the reads still running.
    await expect(feed.getByText("Check the routing doc", { exact: true })).toBeVisible();
    await expect(feed.getByText("Looking through recent notes first.")).toBeVisible();
    await expect(
      feed.getByRole("button", { name: /^Reading team\/wiki\/index\.md/ }),
    ).toBeVisible();
    await expect(feed.getByText("Working")).toBeVisible();
    await expect(feed.getByText(/before this page connected/)).toHaveCount(0);
    await expectNoAxeViolations(other);
    await mock.post("/api/mock/turn-hold", { data: { held: false } });
    await mock.advance(60_000);

    // The steps' starts come with the turn so far, at the mock's times (from
    // 300ms); their ends are read in the move that reached 60.7s.
    await expect(summary(feed, "Searched memory, read 2 files · 1m 00s")).toBeVisible();
    await other.close();
  });

  test("a turn still running after the connection drops shows the steps taken while it was down", async ({
    page,
    mock,
  }) => {
    await openAtlas(page);
    await mock.manualTime();
    await send(page, "drop: keep going");
    const feed = conversation(page);
    await expect(feed.getByText("Working")).toBeVisible();

    // The connection drops at 600ms, and the reads finish at 900ms and 1.2s while it is down.
    // The page reconnects about a second of real time later, and asks for the turn in flight.
    await mock.advance(1_300);
    await expect(
      feed.getByRole("button", { name: /^Read(ing)? team\/wiki\/index\.md/ }),
    ).toBeVisible();

    // Frames are live again from 3.5s, so the turn's end at 4s reaches the page as it happens.
    await mock.advance(60_000);
    await expect(summary(feed, "Searched memory, read 2 files")).toBeVisible();
    await expect(feed.getByText(/may be missing/)).toHaveCount(0);
  });
});

test("a session's transcript shows its live line too", async ({ page, isMobile, mock }) => {
  await page.goto(`/agent/atlas/activity?panel=session:atlas:run-live-research`);
  const panel = page.getByRole(isMobile ? "dialog" : "complementary", { name: RESEARCH });
  await expect(panel.getByText("Starting with what's already in the wiki.")).toBeVisible();

  await mock.manualTime();
  const box = panel.getByRole("textbox", { name: "Message this session" });
  await box.fill("Weigh safety over speed");
  await box.press("Enter");
  // The turn's start shows once the mock has set its timers, so the advances below reach them.
  await expect(panel.getByText("Working", { exact: true })).toBeVisible();
  // The search is sent at 500ms.
  await mock.advance(500);
  await expect(
    panel.getByRole("button", { name: /^Search(ing|ed) memory for “fallback”/ }),
  ).toBeVisible();
  if (isMobile) await expectNoAxeViolations(page, { within: "[data-overlay-host]" });
  else await expectNoAxeViolations(page);

  // The result comes at 1.2s, and the reply ends the turn at 2.4s.
  await mock.advance(2_000);
  // The turn's work before the message and after it are separate lines; the one after it is last.
  await expect(summary(panel, "Searched memory").last()).toBeVisible();
  await expect(panel.getByText('Understood: "Weigh safety over speed".')).toBeVisible();
});
