import type { Locator, Page } from "@playwright/test";
import { expectNoAxeViolations } from "../support/axe";
import { expect, test } from "../support/fixtures";
import { FIRST_RECONNECT_MS, reconnectingLine, sendFromComposer } from "../support/composer";
import { expectFileRendered } from "../support/lazy";

/**
 * The chat feed: older history loading near the top, Jump to latest, catching
 * up after the connection drops, path links into the context panel, and the
 * header.
 */

const GREETING = "Hi, this is atlas. You are in my conversation, not scout's.";
const FIRST_REPLY = "I've looked into that and here's what I found:";
const FALLBACKS = "team/wiki/notification-fallbacks.md";

function conversation(page: Page, agent = "atlas"): Locator {
  return page.getByRole("region", { name: `Conversation with ${agent}` });
}

function composer(page: Page): Locator {
  return page.getByRole("combobox", { name: "Message atlas" });
}

async function send(page: Page, text: string): Promise<void> {
  await composer(page).fill(text);
  await sendFromComposer(composer(page));
}

/** How many pages have atlas's chat socket open, by the mock's count. */
async function connectedPages(page: Page): Promise<number> {
  const answer = (await (
    await page.request.get("/api/mock/connected-pages?agent=atlas")
  ).json()) as { pages: number };
  return answer.pages;
}

/** How far the feed's scroll position is above the bottom of its range. */
async function gapToBottom(feed: Locator): Promise<number> {
  return feed.evaluate((el) => el.scrollHeight - el.scrollTop - el.clientHeight);
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
  await expectNoAxeViolations(page);
});

test("older episodes load as the reader nears the top, and what they read stays put", async ({
  page,
}) => {
  await page.goto("/agent/atlas");
  const feed = conversation(page);
  await expect(feed.getByText(GREETING)).toBeInViewport();

  // The newest episode comes with recent history, so the marker shows from the start.
  const newest = feed.getByRole("separator", { name: /, ep-003$/ });
  await expect(newest).toBeAttached();
  await expect(feed.getByRole("note")).toHaveText(
    "Older messages are summarized. atlas remembers what was said, not the exact wording.",
  );
  await expect(feed.getByRole("separator", { name: /, ep-002$/ })).toHaveCount(0);

  await scrollToTop(feed);
  await expect(feed.getByRole("separator", { name: /, ep-002$/ })).toBeAttached();
  // The older part went in above: the divider the reader was at is still in view.
  await expect(newest).toBeInViewport();

  await expect(async () => {
    await scrollToTop(feed);
    await expect(feed.getByRole("separator", { name: /, ep-001$/ })).toBeInViewport();
  }).toPass();
  await expect(feed.getByText("Loading earlier messages…")).toHaveCount(0);
});

test("Jump to latest sits above the composer, says when a reply landed below, and takes the reader back", async ({
  page,
  mock,
}) => {
  await page.goto("/agent/atlas");
  const feed = conversation(page);
  const greeting = feed.getByText(GREETING);
  await expect(greeting).toBeInViewport();
  const jump = page.getByRole("button", { name: "Jump to latest" });
  await expect(jump).toHaveCount(0);

  await scrollToTop(feed);
  await expect(jump).toBeVisible();
  // Bottom centre of the conversation, just above the composer.
  const pill = await jump.boundingBox();
  const area = await feed.boundingBox();
  const field = await page.locator("form.composer").boundingBox();
  if (!pill || !area || !field)
    throw new Error("the pill, conversation or composer isn't laid out");
  expect(pill.y + pill.height).toBeLessThanOrEqual(field.y);
  expect(field.y - (pill.y + pill.height)).toBeLessThan(40);
  expect(Math.abs(pill.x + pill.width / 2 - (area.x + area.width / 2))).toBeLessThan(20);
  await expect(jump).not.toHaveAccessibleDescription(/ep-00/);
  await expectNoAxeViolations(page);

  // A reply lands below them: the pill says so.
  await mock.post("/api/mock/teammate-message?agent=atlas");
  const fresh = page.getByRole("button", { name: "New reply, jump to latest" });
  await expect(fresh).toBeVisible();
  await expect(fresh).toHaveText("New reply");
  await expect(greeting).not.toBeInViewport();
  await expectNoAxeViolations(page);

  await fresh.click();
  await expect(feed.getByText("scout asked me to check the wiki index. On it.")).toBeInViewport();
  await expect(fresh).toHaveCount(0);
  await expect(jump).toHaveCount(0);

  // Sending from further up brings the reader down to their own message. The reply is held
  // short of its end: a whole one is longer than a phone's view, and following it moves the
  // message up out of sight.
  await scrollToTop(feed);
  await expect(jump).toBeVisible();
  await mock.post("/api/mock/turn-hold", { data: { held: true } });
  await send(page, "Back to the routing doc.");
  await expect(feed.getByText("Back to the routing doc.", { exact: true })).toBeInViewport();
});

