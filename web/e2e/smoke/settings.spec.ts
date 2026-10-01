import type { Locator, Page } from "@playwright/test";
import { expectNoAxeViolations } from "../support/axe";
import { expect, test } from "../support/fixtures";

/** Legacy panels hosted in sections not rebuilt yet; their own units scan them. */
const LEGACY = "[data-legacy-view]";

function address(page: Page): string {
  const url = new URL(page.url());
  return decodeURIComponent(`${url.pathname}${url.search}`);
}

const modal = (page: Page): Locator => page.getByRole("dialog", { name: "Settings" });
const sections = (page: Page): Locator =>
  page.getByRole("navigation", { name: "Settings sections" });
const saveBar = (page: Page): Locator => page.getByRole("region", { name: "Unsaved changes" });
const timeout = (page: Page): Locator => page.getByLabel("Reply time limit");

/** Pick a section from the list; on a phone that is the list's own screen. */
async function pickSection(page: Page, name: string): Promise<void> {
  await sections(page)
    .getByRole("button", { name: new RegExp(`^${name}`) })
    .click();
  await expect(page.getByRole("heading", { name, level: 2 })).toBeVisible();
}

/** Pick a scope; on a phone the picker is on the section list, so go back to it first. */
async function pickScope(page: Page, isMobile: boolean, scope: string): Promise<void> {
  if (isMobile) await page.getByRole("button", { name: "Back to all settings" }).click();
  await sections(page).getByRole("combobox", { name: "Settings for" }).selectOption(scope);
}

/** Replace a line of atlas's config.toml on disk, the way something outside the page would. */
async function changeOnDisk(page: Page, from: RegExp, to: string): Promise<void> {
  const url = "/api/agents/atlas/config/raw";
  const text = await (await page.request.get(url)).text();
  expect(text).toMatch(from);
  expect((await page.request.put(url, { data: text.replace(from, to) })).ok()).toBe(true);
}

test("a deep link opens its section in the frame: the scope picker, the list and its Advanced heading", async ({
  page,
}) => {
  await page.goto("/agent/atlas?settings=atlas/runtime");
  await expect(modal(page)).toBeVisible();
  await expect(page.getByRole("heading", { name: "Runtime", level: 2 })).toBeVisible();
  await expect(timeout(page)).toHaveValue("120");
  await expectNoAxeViolations(page, { within: "[data-overlay-host]", exclude: LEGACY });

  await page.goto("/home?settings=_all");
  await expect(sections(page).getByRole("combobox", { name: "Settings for" })).toHaveValue("_all");
  await expect(sections(page).getByText("Applies to every agent")).toBeVisible();
  const advanced = sections(page).getByRole("group", { name: "Advanced" });
  await expect(advanced.getByRole("button")).toHaveCount(4);
  await expect(advanced.getByRole("button", { name: /^Diagnostics/ })).toBeVisible();
  await expectNoAxeViolations(page, { within: "[data-overlay-host]", exclude: LEGACY });
});

test("staged changes stay with their scope through a switch and back", async ({
  page,
  isMobile,
}) => {
  await page.goto("/agent/atlas?settings=atlas/runtime");
  await timeout(page).fill("60");
  await expect(saveBar(page)).toContainText("You have unsaved changes.");

  await pickScope(page, isMobile, "_all");
  await expect(sections(page).getByText("Applies to every agent")).toBeVisible();
  await expect(saveBar(page)).toBeHidden();
  await expect(
    sections(page).getByRole("combobox", { name: "Settings for" }).locator("option", {
      hasText: "atlas (unsaved)",
    }),
  ).toHaveCount(1);

  await sections(page).getByRole("combobox", { name: "Settings for" }).selectOption("atlas");
  await pickSection(page, "Runtime");
  await expect(timeout(page)).toHaveValue("60");
  await expect(saveBar(page)).toContainText("You have unsaved changes.");
});

test("Save writes the staged change and offers Undo; Discard brings the file's value back", async ({
  page,
}) => {
  await page.goto("/agent/atlas?settings=atlas/runtime");
  await timeout(page).fill("90");
  await saveBar(page).getByRole("button", { name: "Save changes" }).click();
  const saved = page.getByRole("status").filter({ hasText: "Saved config.toml." });
  await expect(saved).toBeVisible();
  await expect(saved.getByRole("button", { name: "Undo" })).toBeVisible();
  await expect(saveBar(page)).toBeHidden();
  await page.reload();
  await expect(timeout(page)).toHaveValue("90");

  await timeout(page).fill("30");
  await saveBar(page).getByRole("button", { name: "Discard" }).click();
  await expect(timeout(page)).toHaveValue("90");
  await expect(saveBar(page)).toBeHidden();
});

