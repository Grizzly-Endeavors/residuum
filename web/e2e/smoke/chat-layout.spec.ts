import type { Locator, Page } from "@playwright/test";
import { sendFromComposer } from "../support/composer";
import { expect, test } from "../support/fixtures";

/**
 * The chat's layout: the composer floating over a full-height conversation,
 * the room kept under the newest line, a composer that resizes without moving
 * the thread, expanding and collapsing in place, toasts above the composer,
 * focus after Send, Enter on a touch screen, and the history's loading and
 * failure states.
 */

const GREETING = "Hi, this is atlas. You are in my conversation, not scout's.";

function conversation(page: Page): Locator {
  return page.getByRole("region", { name: "Conversation with atlas" });
}

function box(page: Page): Locator {
  return page.getByRole("combobox", { name: "Message atlas" });
}

async function rect(locator: Locator): Promise<{ x: number; y: number; w: number; h: number }> {
  const found = await locator.boundingBox();
  if (!found) throw new Error("the element isn't laid out");
  return { x: found.x, y: found.y, w: found.width, h: found.height };
}

async function top(locator: Locator): Promise<number> {
  return (await rect(locator)).y;
}

/** Type `lines` lines into the message box, the way a person writes a long message. */
async function typeLines(page: Page, lines: number): Promise<void> {
  await box(page).click();
  await box(page).pressSequentially("line 1");
  for (let line = 2; line <= lines; line++) {
    await page.keyboard.press("Shift+Enter");
    await page.keyboard.type(`line ${String(line)}`);
  }
}

async function openChat(page: Page): Promise<Locator> {
  await page.goto("/agent/atlas");
  const feed = conversation(page);
  await expect(feed.getByText(GREETING)).toBeVisible();
  // The feed has settled at the bottom.
  await expect
    .poll(async () => feed.evaluate((el) => el.scrollHeight - el.scrollTop - el.clientHeight))
    .toBeLessThan(2);
  return feed;
}

test.describe("the composer over the conversation", () => {
  test("the scrolling area runs to the bottom of the place, behind the composer", async ({
    page,
  }) => {
    const feed = await openChat(page);
    const area = await rect(feed);
    const field = await rect(page.locator("form.composer"));
    const viewport = page.viewportSize();
    if (!viewport) throw new Error("no viewport");
    // The conversation's area reaches the foot of the main region, which the composer floats inside.
    const place = await rect(page.locator(".chat-place"));
    expect(area.y + area.h).toBeCloseTo(place.y + place.h, 0);
    expect(field.y).toBeGreaterThan(area.y);
    expect(field.y + field.h).toBeLessThanOrEqual(area.y + area.h);
    // The content scrolls under the composer, so it takes its room.
    const padding = await feed.evaluate((el) =>
      Number.parseFloat(getComputedStyle(el.firstElementChild as Element).paddingBottom),
    );
    expect(padding).toBeGreaterThan(field.h);
  });

  test("the newest reply rests well above the composer while the reader follows", async ({
    isMobile,
    page,
  }) => {
    const feed = await openChat(page);
    const reply = await rect(feed.getByText(GREETING));
    const field = await rect(page.locator("form.composer"));
    const clear = field.y - (reply.y + reply.h);
    // A quarter of the view, at least 96px, beside a composer on a wide screen; a little on a phone.
    if (isMobile) expect(clear).toBeGreaterThanOrEqual(32);
    else {
      const area = await rect(feed);
      expect(clear).toBeGreaterThanOrEqual(96);
      // The reply's quiet row (its time and Copy) is 16px of the room the last line is measured from.
      expect(clear).toBeLessThanOrEqual(area.h * 0.25 + 24 + 16);
    }
  });

  test("a composer that grows leaves the thread where it is, and leaves the reader following", async ({
    isMobile,
    page,
  }) => {
    const feed = await openChat(page);
    const reply = feed.getByText(GREETING);
    const before = await top(reply);

    // Two lines more fit the room under the newest line on any screen.
    await typeLines(page, 3);
    expect(Math.abs((await top(reply)) - before)).toBeLessThan(2);

    // Many more can't on a phone, where only as much as would cover the reply moves it.
    await typeLines(page, 1);
    await box(page).fill("a\nb\nc\nd\ne\nf\ng\nh");
    const field = await rect(page.locator("form.composer"));
    const after = await rect(reply);
    if (isMobile) expect(after.y + after.h).toBeLessThanOrEqual(field.y);
    else expect(Math.abs(after.y - before)).toBeLessThan(1);
    // Growing the composer never makes the reader look scrolled away.
    await expect(page.getByRole("button", { name: /Jump to latest|New reply/ })).toHaveCount(0);
  });

  test("a composer that shrinks doesn't pull the thread down after it", async ({ page }) => {
    const feed = await openChat(page);
    const reply = feed.getByText(GREETING);
    await box(page).fill("a\nb\nc\nd\ne\nf\ng\nh");
    const grown = await top(reply);

    await box(page).fill("");
    expect(Math.abs((await top(reply)) - grown)).toBeLessThan(1);
    await expect(page.getByRole("button", { name: /Jump to latest|New reply/ })).toHaveCount(0);
  });

  test("a reader who scrolled up isn't moved by the composer growing or shrinking", async ({
    page,
  }) => {
    const feed = await openChat(page);
    await feed.evaluate((el) => {
      el.scrollTop = el.scrollTop - 600;
    });
    const probe = feed.getByText("Walk me through what the observer actually stores");
    await expect(page.getByRole("button", { name: "Jump to latest" })).toBeVisible();
    const before = await feed.evaluate((el) => el.scrollTop);

    await box(page).fill("a\nb\nc\nd\ne\nf\ng\nh");
    expect(await feed.evaluate((el) => el.scrollTop)).toBe(before);
    await box(page).fill("");
    expect(await feed.evaluate((el) => el.scrollTop)).toBe(before);
    await expect(probe).toBeAttached();
    await expect(page.getByRole("button", { name: "Jump to latest" })).toBeVisible();
  });
});

