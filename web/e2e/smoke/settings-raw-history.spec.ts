import type { Locator, Page } from "@playwright/test";
import { expectNoAxeViolations } from "../support/axe";
import { expect, test } from "../support/fixtures";

/** Legacy panels hosted in sections not rebuilt yet; their own units scan them. */
const LEGACY = "[data-legacy-view]";
const HOST = "[data-overlay-host]";

const sections = (page: Page): Locator =>
  page.getByRole("navigation", { name: "Settings sections" });
const rawText = (page: Page): Locator =>
  page.getByRole("textbox", { name: "Contents of config.toml" });
const timeout = (page: Page): Locator => page.getByLabel("Reply time limit");
const saveBar = (page: Page): Locator => page.getByRole("region", { name: "Unsaved changes" });

/** Move to another section in the page, so nothing staged is lost; on a phone through the list. */
async function goToSection(page: Page, isMobile: boolean, name: string): Promise<void> {
  if (isMobile) await page.getByRole("button", { name: "Back to all settings" }).click();
  await sections(page)
    .getByRole("button", { name: new RegExp(`^${name}`) })
    .click();
  await expect(page.getByRole("heading", { name, level: 2 })).toBeVisible();
}

test.describe("Raw config", () => {
  test("an edit is checked as it is typed, and its problem shows at its line", async ({ page }) => {
    await page.goto("/agent/atlas?settings=atlas/raw");
    await expect(page.getByText("No problems found.")).toBeVisible();
    const text = await rawText(page).inputValue();
    const line = text.split("\n").length + 1;
    await rawText(page).fill(`${text}\nbroken = \n`);

    await expect(page.getByText("1 problem found.")).toBeVisible();
    const problems = page.getByRole("list", { name: "Problems in config.toml" });
    await expect(problems).toContainText(`line ${String(line)}, column 10`);
    await expect(problems).toContainText("invalid value");
    await expect(page.getByRole("tab", { name: "config.toml (unsaved)" })).toBeVisible();
    await expectNoAxeViolations(page, { within: HOST });

    await page.getByRole("button", { name: "Save config.toml" }).click();
    await expect(
      page.getByRole("status").filter({ hasText: "Saved config.toml. It has problems" }),
    ).toBeVisible();
    expect(await (await page.request.get("/api/agents/atlas/config/raw")).text()).toContain(
      "broken = ",
    );
    await expect(page.getByText("1 problem found.")).toBeVisible();
  });

  test("is read-only while the form has staged changes, and the form while it has edits", async ({
    page,
    isMobile,
  }) => {
    await page.goto("/agent/atlas?settings=atlas/runtime");
    await timeout(page).fill("60");
    await goToSection(page, isMobile, "Raw config");
    await expect(page.getByText("Save or discard your form changes first.")).toBeVisible();
    await expect(rawText(page)).not.toBeEditable();
    await saveBar(page).getByRole("button", { name: "Discard" }).click();
    await expect(rawText(page)).toBeEditable();

    await rawText(page).fill("timeout_secs = 45\n");
    await goToSection(page, isMobile, "Runtime");
    await expect(page.getByText(/unsaved edits to config\.toml in Raw config/)).toBeVisible();
    await expect(timeout(page)).toBeDisabled();
    await expectNoAxeViolations(page, { within: HOST, exclude: LEGACY });

    await page.getByRole("button", { name: "Open Raw config" }).click();
    await page.getByRole("button", { name: "Discard edits" }).click();
    await goToSection(page, isMobile, "Runtime");
    await expect(timeout(page)).toBeEnabled();
    await expect(timeout(page)).toHaveValue("120");
  });
});

test.describe("History", () => {
  test("restoring a file from a checkpoint updates the settings that show it", async ({
    page,
    isMobile,
  }) => {
    await page.goto("/agent/atlas?settings=atlas/runtime");
    await timeout(page).fill("90");
    await saveBar(page).getByRole("button", { name: "Save changes" }).click();
    await expect(page.getByRole("status").filter({ hasText: "Saved config.toml." })).toBeVisible();

    await goToSection(page, isMobile, "History");
    await page.getByRole("radio", { name: "Config files" }).click();
    // The save before last left config.toml as it is before this one: 120.
    await page
      .getByRole("button", { name: /^config patch.*1 file/ })
      .first()
      .click();
    await page.getByRole("button", { name: "Changes to config.toml" }).click();
    await expect(page.getByRole("region", { name: "Changes to config.toml" })).toBeVisible();
    await expectNoAxeViolations(page, { within: HOST });

    await page.getByRole("button", { name: "Restore config.toml as it was then" }).click();
    await expect(
      page.getByRole("status").filter({ hasText: "Restored config.toml." }),
    ).toBeVisible();
    await expect(page.getByRole("button", { name: /^restored config\.toml/ })).toBeVisible();

    await goToSection(page, isMobile, "Runtime");
    await expect(timeout(page)).toHaveValue("120");
  });

  test("undoing a checkpoint puts back what it changed and says so", async ({ page }) => {
    await page.goto("/agent/atlas?settings=atlas/history");
    const soulBefore = await (
      await page.request.get("/api/agents/atlas/workspace/file?path=SOUL.md")
    ).text();
    await page.getByRole("button", { name: /^updated SOUL\.md/ }).click();
    await page.getByRole("button", { name: "Undo these changes" }).click();

    await expect(page.getByText("Put back SOUL.md.")).toBeVisible();
    await expect(page.getByRole("button", { name: /^undid checkpoint/ })).toBeVisible();
    const soulAfter = await (
      await page.request.get("/api/agents/atlas/workspace/file?path=SOUL.md")
    ).text();
    expect(soulAfter).not.toBe(soulBefore);
    await expectNoAxeViolations(page, { within: HOST });
  });

  test("works for a stopped agent", async ({ page }) => {
    await page.goto("/agent/drifter?settings=drifter/history");
    await expect(page.getByRole("radio", { name: "Workspace" })).toBeChecked();
    await expect(page.getByRole("list", { name: "Workspace checkpoints" })).toBeVisible();
  });
});