test("a save the server refuses shows why in the section and keeps the change staged", async ({
  page,
}) => {
  await page.goto("/agent/atlas?settings=atlas/runtime");
  await page.getByLabel("Tool calls per turn").fill("0");
  await saveBar(page).getByRole("button", { name: "Save changes" }).click();

  const why = "agent.max_tool_iterations must be at least 1 (leave it unset for unlimited)";
  await expect(saveBar(page)).toContainText(`Couldn't save config.toml: ${why}.`);
  await expect(modal(page).getByRole("alert").filter({ hasText: why }).first()).toBeVisible();
  await expect(page.getByLabel("Tool calls per turn")).toHaveValue("0");
  await expectNoAxeViolations(page, { within: "[data-overlay-host]", exclude: LEGACY });
});

test("the frame stays mounted while sections and scopes switch", async ({ page, isMobile }) => {
  await page.goto("/agent/atlas?settings=atlas/runtime");
  const dialog = await modal(page).elementHandle();
  await dialog.evaluate((element) => {
    element.setAttribute("data-probe", "first");
  });

  if (isMobile) await page.getByRole("button", { name: "Back to all settings" }).click();
  await pickSection(page, "Memory");
  await pickScope(page, isMobile, "scout");
  await pickSection(page, "Memory");
  await pickScope(page, isMobile, "_all");
  await pickSection(page, "General");

  expect(await dialog.evaluate((element) => element.isConnected)).toBe(true);
  await expect(modal(page)).toHaveAttribute("data-probe", "first");
});

test("on a phone the section list is its own screen, and Back returns to it", async ({
  page,
  isMobile,
}) => {
  test.skip(!isMobile, "The list and the section sit side by side on wider screens.");
  await page.goto("/agent/atlas");
  const bar = page.getByRole("navigation", { name: "Main" });
  await bar.getByRole("button", { name: "Settings" }).click();
  await expect.poll(() => address(page)).toBe("/agent/atlas?settings=atlas");
  await expect(sections(page)).toBeVisible();
  await expect(page.getByRole("heading", { name: "Model", level: 2 })).toBeHidden();
  // The bar stays on screen beside the modal.
  await expect(bar).toBeVisible();
  await expectNoAxeViolations(page, { within: "[data-overlay-host]" });

  await sections(page)
    .getByRole("button", { name: /^Runtime/ })
    .click();
  await expect.poll(() => address(page)).toBe("/agent/atlas?settings=atlas/runtime");
  const back = page.getByRole("button", { name: "Back to all settings" });
  await expect(back).toBeFocused();
  await expect(sections(page)).toBeHidden();

  await back.click();
  await expect.poll(() => address(page)).toBe("/agent/atlas?settings=atlas");
  await expect(sections(page).getByRole("button", { name: /^Runtime/ })).toBeFocused();

  // Back closes the modal: the section's entry was popped, not pushed over.
  await page.goBack();
  await expect.poll(() => address(page)).toBe("/agent/atlas");
  await expect(modal(page)).toBeHidden();
});

test("a file changed on disk under the change asks which to keep", async ({ page }) => {
  await page.goto("/agent/atlas?settings=atlas/runtime");
  await timeout(page).fill("60");
  await changeOnDisk(page, /timeout_secs = \d+/, "timeout_secs = 45");

  await saveBar(page).getByRole("button", { name: "Save changes" }).click();
  const question = page.getByRole("dialog", { name: "atlas's config.toml changed" });
  await expect(question).toBeVisible();
  await expect(question).toContainText("timeout_secs changed on disk after you started editing.");
  await expectNoAxeViolations(page, { within: "[data-overlay-host]", exclude: LEGACY });
  await question.getByRole("button", { name: "Keep my changes" }).click();
  await expect(page.getByRole("status").filter({ hasText: "Saved config.toml." })).toBeVisible();
  expect(await (await page.request.get("/api/agents/atlas/config/raw")).text()).toContain(
    "timeout_secs = 60",
  );

  await timeout(page).fill("75");
  await changeOnDisk(page, /timeout_secs = \d+/, "timeout_secs = 50");
  await saveBar(page).getByRole("button", { name: "Save changes" }).click();
  await question.getByRole("button", { name: "Use what's on disk" }).click();
  await expect(timeout(page)).toHaveValue("50");
  await expect(saveBar(page)).toBeHidden();
});
