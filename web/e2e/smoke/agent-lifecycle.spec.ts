import type { Locator, Page } from "@playwright/test";
import { expectNoAxeViolations } from "../support/axe";
import { expect, test } from "../support/fixtures";
import { expectPaletteOpen, expectSettingsOpen } from "../support/lazy";

/**
 * Creating agents and running their lifecycle from Home: the Create agent
 * dialog (a sheet on phones) from Home's New agent, the rail's "+" and the
 * palette, a board row's menu, delete with Undo, and Recently deleted. The scenario has
 * atlas and scout running, drifter stopped and brittle failed.
 */

function board(page: Page): Locator {
  return page.getByRole("table", { name: "Agents" });
}

function boardRow(page: Page, agent: string): Locator {
  return board(page)
    .getByRole("row")
    .filter({ has: page.getByRole("link", { name: agent, exact: true }) });
}

function createDialog(page: Page): Locator {
  return page.getByRole("dialog", { name: "Create an agent" });
}

/** A toast saying `text`, polite or an alert. */
function toast(page: Page, text: string): Locator {
  return page.locator("[data-overlay-host]").getByText(text, { exact: true });
}

/** The rail, opened in its drawer on a phone. */
async function openRail(page: Page, isMobile: boolean): Promise<Locator> {
  if (isMobile) {
    await page
      .getByRole("navigation", { name: "Main" })
      .getByRole("button", { name: "Menu" })
      .click();
  }
  return page.getByRole("navigation", { name: "Places and agents" });
}

/**
 * Open an agent's "…" menu on the board. The trigger is ready once it is
 * enabled and closed; before that a click lands on a row the board is still
 * drawing, or opens nothing because a dialog that was closing still covers it.
 */
async function openMenu(page: Page, agent: string): Promise<Locator> {
  const trigger = board(page).getByRole("button", { name: `Manage ${agent}` });
  await expect(trigger).toBeEnabled();
  await expect(trigger).toHaveAttribute("aria-expanded", "false");
  await trigger.click();
  const menu = page.getByRole("menu", { name: `Manage ${agent}` });
  await expect(menu).toBeVisible();
  return menu;
}

/** Path and query of the page's URL, decoded. */
function address(page: Page): string {
  const url = new URL(page.url());
  return decodeURIComponent(`${url.pathname}${url.search}`);
}

test("the root address opens Home", async ({ page }) => {
  await page.goto("/");

  await expect(page).toHaveURL(/\/home$/);
  await expect(page.getByRole("heading", { name: "Home", level: 1 })).toBeVisible();
});

test("New agent creates an agent that joins the board and the rail", async ({ page, isMobile }) => {
  await page.goto("/home");
  await page.getByRole("button", { name: "New agent" }).click();
  const dialog = createDialog(page);
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole("textbox", { name: "Name" })).toBeFocused();
  await expectNoAxeViolations(page, { within: "[data-overlay-host]" });

  await dialog.getByRole("textbox", { name: "Name" }).fill("research-buddy");
  await dialog
    .getByRole("textbox", { name: "What should it help with?" })
    .fill("Keep my reading list");
  await dialog.getByRole("button", { name: "Create agent" }).click();

  await expect(dialog).toBeHidden();
  await expect(toast(page, "You created research-buddy.")).toBeVisible();
  await expect(boardRow(page, "research-buddy")).toBeVisible();
  expect(address(page)).toBe("/home");
  const rail = await openRail(page, isMobile);
  const railRow = rail.getByRole("button", { name: /^research-buddy\b/ });
  await expect(railRow).toBeVisible();
  // Beside the main region, the new agent's rail row takes focus.
  if (!isMobile) await expect(railRow).toBeFocused();
});

test("the rail's + opens Create agent where the user is", async ({ page, isMobile }) => {
  await page.goto("/agent/atlas");
  const rail = await openRail(page, isMobile);
  await rail.getByRole("button", { name: "Create an agent" }).click();

  const dialog = createDialog(page);
  await expect(dialog).toBeVisible();
  await dialog.getByRole("textbox", { name: "Name" }).fill("nova");
  await dialog.getByRole("button", { name: "More options" }).click();
  await dialog.getByRole("combobox", { name: "Copy model settings from" }).selectOption("scout");
  await dialog.getByRole("button", { name: "Create agent" }).click();

  await expect(dialog).toBeHidden();
  await expect(toast(page, "You created nova.")).toBeVisible();
  expect(address(page)).toBe("/agent/atlas");
  await expect(
    (await openRail(page, isMobile)).getByRole("button", { name: /^nova\b/ }),
  ).toBeVisible();
});

test("the palette's Create an agent opens the same dialog", async ({ page, isMobile }) => {
  await page.goto("/agent/scout");
  await expect(page.getByRole("heading", { name: "scout", level: 1 })).toBeVisible();
  if (isMobile) {
    await page
      .getByRole("navigation", { name: "Main" })
      .getByRole("button", { name: "Search" })
      .click();
  } else {
    await page.keyboard.press("ControlOrMeta+k");
  }
  const palette = await expectPaletteOpen(page);
  await expect(palette.getByRole("combobox")).toBeFocused();
  await page.keyboard.type("new agent");
  await palette.getByRole("option", { name: /Create an agent/ }).click();

  const dialog = createDialog(page);
  await expect(palette).toBeHidden();
  await expect(dialog.getByRole("textbox", { name: "Name" })).toBeFocused();
  await dialog.getByRole("textbox", { name: "Name" }).fill("kit");
  await page.keyboard.press("Enter");

  await expect(dialog).toBeHidden();
  await expect(toast(page, "You created kit.")).toBeVisible();
  expect(address(page)).toBe("/agent/scout");
});

