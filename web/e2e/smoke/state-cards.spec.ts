import type { Locator, Page } from "@playwright/test";
import { expectNoAxeViolations } from "../support/axe";
import { expect, test } from "../support/fixtures";
import { expectSettingsOpen } from "../support/lazy";

/**
 * The state card an agent's Chat shows in place of the composer while the
 * agent isn't running: brittle couldn't start, drifter is stopped, and atlas
 * runs until it is stopped. The conversation above stays readable, and
 * nothing says it is reconnecting to an agent that isn't running.
 */

const ATLAS_GREETING = "Hi, this is atlas. You are in my conversation, not scout's.";

function card(page: Page, title: string): Locator {
  return page.getByRole("region", { name: title });
}

function composer(page: Page): Locator {
  return page.getByRole("textbox", { name: /^Message (atlas|brittle|drifter)$/ });
}

async function expectNoReconnecting(page: Page): Promise<void> {
  await expect(page.getByText(/Reconnecting/)).toHaveCount(0);
}

test("a failed agent says why, with its fix, in place of the composer", async ({ page }) => {
  await page.goto("/agent/brittle");
  const failed = card(page, "brittle couldn't start");

  await expect(failed).toContainText("Something in its settings needs fixing before it can run.");
  await expect(failed.getByRole("button", { name: "Fix settings" })).toBeVisible();
  await expect(failed.getByRole("button", { name: "Restart brittle" })).toBeVisible();
  await expect(failed.getByRole("button", { name: "Report a bug" })).toHaveCount(0);
  await expect(composer(page)).toHaveCount(0);
  await expectNoReconnecting(page);

  const reason = failed.getByText("model 'gpt-9' is not offered by provider 'openai'", {
    exact: false,
  });
  await expect(reason).toBeHidden();
  await failed.getByRole("button", { name: "Details" }).click();
  await expect(reason).toBeVisible();
  await expectNoAxeViolations(page);
});

test("a restart that fails again says so, and one after the fix brings the composer back", async ({
  page,
  mock,
}) => {
  await page.goto("/agent/brittle");
  const failed = card(page, "brittle couldn't start");
  await expect(failed).toBeVisible();

  await failed.getByRole("button", { name: "Restart brittle" }).click();
  await expect(page.getByRole("alert").filter({ hasText: "brittle failed" })).toBeVisible();
  await expect(failed).toContainText("It still couldn't start after the restart.");
  await expect(composer(page)).toHaveCount(0);

  await mock.post("/api/mock/fix-agent", { params: { agent: "brittle" } });
  await failed.getByRole("button", { name: "Restart brittle" }).click();

  await expect(failed).toHaveCount(0);
  await expect(composer(page)).toBeVisible();
  await expect(
    page
      .getByRole("region", { name: "Conversation with brittle" })
      .getByText("Hi, this is brittle. You are in my conversation, not scout's."),
  ).toBeVisible();
});

test("Fix settings opens brittle's settings where the problem shows", async ({ page }) => {
  await page.goto("/agent/brittle");
  await card(page, "brittle couldn't start").getByRole("button", { name: "Fix settings" }).click();

  // The check names brittle's main model, so it lands on Model with that field flagged.
  await expect(page).toHaveURL(/\/agent\/brittle\?settings=brittle\/model$/);
  const settings = await expectSettingsOpen(page);
  await expect(settings.getByRole("heading", { name: "Model", level: 2 })).toBeVisible();
  await expect(
    settings.getByRole("region", { name: "Main model" }).getByRole("combobox", { name: "Model" }),
  ).toHaveAttribute("aria-invalid", "true");
});

test("a stopped agent offers Start and Start automatically", async ({ page }) => {
  await page.goto("/agent/drifter");
  const stopped = card(page, "drifter is stopped");
  await expect(stopped).toBeVisible();
  await expect(composer(page)).toHaveCount(0);
  await expect(page.getByRole("heading", { name: "No messages yet" })).toHaveCount(0);
  await expectNoReconnecting(page);
  await expectNoAxeViolations(page);

  const autostart = stopped.getByRole("switch", { name: "Start automatically" });
  await expect(autostart).toHaveAttribute("aria-checked", "false");
  await autostart.click();
  await expect(autostart).toHaveAttribute("aria-checked", "true");

  await stopped.getByRole("button", { name: "Start drifter" }).click();
  await expect(stopped).toHaveCount(0);
  await expect(composer(page)).toBeFocused();
  await expect(
    page
      .getByRole("region", { name: "Conversation with drifter" })
      .getByText("Hi, this is drifter. You are in my conversation, not scout's."),
  ).toBeVisible();
});

test("stopping a running agent keeps its conversation readable", async ({ page, mock }) => {
  await page.goto("/agent/atlas");
  const conversation = page.getByRole("region", { name: "Conversation with atlas" });
  await expect(conversation.getByText(ATLAS_GREETING)).toBeVisible();
  await expect(composer(page)).toBeVisible();

  // Stopping and starting take long enough to see.
  await mock.post("/api/mock/delays", { data: { scale: 4 } });
  await page.getByRole("button", { name: "More for atlas" }).click();
  await page.getByRole("menuitem", { name: /^Stop atlas/ }).click();

  await expect(card(page, "Stopping atlas")).toBeVisible();
  await expect(composer(page)).toHaveCount(0);
  const stopped = card(page, "atlas is stopped");
  await expect(stopped).toBeVisible();
  await expect(conversation.getByText(ATLAS_GREETING)).toBeVisible();
  await expectNoReconnecting(page);
  await expectNoAxeViolations(page);

  await stopped.getByRole("button", { name: "Start atlas" }).click();
  await expect(card(page, "Starting atlas")).toBeVisible();
  await expect(composer(page)).toBeVisible();
  await expect(card(page, "Starting atlas")).toHaveCount(0);
});
