import type { Locator, Page } from "@playwright/test";
import { expectNoAxeViolations } from "../support/axe";
import { sendFromComposer } from "../support/composer";
import { expect, test } from "../support/fixtures";

const GREETING = "Hi, this is atlas. You are in my conversation, not scout's.";

test.use({ frozenClock: true });

function conversation(page: Page): Locator {
  return page.getByRole("region", { name: "Conversation with atlas" });
}

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
  const times = conversation(page).locator("time");
  await expect(times.first()).toBeAttached();
  const labels = await times.allTextContents();
  expect(labels.some((label) => /[A-Z][a-z]{2} \d{1,2}/.test(label))).toBe(true);
  expect(labels.some((label) => /^\d{1,2}:\d{2}/.test(label))).toBe(true);
});

test("Copy is quiet until the reply is hovered, tapped or focused, and reaches the keyboard", async ({
  page,
  isMobile,
}) => {
  await page.goto("/agent/atlas");
  const message = messageSaying(page, GREETING);
  const copy = message.getByRole("button", { name: "Copy reply" });
  await expect(copy).toBeAttached();

  if (isMobile) {
    await expect(copy.locator("xpath=..")).toHaveCSS("opacity", "0");
    await message.getByText(GREETING).tap();
    await expect(copy).toBeVisible();
    await expect(copy.locator("xpath=..")).toHaveCSS("opacity", "1");
  } else {
    await expect(copy.locator("xpath=..")).toHaveCSS("opacity", "0");
    await message.hover();
    await expect(copy.locator("xpath=..")).toHaveCSS("opacity", "1");
    await page.mouse.move(0, 0);
    await expect(copy.locator("xpath=..")).toHaveCSS("opacity", "0");
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
  isMobile,
}) => {
  test.skip(browserName === "webkit", "WebKit grants no clipboard permission to a test.");
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await page.goto("/agent/atlas");
  const feed = conversation(page);
  const reply = feed
    .locator(".feed-message")
    .filter({ hasText: "The observer keeps three things" });
  await reply.scrollIntoViewIfNeeded();

  if (isMobile) await reply.getByText("The observer keeps three things").tap();
  else await reply.hover();
  await reply.getByRole("button", { name: "Copy reply" }).click();

  const copied = await page.evaluate(() => navigator.clipboard.readText());
  expect(copied).toContain("The observer keeps three things for each compression pass:");
  expect(copied).toContain("**Observations**");
  expect(copied).toMatch(/^1\. /m);
  await expect(reply.getByRole("button", { name: "Copied" })).toBeVisible();
  await expect(reply.locator(".reply-meta [role=status]")).toHaveText("Copied");
  await expect(reply.getByRole("button", { name: "Copy reply" })).toBeVisible();
});

test("a reply that has streamed in gets its time once it is whole", async ({ page, mock }) => {
  await page.goto("/agent/atlas");
  await mock.manualTime();
  const feed = conversation(page);
  await page.evaluate(() => {
    const seen: boolean[] = [];
    (window as unknown as { streamingFootInert: boolean[] }).streamingFootInert = seen;
    new MutationObserver(() => {
      for (const foot of document.querySelectorAll<HTMLElement>(
        ".reply[data-streaming] .reply-meta",
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
  await expect(feed.getByText("Working", { exact: true })).toBeVisible();

  await mock.advance(1_590);
  await expect(feed.locator(".reply[data-streaming]")).toHaveCount(1);
  await mock.advance(60_000);
  await expect(feed.locator(".reply[data-streaming]")).toHaveCount(0);
  const inert = await page.evaluate(
    () => (window as unknown as { streamingFootInert: boolean[] }).streamingFootInert,
  );
  expect(inert.length).toBeGreaterThan(0);
  expect(inert.every(Boolean)).toBe(true);
  const finished = feed.locator(".reply").last();
  await expect(finished.locator("time")).toHaveText(/^\d{1,2}:\d{2}/);
  await expect(finished.locator(".reply-meta")).toHaveJSProperty("inert", false);
});