test("a name the rules refuse is flagged as it is typed, and nothing is created", async ({
  page,
}) => {
  await page.goto("/home");
  await page.getByRole("button", { name: "New agent" }).click();
  const dialog = createDialog(page);
  const name = dialog.getByRole("textbox", { name: "Name" });

  await name.fill("Research!");
  await expect(
    dialog.getByText("Use letters, numbers, spaces, hyphens, and apostrophes."),
  ).toBeVisible();
  await expect(name).toHaveAttribute("aria-invalid", "true");

  await name.fill("atlas");
  await expect(dialog.getByText("You already have an agent called atlas.")).toBeVisible();
  await expectNoAxeViolations(page, { within: "[data-overlay-host]" });
  await dialog.getByRole("button", { name: "Create agent" }).click();
  await expect(name).toBeFocused();
  await expect(dialog).toBeVisible();

  await page.keyboard.press("Escape");
  await expect(dialog).toBeHidden();
  await expect(board(page).getByRole("link")).toHaveCount(4);
});

/** The state word a board row shows, in the table's column or on the phone's card. */
function stateWord(row: Locator, word: string): Locator {
  return row.getByText(word, { exact: true }).filter({ visible: true });
}

test("a row's menu starts and stops its agent", async ({ page }) => {
  await page.goto("/home");
  const drifter = boardRow(page, "drifter");
  await expect(stateWord(drifter, "Stopped")).toBeVisible();

  let menu = await openMenu(page, "drifter");
  await expect(menu.getByRole("menuitem", { name: "Stop", exact: true })).toHaveAttribute(
    "aria-disabled",
    "true",
  );
  await expectNoAxeViolations(page);
  await menu.getByRole("menuitem", { name: "Start", exact: true }).click();
  await expect(menu).toBeHidden();
  await expect(stateWord(drifter, "Running")).toBeVisible();

  menu = await openMenu(page, "drifter");
  await menu.getByRole("menuitem", { name: "Stop", exact: true }).click();
  await expect(stateWord(drifter, "Stopped")).toBeVisible();
});

test("a row's menu turns Start automatically off and on", async ({ page }) => {
  await page.goto("/home");
  const menu = await openMenu(page, "atlas");
  const autostart = menu.getByRole("menuitemcheckbox", { name: "Start automatically" });
  await expect(autostart).toHaveAttribute("aria-checked", "true");
  await autostart.click();
  await expect(autostart).toHaveAttribute("aria-checked", "false");
  await autostart.click();
  await expect(autostart).toHaveAttribute("aria-checked", "true");
});

test("a row's menu opens the agent's chat and its settings", async ({ page }) => {
  await page.goto("/home");
  await (await openMenu(page, "scout")).getByRole("menuitem", { name: "Settings" }).click();
  await expect.poll(() => address(page)).toMatch(/^\/home\?settings=scout/);
  // The modal's code loads the first time it opens, so Esc waits for the modal to be there.
  const settings = await expectSettingsOpen(page);
  await page.keyboard.press("Escape");
  await expect.poll(() => address(page)).toBe("/home");
  // The board is out of reach until the modal has gone.
  await expect(settings).toBeHidden();

  await (await openMenu(page, "scout")).getByRole("menuitem", { name: "Open chat" }).click();
  await expect.poll(() => address(page)).toBe("/agent/scout");
});

test("delete asks first, and the toast's Undo brings the agent back", async ({ page }) => {
  await page.goto("/home");
  await (await openMenu(page, "atlas")).getByRole("menuitem", { name: "Delete" }).click();

  const confirm = page.getByRole("alertdialog", { name: "Delete atlas?" });
  await expect(confirm).toBeVisible();
  await expect(confirm).toContainText("atlas stops, and its folder is removed");
  await expectNoAxeViolations(page, { within: "[data-overlay-host]" });
  await confirm.getByRole("button", { name: "Delete atlas" }).click();

  await expect(boardRow(page, "atlas")).toHaveCount(0);
  const deleted = page.getByRole("status").filter({ hasText: "You deleted atlas." });
  await deleted.getByRole("button", { name: "Undo" }).click();

  await expect(toast(page, "You restored atlas.")).toBeVisible();
  await expect(boardRow(page, "atlas")).toBeVisible();
});

test("Cancel on the delete question keeps the agent", async ({ page }) => {
  await page.goto("/home");
  await (await openMenu(page, "drifter")).getByRole("menuitem", { name: "Delete" }).click();
  await page
    .getByRole("alertdialog", { name: "Delete drifter?" })
    .getByRole("button", { name: "Cancel" })
    .click();
  await expect(page.getByRole("alertdialog")).toBeHidden();
  await expect(boardRow(page, "drifter")).toBeVisible();
});

test("Recently deleted restores an agent", async ({ page }) => {
  await page.goto("/home");
  await expect(boardRow(page, "drifter")).toBeVisible();
  await expect(page.getByRole("button", { name: "Recently deleted" })).toHaveCount(0);

  const removed = await page.request.delete("/api/hub/agents/drifter");
  expect(removed.ok()).toBe(true);
  await expect(boardRow(page, "drifter")).toHaveCount(0);

  const recent = page.getByRole("button", { name: "Recently deleted" });
  await expect(recent).toHaveAttribute("aria-expanded", "false");
  await recent.click();
  await expect(
    page.getByText("A deleted agent's files stay in its checkpoint history."),
  ).toBeVisible();
  await expectNoAxeViolations(page);
  await page.getByRole("button", { name: "Restore drifter" }).click();

  await expect(boardRow(page, "drifter")).toBeVisible();
  await expect(page.getByText("Nothing to restore.")).toBeVisible();
  await expect(recent).toBeFocused();
});
