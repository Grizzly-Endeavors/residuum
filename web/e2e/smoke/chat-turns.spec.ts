import type { Locator, Page } from "@playwright/test";
import { expectNoAxeViolations } from "../support/axe";
import { sendFromComposer } from "../support/composer";
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
  return page.getByRole("combobox", { name: "Message atlas" });
}

async function send(page: Page, text: string): Promise<void> {
  await composer(page).fill(text);
  await sendFromComposer(composer(page));
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
  test("keeps each round's steps between the texts around them, and the head last", async ({
    page,
    mock,
  }) => {
    // The turn waits at its last step, so the layout is looked at while it runs.
    await mock.post("/api/mock/turn-hold", { data: { held: true } });
    await openAtlas(page);
    await mock.manualTime();
    await send(page, "segments: fix the port");
    const feed = conversation(page);
    // The turn head shows from the mock's `turn_started`, so the turn's timers are set.
    await expect(feed.getByText("Working", { exact: true })).toBeVisible();
    // Its last step, the edit's result, lands at 1.9s; the turn then waits at the hold.
    await mock.advance(2_000);

    const steps = feed.getByRole("button", { name: /^Edit(ing|ed) team\/wiki\/config\.toml/ });
    await expect(steps).toBeVisible();
    const read = feed.getByRole("button", { name: /^Read 3 files, thought/ });
    const ran = feed.getByRole("button", { name: /^Ran 2 commands/ });
    // The rounds the agent has said something after are collapsed to a line each.
    await expect(read).toHaveAttribute("aria-expanded", "false");
    await expect(ran).toHaveAttribute("aria-expanded", "false");

    const inOrder = [
      read,
      feed.getByText("Let me check the config first."),
      ran,
      feed.getByText("The port is set twice. Fixing:"),
      steps,
      feed.getByText("Working", { exact: true }),
    ];
    const tops: number[] = [];
    for (const element of inOrder) tops.push(await top(element));
    expect(tops).toEqual([...tops].sort((a, b) => a - b));
    await expectNoAxeViolations(page);

    await mock.post("/api/mock/turn-hold", { data: { held: false } });
    await mock.advance(60_000);
    await expect(feed.getByText("Done. The port is set once now")).toBeVisible();
    // Over, the head gives way to how long it took, and the last round is a line too.
    // The page read the turn's start at 0 and its end in the move that reached
    // 62s of simulated time (`advance` sets the page's clock to a move's end).
    await expect(feed.getByText("Working", { exact: true })).toHaveCount(0);
    await expect(feed.getByText("Worked for 1m 02s", { exact: true })).toBeVisible();
    await expect(feed.getByRole("button", { name: /^Edited 1 file/ })).toHaveAttribute(
      "aria-expanded",
      "false",
    );
  });

  test("opens a collapsed round to its steps, and they stay where they were", async ({
    page,
    mock,
  }) => {
    await openAtlas(page);
    await mock.manualTime();
    await send(page, "segments: fix the port");
    const feed = conversation(page);
    await expect(feed.getByText("Working", { exact: true })).toBeVisible();
    await mock.advance(60_000);
    await expect(feed.getByText("Done. The port is set once now")).toBeVisible();

    const ran = feed.getByRole("button", { name: /^Ran 2 commands/ });
    await ran.click();
    const commands = feed.getByRole("button", { name: /^Ran grep -n port/ });
    await expect(commands).toBeVisible();
    await expect(feed.getByText("The port is set twice. Fixing:")).toBeVisible();
    expect(await top(feed.getByText("Let me check the config first."))).toBeLessThan(
      await top(commands),
    );
    expect(await top(commands)).toBeLessThan(
      await top(feed.getByText("The port is set twice. Fixing:")),
    );
  });
});

