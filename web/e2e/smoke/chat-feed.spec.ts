import type { Locator, Page } from "@playwright/test";
import { expectNoAxeViolations } from "../support/axe";
import { expect, test } from "../support/fixtures";

/**
 * The chat feed: older history loading near the top, Jump to latest, catching
 * up after the connection drops, path links into the context panel, and the
 * header. The composer and the running-turn line are legacy views until their
 * units rebuild them, so the scans leave them out.
 */

const LEGACY = "[data-legacy-view]";
const GREETING = "Hi, this is atlas. You are in my conversation, not scout's.";
const FIRST_REPLY = "I've looked into that and here's what I found:";
const FALLBACKS = "team/wiki/notification-fallbacks.md";

function conversation(page: Page, agent = "atlas"): Locator {
  return page.getByRole("region", { name: `Conversation with ${agent}` });
}

function composer(page: Page): Locator {
  return page.getByRole("textbox", { name: "Send a message..." });
}

async function send(page: Page, text: string): Promise<void> {
  await composer(page).fill(text);
  await composer(page).press("Enter");
}

async function scrollToTop(feed: Locator): Promise<void> {
  await feed.evaluate((el) => {
    el.scrollTop = 0;
  });
}

test("the conversation reads as replies, bubbles and cards", async ({ page }) => {
  await page.goto("/agent/atlas");
  const feed = conversation(page);
  await expect(feed.getByText(GREETING)).toBeVisible();

  await expect(
    feed.getByText("Good. Let's keep iterating on the notification routing doc."),
  ).toBeVisible();
  // A header the owner pasted stays their own message, not a card.
  await expect(feed.getByRole("article")).toHaveCount(2);
  // A background turn that didn't start with an agent's message stays out of the conversation.
  await expect(feed.getByText("Pulse check: inbox_check.")).toHaveCount(0);
  await expect(feed.getByText("HEARTBEAT_OK")).toHaveCount(0);
  await expect(
    feed.getByRole("article", { name: "Background session: spawned-research-3f9a" }),
  ).toContainText("Found three fallback strategies worth comparing");
  await expectNoAxeViolations(page, { exclude: LEGACY });
});

test("older episodes load as the reader nears the top, and what they read stays put", async ({
  page,
}) => {
  await page.goto("/agent/atlas");
  const feed = conversation(page);
  await expect(feed.getByText(GREETING)).toBeInViewport();

  // The newest episode comes with recent history, so the marker shows from the start.
  const newest = feed.getByRole("separator", { name: /^ep-003 · / });
  await expect(newest).toBeAttached();
  await expect(feed.getByRole("note")).toHaveText(
    "Older messages are summarized. atlas remembers what was said, not the exact wording.",
  );
  await expect(feed.getByRole("separator", { name: /^ep-002 · / })).toHaveCount(0);

  await scrollToTop(feed);
  await expect(feed.getByRole("separator", { name: /^ep-002 · / })).toBeAttached();
  // The older part went in above: the divider the reader was at is still in view.
  await expect(newest).toBeInViewport();

  await expect(async () => {
    await scrollToTop(feed);
    await expect(feed.getByRole("separator", { name: /^ep-001 · / })).toBeInViewport({
      timeout: 1000,
    });
  }).toPass();
  await expect(feed.getByText("Loading earlier messages…")).toHaveCount(0);
});

test("Jump to latest names where the reader is, and takes them back", async ({ page }) => {
  await page.goto("/agent/atlas");
  const feed = conversation(page);
  const greeting = feed.getByText(GREETING);
  await expect(greeting).toBeInViewport();
  const jump = page.getByRole("button", { name: "Jump to latest" });
  await expect(jump).toHaveCount(0);

  await scrollToTop(feed);
  await expect(jump).toBeVisible();
  await expect(jump).toHaveAccessibleDescription(/^ep-00\d · \d{4}-\d{2}-\d{2}$/);
  await expectNoAxeViolations(page, { exclude: LEGACY });

  await jump.click();
  await expect(greeting).toBeInViewport();
  await expect(jump).toHaveCount(0);

  // Sending from further up brings the reader down to their own message.
  await scrollToTop(feed);
  await expect(jump).toBeVisible();
  await send(page, "Back to the routing doc.");
  await expect(feed.getByText("Back to the routing doc.")).toBeInViewport();
});