test.describe("opening and closing in the conversation", () => {
  /** A press at the control's own coordinates: no scrolling to it first, as a person's tap has none. */
  async function press(page: Page, isMobile: boolean, control: Locator): Promise<void> {
    const at = await rect(control);
    const x = at.x + at.w / 2;
    const y = at.y + at.h / 2;
    if (isMobile) await page.touchscreen.tap(x, y);
    else await page.mouse.click(x, y);
  }

  test("an activity summary and a step stay where they were pressed", async ({
    isMobile,
    mock,
    page,
  }) => {
    await mock.post("/api/mock/delays", { data: { scale: 0 } });
    const feed = await openChat(page);
    await box(page).fill("Look at the wiki");
    await sendFromComposer(box(page));
    await expect(feed.getByText("Looking through recent notes first.").first()).toBeVisible();
    const summary = feed.getByRole("button", { name: /^Searched memory, read 2 files/ }).last();
    await expect(summary).toHaveAttribute("aria-expanded", "false", { timeout: 20_000 });
    await summary.evaluate((el) => {
      el.scrollIntoView({ block: "center" });
    });
    await expect(summary).toBeInViewport();

    const resting = await top(summary);
    await press(page, isMobile, summary);
    await expect(summary).toHaveAttribute("aria-expanded", "true");
    await expect.poll(async () => Math.abs((await top(summary)) - resting)).toBeLessThan(2);

    const step = feed.getByRole("button", { name: /^Read(ing)? team\/wiki\/index\.md/ }).last();
    const stepAt = await top(step);
    await press(page, isMobile, step);
    await expect(step).toHaveAttribute("aria-expanded", "true");
    await expect.poll(async () => Math.abs((await top(step)) - stepAt)).toBeLessThan(2);
    await press(page, isMobile, step);
    await expect(step).toHaveAttribute("aria-expanded", "false");
    await expect.poll(async () => Math.abs((await top(step)) - stepAt)).toBeLessThan(2);

    await press(page, isMobile, summary);
    await expect(summary).toHaveAttribute("aria-expanded", "false");
    await expect.poll(async () => Math.abs((await top(summary)) - resting)).toBeLessThan(2);
  });

  test("a card's Show all stays where it was pressed", async ({ isMobile, page }) => {
    test.skip(!isMobile, "the sample card's text only overflows at phone width");
    const feed = await openChat(page);
    const showAll = feed.getByRole("button", { name: "Show all" }).first();
    await showAll.evaluate((el) => {
      el.scrollIntoView({ block: "center" });
    });
    const resting = await top(showAll);

    await press(page, isMobile, showAll);
    const showLess = feed.getByRole("button", { name: "Show less" }).first();
    await expect(showLess).toBeVisible();
    await expect.poll(async () => Math.abs((await top(showLess)) - resting)).toBeLessThan(2);
    await press(page, isMobile, showLess);
    await expect(feed.getByRole("button", { name: "Show all" }).first()).toBeVisible();
    await expect
      .poll(async () =>
        Math.abs((await top(feed.getByRole("button", { name: "Show all" }).first())) - resting),
      )
      .toBeLessThan(2);
  });

  test("pressing one stops following, until the reader is back at the end", async ({
    isMobile,
    mock,
    page,
  }) => {
    await mock.post("/api/mock/delays", { data: { scale: 0 } });
    const feed = await openChat(page);
    const summary = feed.getByRole("button", { name: /^Ran 1 command/ }).first();
    await summary.evaluate((el) => {
      el.scrollIntoView({ block: "center" });
    });
    await press(page, isMobile, summary);
    await expect(summary).toHaveAttribute("aria-expanded", "true");

    // A reply landing now leaves them where they are, and says so.
    await mock.post("/api/mock/teammate-message?agent=atlas");
    await expect(page.getByRole("button", { name: "New reply, jump to latest" })).toBeVisible();
    await page.getByRole("button", { name: "New reply, jump to latest" }).click();
    await expect(feed.getByText("scout asked me to check the wiki index. On it.")).toBeInViewport();
  });
});