test.describe("a turn that couldn't finish", () => {
  test("leaves its account in the feed once the toast is gone, and sends the message again", async ({
    page,
    mock,
  }) => {
    await openAtlas(page);
    await mock.manualTime();
    await send(page, "error: check the wiki");
    const feed = conversation(page);
    await expect(feed.getByText("Working", { exact: true })).toBeVisible();
    // The turn fails at 810ms, after its search.
    await mock.advance(60_000);

    const account = feed.getByText("atlas couldn't finish this reply");
    await expect(account).toBeVisible();
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
    // The resent turn has started once its head shows, so its timers are set.
    await expect(feed.getByText("Working", { exact: true })).toBeVisible();
    await mock.advance(60_000);
    await expect(account).toHaveCount(2);
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
    await mock.post("/api/mock/turn-hold", { data: { held: true } });
    await openAtlas(page);
    await mock.manualTime();
    await send(page, "check the wiki");
    await expect(announced(page, /^atlas is working$/)).toBeAttached();
    // The note it sends on the way is a message in the feed, not something read out.
    // It goes out at 300ms, while the turn waits at the hold before its reply.
    await mock.advance(400);
    await expect(conversation(page).getByText("Looking through recent notes first.")).toBeVisible();
    await expect(announced(page, /Looking through/)).toHaveCount(0);

    await mock.post("/api/mock/turn-hold", { data: { held: false } });
    await mock.advance(60_000);
    await expect(
      announced(page, /^atlas replied: I've looked into that and here's what I found/),
    ).toBeAttached();

    await send(page, "error: check the wiki");
    await expect(conversation(page).getByText("Working", { exact: true })).toBeVisible();
    await mock.advance(60_000);
    await expect(announced(page, /^atlas couldn't finish$/)).toBeAttached();
  });

  test("sits outside the scrolling conversation", async ({ page, mock }) => {
    await openAtlas(page);
    await mock.manualTime();
    await send(page, "check the wiki");
    await expect(announced(page, /^atlas (is working|replied)/)).toBeAttached();
    await expect(
      conversation(page)
        .getByRole("status")
        .filter({ hasText: /^atlas/ }),
    ).toHaveCount(0);
  });
});

test.describe("text and reasoning streaming in", () => {
  test("a reply grows with a caret at its end, and the complete message takes its place", async ({
    page,
    mock,
  }) => {
    await openAtlas(page);
    await mock.manualTime();
    await send(page, "Tidy the wiki index");
    const feed = conversation(page);
    // The turn head shows from the mock's `turn_started`, so the turn's timers are set.
    await expect(feed.getByText("Working", { exact: true })).toBeVisible();

    // Partway through: the reply's last call starts writing at 1.5s, a piece every
    // 45ms, so 1.59s in its first three pieces are in and its last aren't.
    await mock.advance(1_590);
    const reply = feed.getByText("I've looked into that and here's what I found:");
    await expect(reply).toBeVisible();
    await expect(feed.getByText("Would you like me to adjust any of these values?")).toHaveCount(0);
    await expect(feed.locator("[data-streaming]")).toHaveCount(1);
    await expect(feed.getByText("Working", { exact: true })).toBeVisible();
    // The caret is drawn after the last word so far: on the last paragraph, or the last list item.
    await expect
      .poll(() =>
        feed.locator(".prose[data-caret] .prose-body").evaluate((body) => {
          const last = body.lastElementChild;
          const target = last?.matches("ul, ol") ? last.lastElementChild : last;
          return target === null ? "none" : getComputedStyle(target, "::after").content;
        }),
      )
      .toBe('""');
    await expectNoAxeViolations(page);

    await mock.advance(60_000);
    await expect(feed.getByText("Would you like me to adjust any of these values?")).toBeVisible();
    await expect(feed.locator("[data-streaming]")).toHaveCount(0);
    await expect(feed.getByText("Working", { exact: true })).toHaveCount(0);
    // It is the one message, not the draft and the message.
    await expect(reply).toHaveCount(1);
  });

  test("stopping keeps the text so far, and says it was stopped", async ({ page, mock }) => {
    await openAtlas(page);
    await mock.manualTime();
    await send(page, "Tidy the wiki index");
    const feed = conversation(page);
    await expect(feed.getByText("Working", { exact: true })).toBeVisible();

    // Partway through the reply: its first three pieces are in, the rest aren't.
    await mock.advance(1_590);
    await expect(feed.getByText("I've looked into that and here's what I found:")).toBeVisible();
    await page.getByRole("button", { name: "Stop the reply" }).click();

    await expect(feed.getByText("Stopped here")).toBeVisible();
    await expect(feed.getByText(/^Stopped by you/)).toBeVisible();
    await expect(feed.getByText("I've looked into that and here's what I found:")).toBeVisible();
    await expect(feed.getByText("Would you like me to adjust any of these values?")).toHaveCount(0);
    await expect(feed.locator("[data-streaming]")).toHaveCount(0);
  });

  test("a reply that starts over says it is retrying, then keeps the second attempt", async ({
    page,
    mock,
  }) => {
    await openAtlas(page);
    await mock.manualTime();
    await send(page, "retry: look at the routing doc");
    const feed = conversation(page);
    await expect(feed.getByText("Working", { exact: true })).toBeVisible();

    // The first attempt starts over at 555ms; the second one's text begins at 1.355s.
    await mock.advance(800);
    await expect(feed.getByText("Retrying…")).toBeVisible();
    await expect(feed.getByText("Let me look at the notification routing doc")).toHaveCount(0);

    await mock.advance(600);
    await expect(feed.getByText(/^The routing doc sends urgent notices/)).toBeVisible();
    await expect(feed.getByText("Retrying…")).toHaveCount(0);
    await mock.advance(60_000);
    await expect(feed.getByText("Working", { exact: true })).toHaveCount(0);
  });

  test("reasoning streams in muted, then folds to its line and opens to all of it", async ({
    page,
    mock,
  }) => {
    await openAtlas(page);
    await mock.manualTime();
    await send(page, "think about the fallback order");
    const feed = conversation(page);
    await expect(feed.getByText("Working", { exact: true })).toBeVisible();

    // Reasoning starts at 40ms and a piece lands every 45ms: two are in by 100ms.
    await mock.advance(100);
    await expect(feed.getByText("Thinking", { exact: true })).toBeVisible();
    await expect(feed.getByText(/The question is whether to cascade/)).toBeVisible();
    await expectNoAxeViolations(page);

    // The reasoning is complete at 625ms, and the reply's first pieces follow at 665ms.
    // The page read its start in the move to 100ms and its end in the move to
    // 650ms: 550ms, under a second, so its line names no time.
    await mock.advance(550);
    const thought = feed.getByRole("button", { name: "Thought", exact: true });
    await expect(thought).toBeVisible();
    await expect(feed.getByText("Thinking", { exact: true })).toHaveCount(0);
    await expect(thought).toHaveAttribute("aria-expanded", "false");

    await mock.advance(100);
    await expect(feed.getByText(/^Retry three times with backoff/)).toBeVisible();
    await mock.advance(60_000);

    await thought.click();
    await expect(feed.getByText(/I should answer with that order/)).toBeVisible();
  });

  test("history keeps the reasoning, in the line of the step that did it", async ({
    page,
    mock,
  }) => {
    await openAtlas(page);
    await mock.manualTime();
    await send(page, "think about the fallback order");
    const feed = conversation(page);
    await expect(feed.getByText("Working", { exact: true })).toBeVisible();
    await mock.advance(60_000);
    await expect(feed.getByText(/^Retry three times with backoff/)).toBeVisible();
    await expect(feed.getByText("Working", { exact: true })).toHaveCount(0);

    // The page's own record of the turn is gone; history has it, readable.
    await page.reload();
    await expect(feed.getByText(/^Retry three times with backoff/)).toBeVisible();
    const thought = feed.getByRole("button", { name: "Thought" });
    await thought.click();
    await expect(feed.getByText(/Cascading is what most setups expect/)).toBeVisible();
  });
});

test.describe("turns from other places", () => {
  test("a message from Telegram shows with who sent it, and its reply says where it went", async ({
    page,
    mock,
  }) => {
    await openAtlas(page);
    await mock.manualTime();
    await mock.post("/api/mock/telegram-message", { params: { agent: "atlas" } });
    const feed = conversation(page);

    await expect(feed.getByText("Alex · telegram · direct message")).toBeVisible();
    await expect(
      feed.getByText("Can you check what the routing doc says about urgent notices?"),
    ).toBeVisible();
    await expect(feed.getByText("Working", { exact: true })).toBeVisible();

    // The reply's first pieces are in by 790ms, after its search has run until 705ms.
    await mock.advance(800);
    await expect(feed.getByText(/^Urgent notices go to every channel/)).toBeVisible();
    await mock.advance(60_000);
    await expect(feed.getByText("Sent to Telegram")).toBeVisible();
    await expect(feed.getByText("Working", { exact: true })).toHaveCount(0);
    await expectNoAxeViolations(page);
  });

  test("a message sent from another tab shows here as it is sent, once in the tab that sent it", async ({
    page,
    mock,
  }) => {
    await openAtlas(page);
    const other = await page.context().newPage();
    await other.goto("/agent/atlas");
    await expect(conversation(other).getByText(GREETING)).toBeVisible();
    await expect
      .poll(async () => {
        const answer = (await (
          await page.request.get("/api/mock/connected-pages?agent=atlas")
        ).json()) as { pages: number };
        return answer.pages;
      })
      .toBe(2);

    // Both pages share the context's clock, which follows the mock's simulated time.
    await mock.manualTime();
    await send(other, "Check the routing doc, please");
    await expect(conversation(page).getByText("Check the routing doc, please")).toBeVisible();
    await expect(conversation(page).getByText("Working", { exact: true })).toBeVisible();
    await expect(conversation(other).getByText("Working", { exact: true })).toBeVisible();
    await expect(conversation(other).getByText("Check the routing doc, please")).toHaveCount(1);

    await mock.advance(60_000);
    // Started at 0 and read as ended in the move that reached 60s.
    await expect(conversation(page).getByText("Worked for 1m 00s", { exact: true })).toBeVisible();
    await expect(conversation(page).getByText("Check the routing doc, please")).toHaveCount(1);
    await other.close();
  });
});
