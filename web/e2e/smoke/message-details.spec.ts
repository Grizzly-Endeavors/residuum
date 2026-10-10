import type { Locator, Page } from "@playwright/test";
import { expectNoAxeViolations } from "../support/axe";
import { sendFromComposer } from "../support/composer";
import { expect, test } from "../support/fixtures";

/**
 * What a message keeps quiet until it is hovered, focused or tapped: when it
 * was sent.
 */

const GREETING = "Hi, this is atlas. You are in my conversation, not scout's.";

// The page's clock stands where the mock's does, so the sample conversation's messages from
// "today" are from today.
test.use({ frozenClock: true });

function conversation(page: Page): Locator {
  return page.getByRole("region", { name: "Conversation with atlas" });
}

/** The conversation's message that says `text`. */
function messageSaying(page: Page, text: string): Locator {
  return conversation(page).locator(".feed-message").filter({ hasText: text });
}

async function send(page: Page, text: string): Promise<void> {
  const box = page.getByRole("combobox", { name: "Message atlas" });
  await box.fill(text);
  await sendFromComposer(box);
}

test("a message's time is in the page but quiet until it is hovered, focused or tapped", async ({
  page,
  isMobile,
}) => {
  await page.goto("/agent/atlas");
  const message = messageSaying(page, GREETING);
  const time = message.locator("time");

  // Always there for assistive technology, and readable: a time of day for today.
  await expect(time).toBeAttached();
  await expect(time).toHaveAttribute("datetime", /^\d{4}-\d{2}-\d{2}T/);
  await expect(time).toHaveText(/^\d{1,2}:\d{2}/);
  await expect(time).toHaveCSS("opacity", "0");

  if (isMobile) {
    await message.getByText(GREETING).tap();
    await expect(time).toHaveCSS("opacity", "1");
    await message.getByText(GREETING).tap();
    await expect(time).toHaveCSS("opacity", "0");
  } else {
    await message.hover();
    await expect(time).toHaveCSS("opacity", "1");
    await page.mouse.move(0, 0);
    await expect(time).toHaveCSS("opacity", "0");
  }
});

test("keyboard focus inside a message shows its time", async ({ page }) => {
  await page.goto("/agent/atlas");
  const card = conversation(page).getByRole("article", {
    name: "Background session: spawned-research-3f9a",
  });
  const time = card.locator("time");
  await expect(time).toHaveCSS("opacity", "0");

  await page.keyboard.press("Tab");
  await card.getByRole("button", { name: "Open session" }).focus();
  await expect(time).toHaveCSS("opacity", "1");
  await expectNoAxeViolations(page);

  await card.getByRole("button", { name: "Open session" }).blur();
  await expect(time).toHaveCSS("opacity", "0");
});

test("a message from another day names the date as well", async ({ page }) => {
  await page.goto("/agent/atlas");
  // The sample conversation's earlier messages are from yesterday and the day before; the
  // archived episodes above them carry no time at all.
  const times = conversation(page).locator("time");
  await expect(times.first()).toBeAttached();
  const labels = await times.allTextContents();
  expect(labels.some((label) => /[A-Z][a-z]{2} \d{1,2}/.test(label))).toBe(true);
  expect(labels.some((label) => /^\d{1,2}:\d{2}/.test(label))).toBe(true);
});

test("Copy is quiet on a screen that hovers and in sight on one that doesn't, and reaches the keyboard", async ({
  page,
  isMobile,
}) => {
  await page.goto("/agent/atlas");
  const message = messageSaying(page, GREETING);
  const copy = message.getByRole("button", { name: "Copy reply" });
  await expect(copy).toBeAttached();

  if (isMobile) {
    // No hover to reveal it, so it stays where a thumb can find it.
    await expect(copy).toBeVisible();
    await expect(copy.locator("xpath=..")).toHaveCSS("opacity", "1");
  } else {
    await expect(copy.locator("xpath=..")).toHaveCSS("opacity", "0");
    await message.hover();
    await expect(copy.locator("xpath=..")).toHaveCSS("opacity", "1");
    await page.mouse.move(0, 0);
    await expect(copy.locator("xpath=..")).toHaveCSS("opacity", "0");
    // The keyboard reaches it, and focus there shows it.
    await page.keyboard.press("Tab");
    await copy.focus();
    await expect(copy.locator("xpath=..")).toHaveCSS("opacity", "1");
  }
  await expectNoAxeViolations(page);
});

test("Copy puts the reply's Markdown on the clipboard and says Copied", async ({
  page,
  browserName,
  context,
}) => {
  test.skip(browserName === "webkit", "WebKit grants no clipboard permission to a test.");
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await page.goto("/agent/atlas");
  const feed = conversation(page);
  // An archived reply whose Markdown has bold and a numbered list.
  const reply = feed
    .locator(".feed-message")
    .filter({ hasText: "The observer keeps three things" });
  await reply.scrollIntoViewIfNeeded();

  await reply.getByRole("button", { name: "Copy reply" }).click();

  const copied = await page.evaluate(() => navigator.clipboard.readText());
  expect(copied).toContain("The observer keeps three things for each compression pass:");
  expect(copied).toContain("**Observations**");
  expect(copied).toMatch(/^1\. /m);
  // The button says so, and a polite status says it for a screen reader.
  await expect(reply.getByRole("button", { name: "Copied" })).toBeVisible();
  await expect(reply.locator(".reply-foot [role=status]")).toHaveText("Copied");
  await expect(reply.getByRole("button", { name: "Copy reply" })).toBeVisible();
});

test("a reply that has streamed in gets its time once it is whole", async ({ page, mock }) => {
  await mock.post("/api/mock/delays", { data: { scale: 4 } });
  await page.goto("/agent/atlas");
  const feed = conversation(page);
  // Recorded in the page as the reply streams, since a slow machine can see
  // the stream end before a separate check looks at it.
  await page.evaluate(() => {
    const seen: boolean[] = [];
    (window as unknown as { streamingFootInert: boolean[] }).streamingFootInert = seen;
    new MutationObserver(() => {
      for (const foot of document.querySelectorAll<HTMLElement>(
        ".reply[data-streaming] .reply-foot",
      )) {
        seen.push(foot.inert);
      }
    }).observe(document.body, {
      subtree: true,
      childList: true,
      attributes: true,
      characterData: true,
    });
  });
  await send(page, "Tidy the wiki index");

  await expect(feed.locator(".reply[data-streaming]")).toHaveCount(1, { timeout: 30_000 });
  await expect(feed.locator(".reply[data-streaming]")).toHaveCount(0, { timeout: 30_000 });
  // A streaming reply's row is out of reach, so nothing in it can be focused or read.
  const inert = await page.evaluate(
    () => (window as unknown as { streamingFootInert: boolean[] }).streamingFootInert,
  );
  expect(inert.length).toBeGreaterThan(0);
  expect(inert.every(Boolean)).toBe(true);
  const finished = feed.locator(".reply").last();
  await expect(finished.locator("time")).toHaveText(/^\d{1,2}:\d{2}/);
  await expect(finished.locator(".reply-foot")).toHaveJSProperty("inert", false);
});