test("scrolling back up while the Jump to latest glide is under way brings the pill back", async ({
  mock,
  page,
}) => {
  await page.goto("/agent/atlas");
  const feed = conversation(page);
  await expect(feed.getByText(GREETING)).toBeInViewport();
  await scrollToTop(feed);
  const jump = page.getByRole("button", { name: "Jump to latest" });
  await expect(jump).toBeVisible();

  // Press the pill, and once the glide has started, move back to the top with no wheel,
  // touch, key or pointer event in the feed, as find in page or a screen reader does.
  await feed.evaluate(
    (el) =>
      new Promise<void>((resolve) => {
        const onScroll = (): void => {
          if (el.scrollTop <= 0) return;
          el.removeEventListener("scroll", onScroll);
          el.scrollTop = 0;
          resolve();
        };
        el.addEventListener("scroll", onScroll);
        document.querySelector<HTMLButtonElement>(".feed-pill-jump")?.click();
      }),
  );

  await expect(jump).toBeVisible();
  // A reply landing now leaves the reader where they went.
  await mock.post("/api/mock/teammate-message?agent=atlas");
  await expect(page.getByRole("button", { name: "New reply, jump to latest" })).toBeVisible();
  await expect(feed.getByText(GREETING)).not.toBeInViewport();
});

test("a reader who starts scrolling up while a reply streams isn't pulled back down", async ({
  browserName,
  isMobile,
  mock,
  page,
}) => {
  test.skip(
    isMobile && browserName !== "chromium",
    "A touch drag is sent over CDP, which only Chromium has.",
  );
  // 100px is well inside the distance a reader coming back down counts as at the end.
  const distance = 100;
  await page.goto("/agent/atlas");
  const feed = conversation(page);
  await expect(feed.getByText(GREETING)).toBeVisible();
  await expect.poll(() => gapToBottom(feed)).toBeLessThan(2);
  await mock.manualTime();
  await send(page, "markdown please");
  await expect(feed.getByText("Working")).toBeVisible();
  // The reply has started streaming, and the reader is following it.
  const before = await feed.evaluate((el) => el.scrollHeight);
  await mock.advance(1_000);
  await expect.poll(() => feed.evaluate((el) => el.scrollHeight)).toBeGreaterThan(before + 100);
  await expect.poll(() => gapToBottom(feed)).toBeLessThan(2);

  // A short drag up, a finger on a phone and the wheel on a desktop, with a piece of the
  // reply landing between each move and the next.
  const area = await feed.boundingBox();
  if (!area) throw new Error("the conversation isn't laid out");
  const x = Math.round(area.x + area.width / 2);
  const y = Math.round(area.y + area.height / 3);
  const steps = 25;
  if (isMobile) {
    const cdp = await page.context().newCDPSession(page);
    await cdp.send("Input.dispatchTouchEvent", { type: "touchStart", touchPoints: [{ x, y }] });
    for (let step = 1; step <= steps; step++) {
      const touchPoints = [{ x, y: y + (step * distance) / steps }];
      await cdp.send("Input.dispatchTouchEvent", { type: "touchMove", touchPoints });
      await mock.advance(45);
    }
    await cdp.send("Input.dispatchTouchEvent", { type: "touchEnd", touchPoints: [] });
  } else {
    await page.mouse.move(x, y);
    for (let step = 1; step <= steps; step++) {
      await page.mouse.wheel(0, -distance / steps);
      await mock.advance(45);
    }
  }
  for (let piece = 0; piece < 10; piece++) await mock.advance(45);
  expect(await gapToBottom(feed)).toBeGreaterThan(distance / 2);

  // The rest of the reply lands below them, and the pill says so.
  await mock.advance(60_000);
  await expect(feed.getByText("Working")).toHaveCount(0);
  await expect(page.getByRole("button", { name: /Jump to latest|New reply/ })).toBeVisible();
  expect(await gapToBottom(feed)).toBeGreaterThan(distance / 2);
});