test.describe("toasts", () => {
  test("sit above the composer, and keep clear as it grows", async ({ page }) => {
    await openChat(page);
    await box(page).fill("/stop now");
    await sendFromComposer(box(page));
    const toast = page.locator(".ui-toast", { hasText: "Couldn't run /stop" });
    await expect(toast).toBeVisible();

    const field = page.locator("form.composer");
    const at = await rect(toast);
    expect(at.y + at.h).toBeLessThanOrEqual((await rect(field)).y);

    await typeLines(page, 6);
    await expect
      .poll(async () => {
        const toastAt = await rect(toast);
        const fieldAt = await rect(field);
        return fieldAt.y - (toastAt.y + toastAt.h);
      })
      .toBeGreaterThanOrEqual(0);
    // Still in reach, not pushed off the page.
    expect((await rect(toast)).y).toBeGreaterThan(0);
  });
});

test.describe("the message box", () => {
  test("keeps Send inside the box when the model name doesn't fit", async ({ page }) => {
    // The narrowest phones leave the model's name too little room.
    await page.setViewportSize({ width: 320, height: 640 });
    await openChat(page);
    const composer = page.locator("form.composer");
    const send = page.getByRole("button", { name: "Send" });
    const outer = await rect(composer);
    const padding = await composer.evaluate((form) =>
      parseFloat(getComputedStyle(form).paddingRight),
    );
    const button = await rect(send);
    // Inside the box's padding, not just its border.
    expect(button.x + button.w).toBeLessThanOrEqual(outer.x + outer.w - padding + 0.5);
  });

  test("keeps focus after Send is pressed", async ({ isMobile, page }) => {
    await openChat(page);
    await box(page).fill("Check the wiki");
    const send = page.getByRole("button", { name: "Send" });
    const at = await rect(send);
    if (isMobile) await page.touchscreen.tap(at.x + at.w / 2, at.y + at.h / 2);
    else await page.mouse.click(at.x + at.w / 2, at.y + at.h / 2);
    await expect(box(page)).toHaveValue("");
    await expect(box(page)).toBeFocused();
    await expect(page.getByText("Check the wiki").first()).toBeVisible();
  });

  test("Enter sends with a keyboard, and starts a new line on a touch screen", async ({
    isMobile,
    page,
  }) => {
    await openChat(page);
    await box(page).click();
    if (isMobile) {
      await expect(box(page)).toHaveAttribute("enterkeyhint", "enter");
      await page.keyboard.type("first");
      await page.keyboard.press("Enter");
      await page.keyboard.type("second");
      await expect(box(page)).toHaveValue("first\nsecond");
      await page.getByRole("button", { name: "Send" }).click();
    } else {
      await expect(box(page)).not.toHaveAttribute("enterkeyhint");
      await page.keyboard.type("first");
      await page.keyboard.press("Shift+Enter");
      await page.keyboard.type("second");
      await expect(box(page)).toHaveValue("first\nsecond");
      await page.keyboard.press("Enter");
    }
    await expect(box(page)).toHaveValue("");
    await expect(conversation(page).getByText("second").first()).toBeVisible();
  });
});

test.describe("the history", () => {
  test("shows a skeleton while it loads", async ({ page }) => {
    await page.route("**/api/agents/atlas/chat/history", async (route) => {
      await new Promise((resolve) => setTimeout(resolve, 1500));
      await route.continue();
    });
    await page.goto("/agent/atlas");
    const feed = conversation(page);
    await expect(feed.getByText("Loading the conversation")).toBeAttached();
    await expect(feed.getByText("No messages yet")).toHaveCount(0);
    await expect(feed.getByText(GREETING)).toBeVisible({ timeout: 15_000 });
    await expect(feed.getByText("Loading the conversation")).toHaveCount(0);
  });

  test("says in the conversation when it didn't load, and Retry loads it", async ({ page }) => {
    let failing = true;
    await page.route("**/api/agents/atlas/chat/history", async (route) => {
      if (failing) await route.fulfill({ status: 500, body: "nope" });
      else await route.continue();
    });
    await page.goto("/agent/atlas");
    const feed = conversation(page);
    await expect(
      feed.getByRole("heading", { name: "Couldn't load the conversation" }),
    ).toBeVisible();
    await expect(feed.getByText(/ran into a problem on its end/)).toBeVisible();
    // The failure is in the conversation, not a toast over it.
    await expect(page.locator(".ui-toast")).toHaveCount(0);

    failing = false;
    await feed.getByRole("button", { name: "Retry" }).click();
    await expect(feed.getByText(GREETING)).toBeVisible();
    await expect(feed.getByRole("heading", { name: "Couldn't load the conversation" })).toHaveCount(
      0,
    );
  });
});