test.describe("after the connection drops", () => {
  test.beforeEach(async ({ mock }) => {
    // Losing and regaining the connection takes real time.
    await mock.post("/api/mock/delays", { data: { scale: 1 } });
  });

  test("a turn that ended while it was down merges in once", async ({ page }) => {
    await page.goto("/agent/atlas");
    const feed = conversation(page);
    await expect(feed.getByText(GREETING)).toBeVisible();

    await send(page, "drop finish: check the routing doc");

    await expect(feed.getByText(FIRST_REPLY)).toBeVisible({ timeout: 15_000 });
    await expect(feed.getByText("drop finish: check the routing doc")).toHaveCount(1);
    await expect(feed.getByText(FIRST_REPLY)).toHaveCount(1);
  });

  test("a turn still running when it's back finishes live", async ({ page }) => {
    await page.goto("/agent/atlas");
    const feed = conversation(page);
    await expect(feed.getByText(GREETING)).toBeVisible();

    await send(page, "drop: keep going");

    await expect(feed.getByText(FIRST_REPLY)).toBeVisible({ timeout: 15_000 });
    await expect(feed.getByText("drop: keep going")).toHaveCount(1);
    await expect(feed.getByText(FIRST_REPLY)).toHaveCount(1);
  });

  test("a session's result sent while it was down shows as its card", async ({ page, mock }) => {
    await page.goto("/agent/atlas");
    const feed = conversation(page);
    await expect(feed.getByText(GREETING)).toBeVisible();

    await mock.post("/api/mock/missed-relay", { params: { agent: "atlas" } });

    await expect(
      feed
        .getByRole("article", { name: "Background session: spawned-research-3f9a" })
        .filter({ hasText: "Missed while you were away" }),
    ).toBeVisible({ timeout: 15_000 });
    await expect(
      feed.getByText("The research session finished the fallback doc while you were disconnected."),
    ).toBeVisible();
  });

  test("a reload under a reader who scrolled up keeps their place", async ({ page }) => {
    await page.goto("/agent/atlas");
    const feed = conversation(page);
    await expect(feed.getByText(GREETING)).toBeVisible();

    // The summarizer folds the conversation into a new episode while the
    // connection is down, so the page reloads its history when it's back.
    await send(page, "drop compress: tidy the wiki");
    const reading = feed.getByText("Did the observer flag anything odd in last night's batch?");
    await reading.evaluate((el) => {
      el.scrollIntoView({ block: "start" });
    });
    await expect(page.getByRole("button", { name: "Jump to latest" })).toBeVisible();

    await expect(feed.getByRole("separator", { name: /^ep-004 · / })).toBeAttached({
      timeout: 15_000,
    });
    await expect(reading).toBeInViewport();
    await expect(feed.getByText("drop compress: tidy the wiki")).toHaveCount(1);
  });
});

test.describe("path links", () => {
  test("a workspace path in a message opens that file in the panel", async ({ page, mock }) => {
    await mock.post("/api/mock/team-file", {
      data: { path: FALLBACKS, content: "# Fallbacks\n\nCascade first, then park.\n" },
    });
    await page.goto("/agent/atlas");
    await conversation(page).getByRole("link", { name: FALLBACKS }).click();

    await expect(page).toHaveURL(
      /\/agent\/atlas\?panel=file:team\/wiki\/notification-fallbacks\.md$/,
    );
    await expect(
      page.getByRole("textbox", { name: "Contents of notification-fallbacks.md" }),
    ).toHaveValue(/# Fallbacks/);

    await page.goBack();
    await expect(page).toHaveURL(/\/agent\/atlas$/);
    await expect(
      page.getByRole("textbox", { name: "Contents of notification-fallbacks.md" }),
    ).toHaveCount(0);
  });

  test("a path that doesn't exist says so, and offers nothing to edit", async ({ page }) => {
    await page.goto("/agent/atlas");
    await conversation(page).getByRole("link", { name: FALLBACKS }).click();

    await expect(page.getByRole("heading", { name: "notification-fallbacks.md" })).toBeVisible();
    await expect(page.getByRole("heading", { name: "This file doesn't exist" })).toBeVisible();
    await expect(page.getByText(`Nothing is saved at ${FALLBACKS}.`)).toBeVisible();
    await expect(
      page.getByRole("textbox", { name: "Contents of notification-fallbacks.md" }),
    ).toHaveCount(0);
  });
});

test("Open session is offered for a session's message, not a teammate's", async ({
  page,
  mock,
}) => {
  await mock.post("/api/mock/teammate-message", { params: { agent: "atlas" } });
  await page.goto("/agent/atlas");
  const feed = conversation(page);

  const teammate = feed.getByRole("article", { name: "Teammate: scout" });
  await expect(teammate).toContainText("Can you look over the wiki index");
  await expect(teammate.getByRole("button", { name: "Open session" })).toHaveCount(0);
  await expectNoAxeViolations(page, { exclude: LEGACY });

  await feed
    .getByRole("article", { name: "Background session: spawned-research-3f9a" })
    .getByRole("button", { name: "Open session" })
    .click();
  await expect(page).toHaveURL(/panel=session:atlas:/);
});

test("a code block's Copy button copies it", async ({ page, context, browserName }) => {
  test.skip(browserName === "webkit", "WebKit grants no clipboard permission to a test.");
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await page.goto("/agent/atlas");
  const feed = conversation(page);
  await expect(feed.getByText(GREETING)).toBeVisible();
  await send(page, "Where do the memory thresholds live?");
  await expect(feed.getByText(FIRST_REPLY)).toBeVisible();

  await feed.getByRole("button", { name: "Copy" }).click();
  await expect(feed.getByRole("button", { name: "Copied" })).toBeVisible();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(
    "[memory]\nobserver_threshold_tokens = 30000\nreflector_threshold_tokens = 40000",
  );
});

test.describe("the header", () => {
  test("shows the agent, its running sessions and its menu", async ({ page }) => {
    await page.goto("/agent/atlas");
    await expect(page.getByRole("heading", { level: 1, name: "atlas" })).toBeVisible();
    await expect(page.getByText("Keeps the team wiki tidy")).toBeVisible();

    await page.getByRole("button", { name: "More for atlas" }).click();
    const menu = page.getByRole("menu", { name: "More for atlas" });
    await expect(menu.getByRole("menuitem", { name: /^Restart atlas/ })).toBeVisible();
    await expect(menu.getByRole("menuitem", { name: /^Stop atlas/ })).toBeVisible();
    await expectNoAxeViolations(page, { exclude: LEGACY });

    await menu.getByRole("menuitem", { name: /^Show conversation size/ }).click();
    await expect(page).toHaveURL(/\/agent\/atlas\?panel=size$/);
  });

  test("its running pill opens Activity", async ({ page }) => {
    await page.goto("/agent/atlas");
    await page.getByRole("button", { name: /^\d+ running, open Activity$/ }).click();
    await expect(page).toHaveURL(/\/agent\/atlas\/activity$/);
  });
});