test("the keyboard scrolls a conversation that overflows, and follows it again at the end", async ({
  page,
}) => {
  await page.goto("/agent/atlas");
  const feed = conversation(page);
  const greeting = feed.getByText(GREETING);
  await expect(greeting).toBeInViewport();
  const jump = page.getByRole("button", { name: "Jump to latest" });

  // It scrolls, so it is a tab stop.
  await expect(feed).toHaveAttribute("tabindex", "0");
  await feed.focus();
  await expect(feed).toBeFocused();

  await page.keyboard.press("Home");
  await expect(greeting).not.toBeInViewport();
  await expect(jump).toBeVisible();

  await page.keyboard.press("End");
  await expect(greeting).toBeInViewport();
  await expect.poll(() => gapToBottom(feed)).toBeLessThan(2);
  await expect(jump).toHaveCount(0);
  await expect(feed).toBeFocused();
});

test("End pressed while an older episode loads still follows the conversation to its end", async ({
  page,
}) => {
  // Home brings the reader near the top, which asks for the older episode; its answer
  // waits until End has been pressed, so it lands while End takes the reader down.
  let releaseOlder: () => void = () => undefined;
  const released = new Promise<void>((resolve) => {
    releaseOlder = resolve;
  });
  const olderEpisode = /\/api\/agents\/atlas\/chat\/history\?episode=ep-002$/;
  await page.route(olderEpisode, async (route) => {
    await released;
    await route.continue();
  });
  await page.goto("/agent/atlas");
  const feed = conversation(page);
  const greeting = feed.getByText(GREETING);
  await expect(greeting).toBeInViewport();
  await feed.focus();

  const asked = page.waitForRequest(olderEpisode);
  await page.keyboard.press("Home");
  await asked;
  await page.keyboard.press("End");
  releaseOlder();

  await expect(feed.getByRole("separator", { name: /, ep-002$/ })).toBeAttached();
  await expect.poll(() => gapToBottom(feed)).toBeLessThan(2);
  await expect(greeting).toBeInViewport();
  await expect(page.getByRole("button", { name: "Jump to latest" })).toHaveCount(0);
});

test("a conversation with nothing to scroll is no tab stop", async ({ page }) => {
  await page.goto("/agent/drifter");
  const feed = conversation(page, "drifter");
  await expect(feed).toBeVisible();
  await expect(feed).not.toHaveAttribute("tabindex");
});

