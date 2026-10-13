import type { Locator, Page } from "@playwright/test";
import { expectNoAxeViolations } from "../support/axe";
import { sendFromComposer } from "../support/composer";
import { expect, test, type MockControls } from "../support/fixtures";

/**
 * A message sent while the agent works waits apart from the conversation
 * until the agent reads it, and can be sent now or sent to a multitask
 * session instead.
 */

const GREETING = "Hi, this is atlas. You are in my conversation, not scout's.";

function conversation(page: Page): Locator {
  return page.getByRole("region", { name: "Conversation with atlas" });
}

function waiting(page: Page): Locator {
  return page.getByRole("region", { name: "Waiting for atlas to read" });
}

async function send(page: Page, text: string): Promise<void> {
  const box = page.getByRole("combobox", { name: "Message atlas" });
  await box.fill(text);
  await sendFromComposer(box);
}

/** A turn that waits at its end until let go, so messages sent during it wait too. */
async function startHeldTurn(page: Page, mock: MockControls): Promise<void> {
  await mock.post("/api/mock/turn-hold", { data: { held: true } });
  await page.goto("/agent/atlas");
  await expect(conversation(page).getByText(GREETING)).toBeVisible();
  await send(page, "look into the outage");
  await expect(conversation(page).getByText("Working", { exact: true })).toBeVisible();
}

test("a message sent while the agent works waits, then joins the chat once read", async ({
  page,
  mock,
}) => {
  await startHeldTurn(page, mock);
  await send(page, "also check the error logs");

  const message = waiting(page)
    .getByRole("listitem")
    .filter({ hasText: "also check the error logs" });
  await expect(message).toBeVisible();
  // Not part of the conversation yet: no user bubble says it.
  await expect(
    conversation(page).locator(".user-message").filter({ hasText: "also check the error logs" }),
  ).toHaveCount(0);
  await expectNoAxeViolations(page);

  await mock.post("/api/mock/turn-hold", { data: { held: false } });
  await expect(waiting(page)).toHaveCount(0);
  await expect(
    conversation(page).locator(".user-message").filter({ hasText: "also check the error logs" }),
  ).toBeVisible();
});

test("Send now stops the turn, and the message starts the next one", async ({ page, mock }) => {
  await startHeldTurn(page, mock);
  await send(page, "actually, check the logs first");
  const message = waiting(page).getByRole("listitem").filter({ hasText: "check the logs first" });

  await message.getByRole("button", { name: "Send now" }).click();

  await expect(conversation(page).getByText(/^Stopped by you/)).toBeVisible();
  await expect(waiting(page)).toHaveCount(0);
  await expect(
    conversation(page).locator(".user-message").filter({ hasText: "check the logs first" }),
  ).toBeVisible();
});

test("Multitask sends the message to a forked session instead", async ({ page, mock }) => {
  await startHeldTurn(page, mock);
  await send(page, "compare the three quotes");
  const message = waiting(page)
    .getByRole("listitem")
    .filter({ hasText: "compare the three quotes" });

  await message.getByRole("button", { name: "Multitask" }).click();

  await expect(page.getByText(/^Forked into /)).toBeVisible();
  await expect(waiting(page)).toHaveCount(0);
  // The turn it waited on carries on, and the message never reaches it.
  await expect(conversation(page).getByText("Working", { exact: true })).toBeVisible();
  await expect(
    conversation(page).locator(".user-message").filter({ hasText: "compare the three quotes" }),
  ).toHaveCount(0);
});
