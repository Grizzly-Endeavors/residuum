import type { Locator, Page } from "@playwright/test";
import { expectNoAxeViolations } from "../support/axe";
import { sendFromComposer } from "../support/composer";
import { expect, test } from "../support/fixtures";

/**
 * A reply's Markdown on the page: the table that scrolls instead of breaking
 * its words, task items that read as words, links that open beside the app,
 * code a keyboard can reach, and headings that sit below the page's own.
 */

const GREETING = "Hi, this is atlas. You are in my conversation, not scout's.";

function conversation(page: Page): Locator {
  return page.getByRole("region", { name: "Conversation with atlas" });
}

/** Ask the mock for its Markdown showcase and wait for it to land. */
async function showcase(page: Page): Promise<Locator> {
  await page.goto("/agent/atlas");
  const feed = conversation(page);
  await expect(feed.getByText(GREETING)).toBeVisible();
  const box = page.getByRole("combobox", { name: "Message atlas" });
  await box.fill("markdown please");
  await sendFromComposer(box);
  await expect(feed.getByRole("heading", { name: "Rollout notes" })).toBeVisible();
  return feed;
}

test("a table keeps its words whole and scrolls when it is wider than the message", async ({
  page,
  isMobile,
}) => {
  const feed = await showcase(page);
  const table = feed.getByRole("group", { name: "Table" });
  await table.scrollIntoViewIfNeeded();

  // No cell breaks a word across lines: a one-word cell is a single line.
  const brokenWords = await table.evaluate(
    (group) =>
      [...group.querySelectorAll("td, th")].filter((cell) => {
        const text = cell.textContent.trim();
        if (text.includes(" ")) return false;
        const range = document.createRange();
        range.selectNodeContents(cell);
        return range.getClientRects().length > 1;
      }).length,
  );
  expect(brokenWords).toBe(0);

  const { scrollWidth, clientWidth } = await table.evaluate((group) => ({
    scrollWidth: group.scrollWidth,
    clientWidth: group.clientWidth,
  }));
  if (isMobile) expect(scrollWidth).toBeGreaterThan(clientWidth);
  else expect(scrollWidth).toBeLessThanOrEqual(clientWidth);

  // The group takes focus, and the arrow keys move what it shows.
  await table.focus();
  await expect(table).toBeFocused();
  if (isMobile) {
    await page.keyboard.press("ArrowRight");
    await expect.poll(() => table.evaluate((group) => group.scrollLeft)).toBeGreaterThan(0);
  }
});

test("task items read as done and to do, with no checkbox or bullet", async ({ page }) => {
  const feed = await showcase(page);
  await expect(feed.getByRole("checkbox")).toHaveCount(0);
  const items = feed.getByRole("listitem").filter({ hasText: /^(Done|To do): / });
  await expect(items).toHaveCount(4);
  await expect(items.nth(0)).toHaveText("Done: Wire the discord adapter");
  await expect(items.nth(1)).toHaveText(
    "Done: Cascade to the next channel when one is unreachable",
  );
  // The parent item holds the nested one's words as well.
  await expect(items.nth(2)).toContainText(
    "To do: Park the notification in the inbox when every channel is down",
  );
  await expect(items.nth(3)).toHaveText("To do: Write the inbox summary line");
  const style = await items.first().evaluate((li) => getComputedStyle(li).listStyleType);
  expect(style).toBe("none");
  // A plain item keeps its bullet.
  const plain = feed.getByRole("listitem").filter({ hasText: "A plain bullet stays a bullet" });
  expect(await plain.evaluate((li) => getComputedStyle(li).listStyleType)).not.toBe("none");
});

test("a link opens in a new tab and leaves the chat where it is", async ({ page }) => {
  const feed = await showcase(page);
  const link = feed.getByRole("link", { name: "the notification guide" });
  await expect(link).toHaveAttribute("target", "_blank");
  await expect(link).toHaveAttribute("rel", "noopener noreferrer");

  const popup = page.waitForEvent("popup");
  await link.click();
  await popup;
  await expect(page).toHaveURL(/\/agent\/atlas$/);
  await expect(feed.getByRole("heading", { name: "Rollout notes" })).toBeVisible();
});

test("a workspace path link still opens the file in the panel", async ({ page }) => {
  const feed = await showcase(page);
  const path = feed.getByRole("link", { name: "team/wiki/notification-fallbacks.md" }).last();
  await expect(path).not.toHaveAttribute("target");
  await path.click();
  await expect(page).toHaveURL(/panel=file:team\/wiki\/notification-fallbacks\.md/);
});

test("a code block can be focused and scrolled, and its Copy button hides what runs under it", async ({
  page,
}) => {
  const feed = await showcase(page);
  const code = feed.getByRole("group", { name: "Code, toml" });
  await code.scrollIntoViewIfNeeded();
  await code.focus();
  await expect(code).toBeFocused();

  // The block overflows, and the keyboard scrolls it.
  await expect.poll(() => code.evaluate((pre) => pre.scrollWidth > pre.clientWidth)).toBe(true);
  await page.keyboard.press("ArrowRight");
  await expect.poll(() => code.evaluate((pre) => pre.scrollLeft)).toBeGreaterThan(0);

  const copy = feed.getByRole("button", { name: "Copy code" });
  const fill = await copy.evaluate((button) => getComputedStyle(button).backgroundColor);
  expect(fill).not.toBe("rgba(0, 0, 0, 0)");
});

test("a reply's headings sit below the page's", async ({ page }) => {
  const feed = await showcase(page);
  const reply = feed.getByRole("heading", { name: "Rollout notes" });
  await expect(reply).toHaveJSProperty("tagName", "H3");
  await expect(feed.getByRole("heading", { name: "Channels" })).toHaveJSProperty("tagName", "H4");
  await expect(feed.getByRole("heading", { name: "Open questions" })).toHaveJSProperty(
    "tagName",
    "H5",
  );
  // The place's own title stays the page's one h1.
  await expect(page.getByRole("heading", { level: 1 })).toHaveCount(1);
});

test("a reply with a table, tasks and code has no accessibility violations", async ({ page }) => {
  const feed = await showcase(page);
  await feed.getByRole("group", { name: "Table" }).scrollIntoViewIfNeeded();
  await expectNoAxeViolations(page);
});