test.describe("after the connection drops", () => {
  test("a turn that ended while it was down merges in once", async ({ page, mock }) => {
    await page.goto("/agent/atlas");
    const feed = conversation(page);
    await expect(feed.getByText(GREETING)).toBeVisible();

    await mock.manualTime();
    await send(page, "drop finish: check the routing doc");
    // The connection drops at 600ms, and the turn ends at 900ms while the page is away.
    await expect(feed.getByText("Working")).toBeVisible();
    await mock.advance(1_000);
    // The page tries again a second after it saw the drop, and loads the turn from history.
    await expect(reconnectingLine(page)).toBeVisible();
    await mock.advance(FIRST_RECONNECT_MS);

    await expect(feed.getByText(FIRST_REPLY)).toBeVisible();
    await expect(feed.getByText("drop finish: check the routing doc")).toHaveCount(1);
    await expect(feed.getByText(FIRST_REPLY)).toHaveCount(1);
  });

  test("a turn still running when it's back finishes live", async ({ page, mock }) => {
    await page.goto("/agent/atlas");
    const feed = conversation(page);
    await expect(feed.getByText(GREETING)).toBeVisible();

    await mock.manualTime();
    await send(page, "drop: keep going");
    await expect(feed.getByText("Working")).toBeVisible();
    // The connection drops at 600ms. The page tries again a second after it saw the drop, while
    // the turn is still running: the mock's connection returns at 3.5s, and the turn ends at 4s.
    await mock.advance(1_000);
    await expect(reconnectingLine(page)).toBeVisible();
    await mock.advance(FIRST_RECONNECT_MS);
    await expect.poll(() => connectedPages(page)).toBe(1);
    await mock.advance(60_000);

    await expect(feed.getByText(FIRST_REPLY)).toBeVisible();
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
    ).toBeVisible();
    await expect(
      feed.getByText("The research session finished the fallback doc while you were disconnected."),
    ).toBeVisible();
  });

  test("a reload under a reader who scrolled up keeps their place", async ({ page, mock }) => {
    await page.goto("/agent/atlas");
    const feed = conversation(page);
    await expect(feed.getByText(GREETING)).toBeVisible();

    // The summarizer folds the conversation into a new episode while the
    // connection is down, so the page reloads its history when it's back.
    await mock.manualTime();
    await send(page, "drop compress: tidy the wiki");
    await expect(feed.getByText("Working")).toBeVisible();
    const reading = feed.getByText("Did the observer flag anything odd in last night's batch?");
    await reading.evaluate((el) => {
      el.scrollIntoView({ block: "start" });
    });
    await expect(page.getByRole("button", { name: "Jump to latest" })).toBeVisible();

    // The connection drops at 600ms, the history is compressed while it is down, and the page
    // tries again a second after it saw the drop, before the turn ends at 4s.
    await mock.advance(1_000);
    await expect(reconnectingLine(page)).toBeVisible();
    await mock.advance(FIRST_RECONNECT_MS);
    await expect.poll(() => connectedPages(page)).toBe(1);
    await mock.advance(60_000);

    await expect(feed.getByRole("separator", { name: /, ep-004$/ })).toBeAttached();
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
    await expectFileRendered(page, "Fallbacks");
    await expect(page.getByText("Cascade first, then park.")).toBeVisible();

    await page.goBack();
    await expect(page).toHaveURL(/\/agent\/atlas$/);
    await expect(page.getByRole("heading", { name: "Fallbacks", exact: true })).toHaveCount(0);
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
  await expectNoAxeViolations(page);

  await feed
    .getByRole("article", { name: "Background session: spawned-research-3f9a" })
    .getByRole("button", { name: "Open session" })
    .click();
  await expect(page).toHaveURL(/panel=session:atlas:/);
});

test("a session's message the running turn read shows on a page opened partway, and opens its run", async ({
  page,
  mock,
}) => {
  const note = "Interim note: the cascade option needs a retry cap.";
  // The turn waits at its last step, so the page opens again while it runs.
  await mock.post("/api/mock/turn-hold", { data: { held: true } });
  await page.goto("/agent/atlas");
  const feed = conversation(page);
  await expect(feed.getByText(GREETING)).toBeVisible();
  await send(page, "Check the fallback doc");
  await expect(feed.getByText("Working", { exact: true })).toBeVisible();

  const answer = await mock.post("/api/mock/session-message", {
    params: { agent: "atlas" },
    data: { content: note },
  });
  expect(answer).toEqual({ run_id: "run-live-research" });
  const card = feed
    .getByRole("article", { name: "Background session: spawned-research-3f9a" })
    .filter({ hasText: note });
  await expect(card).toHaveCount(1);

  // History holds the turn only once it ends, so the reopened page has the card from the turn so far.
  await page.reload();
  await expect(feed.getByText(GREETING)).toBeVisible();
  await expect(feed.getByText("Working", { exact: true })).toBeVisible();
  await expect(card).toHaveCount(1);
  await expect(card).not.toContainText("Agent Message from");

  await card.getByRole("button", { name: "Open session" }).click();
  await expect(page).toHaveURL(/panel=session:atlas:run-live-research$/);
  await mock.post("/api/mock/turn-hold", { data: { held: false } });
});

test("a teammate's message shows as it arrives, without a reload", async ({ page, mock }) => {
  await page.goto("/agent/atlas");
  const feed = conversation(page);
  await expect(feed.getByText(GREETING)).toBeVisible();
  await expect.poll(() => connectedPages(page)).toBe(1);

  await mock.post("/api/mock/teammate-message", { params: { agent: "atlas" } });

  const teammate = feed.getByRole("article", { name: "Teammate: scout" });
  await expect(teammate).toHaveCount(1);
  await expect(teammate).toContainText("Can you look over the wiki index");
  await expect(teammate).not.toContainText("Message from teammate");
  await expect(feed.getByText("scout asked me to check the wiki index. On it.")).toBeVisible();
});

test("a code block's Copy button copies it", async ({ page, context, browserName }) => {
  test.skip(browserName === "webkit", "WebKit grants no clipboard permission to a test.");
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await page.goto("/agent/atlas");
  const feed = conversation(page);
  await expect(feed.getByText(GREETING)).toBeVisible();
  await send(page, "Where do the memory thresholds live?");
  await expect(feed.getByText(FIRST_REPLY)).toBeVisible();

  await feed.getByRole("button", { name: "Copy code" }).click();
  await expect(feed.getByRole("button", { name: "Copied" })).toBeVisible();
  await expect(feed.getByRole("status").filter({ hasText: "Copied" })).toBeAttached();
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
    await expectNoAxeViolations(page);

    await menu.getByRole("menuitem", { name: /^Show conversation size/ }).click();
    await expect(page).toHaveURL(/\/agent\/atlas\?panel=size$/);
  });

  test("its running pill opens Activity", async ({ page }) => {
    await page.goto("/agent/atlas");
    await page.getByRole("button", { name: /^\d+ running, open Activity$/ }).click();
    await expect(page).toHaveURL(/\/agent\/atlas\/activity$/);
  });
});
