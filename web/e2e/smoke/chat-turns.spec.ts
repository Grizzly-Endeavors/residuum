import type { Locator, Page } from "@playwright/test";
import { expectNoAxeViolations } from "../support/axe";
import { expect, test } from "../support/fixtures";

/**
 * How a turn reads in the chat: the agent's work between what it says about
 * it, and a turn that couldn't finish leaving its account behind.
 */

const GREETING = "Hi, this is atlas. You are in my conversation, not scout's.";

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

async function openAtlas(page: Page): Promise<void> {
  await page.goto("/agent/atlas");
  await expect(conversation(page).getByText(GREETING)).toBeVisible();
}

/** How far down the page `target` sits. */
async function top(target: Locator): Promise<number> {
  const box = await target.boundingBox();
  if (box === null) throw new Error("the element isn't on screen");
  return box.y;
}

test.describe("a turn that works in rounds", () => {
  test.beforeEach(async ({ mock }) => {
    await mock.post("/api/mock/delays", { data: { scale: 2 } });
  });

  test("keeps each round's steps between the texts around them, and the head last", async ({
    page,
    mock,
  }) => {
    // The turn waits at its last step, so the layout is looked at while it runs.
    await mock.post("/api/mock/turn-hold", { data: { held: true } });
    await openAtlas(page);
    await send(page, "segments: fix the port");
    const feed = conversation(page);

    const steps = feed.getByRole("button", { name: /^Edit(ing|ed) team\/wiki\/config\.toml/ });
    await expect(steps).toBeVisible({ timeout: 20_000 });
    const read = feed.getByRole("button", { name: /^Read 3 files/ });
    const ran = feed.getByRole("button", { name: /^Ran 2 commands/ });
    // The rounds the agent has said something after are collapsed to a line each.
    await expect(read).toHaveAttribute("aria-expanded", "false");
    await expect(ran).toHaveAttribute("aria-expanded", "false");

    const inOrder = [
      feed.getByText("Let me check the config first."),
      read,
      feed.getByText("The port is set twice. Fixing:"),
      ran,
      feed.getByText("One edit should do it."),
      steps,
      feed.getByText("Working", { exact: true }),
    ];
    const tops: number[] = [];
    for (const element of inOrder) tops.push(await top(element));
    expect(tops).toEqual([...tops].sort((a, b) => a - b));
    await expectNoAxeViolations(page);

    await mock.post("/api/mock/turn-hold", { data: { held: false } });
    await expect(feed.getByText("Done. The port is set once now")).toBeVisible({
      timeout: 20_000,
    });
    // Over, the head gives way to how long it took, and the last round is a line too.
    await expect(feed.getByText("Working", { exact: true })).toHaveCount(0);
    await expect(feed.getByText(/^Worked for \d+s$/)).toBeVisible();
    await expect(feed.getByRole("button", { name: /^Edited 1 file/ })).toHaveAttribute(
      "aria-expanded",
      "false",
    );
  });

  test("opens a collapsed round to its steps, and they stay where they were", async ({ page }) => {
    await openAtlas(page);
    await send(page, "segments: fix the port");
    const feed = conversation(page);
    await expect(feed.getByText("Done. The port is set once now")).toBeVisible({
      timeout: 20_000,
    });

    const ran = feed.getByRole("button", { name: /^Ran 2 commands/ });
    await ran.click();
    const commands = feed.getByRole("button", { name: /^Ran grep -n port/ });
    await expect(commands).toBeVisible();
    await expect(feed.getByText("The port is set twice. Fixing:")).toBeVisible();
    expect(await top(feed.getByText("The port is set twice. Fixing:"))).toBeLessThan(
      await top(commands),
    );
    expect(await top(commands)).toBeLessThan(await top(feed.getByText("One edit should do it.")));
  });
});

test.describe("a turn that couldn't finish", () => {
  test("leaves its account in the feed once the toast is gone, and sends the message again", async ({
    page,
    mock,
  }) => {
    await mock.post("/api/mock/delays", { data: { scale: 2 } });
    await openAtlas(page);
    await send(page, "error: check the wiki");
    const feed = conversation(page);

    const account = feed.getByText("atlas couldn't finish this reply");
    await expect(account).toBeVisible({ timeout: 15_000 });
    await expect(
      feed.getByText("The model provider didn't answer. Try sending your message again"),
    ).toBeVisible();
    // The steps it took before it failed are still there.
    await expect(feed.getByRole("button", { name: /^Searched memory/ })).toBeVisible();
    await expect(feed.getByText("Working", { exact: true })).toHaveCount(0);
    await expect(feed.getByText(/^Worked for/)).toHaveCount(0);

    // The toast says the same, until it's dismissed; the account stays.
    const toast = page.getByRole("alert").filter({ hasText: "The model provider didn't answer" });
    await expect(toast).toBeVisible();
    await toast.getByRole("button", { name: "Dismiss" }).click();
    await expect(toast).toHaveCount(0);
    await expect(account).toBeVisible();

    const details = feed.getByRole("button", { name: "Details" });
    await details.click();
    await expect(feed.getByText(/provider returned 503 Service Unavailable/)).toBeVisible();
    await expectNoAxeViolations(page);

    await feed.getByRole("button", { name: "Try again" }).click();
    await expect(feed.getByText("error: check the wiki")).toHaveCount(2);
    await expect(account).toHaveCount(2, { timeout: 15_000 });
  });
});

test.describe("what a screen reader is told", () => {
  /** The visually hidden status region of the conversation, which says only whole things. */
  function announced(page: Page, text: RegExp): Locator {
    return page.getByRole("status").filter({ hasText: text });
  }

  test("a turn starting, its reply complete, and a turn that couldn't finish", async ({
    page,
    mock,
  }) => {
    await mock.post("/api/mock/delays", { data: { scale: 2 } });
    await mock.post("/api/mock/turn-hold", { data: { held: true } });
    await openAtlas(page);
    await send(page, "check the wiki");
    await expect(announced(page, /^atlas is working$/)).toBeAttached();
    // The note it sends on the way is a message in the feed, not something read out.
    await expect(conversation(page).getByText("Looking through recent notes first.")).toBeVisible();
    await expect(announced(page, /Looking through/)).toHaveCount(0);

    await mock.post("/api/mock/turn-hold", { data: { held: false } });
    await expect(
      announced(page, /^atlas replied: I've looked into that and here's what I found/),
    ).toBeAttached({ timeout: 20_000 });

    await send(page, "error: check the wiki");
    await expect(announced(page, /^atlas couldn't finish$/)).toBeAttached({ timeout: 20_000 });
  });

  test("sits outside the scrolling conversation", async ({ page }) => {
    await openAtlas(page);
    await send(page, "check the wiki");
    await expect(announced(page, /^atlas (is working|replied)/)).toBeAttached();
    await expect(
      conversation(page)
        .getByRole("status")
        .filter({ hasText: /^atlas/ }),
    ).toHaveCount(0);
  });
});
