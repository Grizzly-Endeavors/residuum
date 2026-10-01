import type { Page } from "@playwright/test";
import { expect, test } from "../support/fixtures";
import { expectScreenshot } from "../support/screenshot";

/**
 * The Settings modal's frame: a section with staged changes and its save bar,
 * the Raw config section, and on a phone the section list. Legacy panels
 * hosted in sections not rebuilt yet are hidden, since their own units give
 * them baselines; hiding rather than masking keeps the save bar over them.
 */

async function frameScreenshot(page: Page, name: string): Promise<void> {
  await page.addStyleTag({
    content: "[data-overlay-host] [data-legacy-view] { visibility: hidden; }",
  });
  await expectScreenshot(page, name);
}

test.describe("settings modal", { tag: "@visual" }, () => {
  test("a section with staged changes and the save bar", async ({ page }) => {
    await page.goto("/agent/atlas?settings=atlas/runtime");
    await page.getByLabel("Timeout (seconds)").fill("60");
    await expect(page.getByRole("region", { name: "Unsaved changes" })).toBeVisible();
    await page.getByLabel("Timeout (seconds)").blur();
    await frameScreenshot(page, "settings-section");
  });

  test("the install-wide Raw config", async ({ page }) => {
    await page.goto("/home?settings=_all/raw");
    await expect(page.getByRole("textbox", { name: "Contents of config.toml" })).toHaveValue(/\S/);
    await frameScreenshot(page, "settings-raw");
  });

  test("an agent's Raw config with a problem", async ({ page }) => {
    await page.goto("/agent/atlas?settings=atlas/raw");
    const text = page.getByRole("textbox", { name: "Contents of config.toml" });
    await text.fill(`${await text.inputValue()}\nbroken = \n`);
    await page.getByRole("button", { name: /^line \d+/ }).click();
    await text.blur();
    await frameScreenshot(page, "settings-raw-problem");
  });

  test("History with a checkpoint open", async ({ page }) => {
    await page.goto("/agent/atlas?settings=atlas/history");
    await page.getByRole("button", { name: /^updated SOUL\.md/ }).click();
    await page.getByRole("button", { name: "Changes to SOUL.md" }).click();
    await expect(page.getByRole("region", { name: "Changes to SOUL.md" })).toBeVisible();
    await page.getByRole("button", { name: "Changes to SOUL.md" }).blur();
    await frameScreenshot(page, "settings-history");
  });

  test("the phone's section list", async ({ page, isMobile }) => {
    test.skip(!isMobile, "Wider screens show the list beside a section.");
    await page.goto("/agent/atlas?settings=atlas");
    await expect(page.getByRole("navigation", { name: "Settings sections" })).toBeVisible();
    await frameScreenshot(page, "settings-list");
  });
});

/** The rebuilt All agents sections hold nothing legacy, so the whole modal is compared. */
test.describe("settings modal: All agents sections", { tag: "@visual" }, () => {
  test("General, with the timezone and the gateway options open", async ({ page }) => {
    await page.goto("/home?settings=_all/general");
    await expect(page.getByLabel("Timezone")).toHaveValue("America/New_York");
    await page.getByRole("button", { name: "More options" }).click();
    await expect(page.getByLabel("Bind address")).toBeVisible();
    await expectScreenshot(page, "settings-general");
  });

  test("Residuum Cloud, connected", async ({ page, mock }) => {
    await mock.post("/api/mock/cloud-callback");
    await page.goto("/home?settings=_all/cloud");
    await expect(page.getByText("Connected", { exact: true })).toBeVisible();
    await expectScreenshot(page, "settings-cloud");
  });

  test("Residuum Cloud, not connected, with the relay options open", async ({ page }) => {
    await page.goto("/home?settings=_all/cloud");
    await expect(page.getByText("Not connected")).toBeVisible();
    await page.getByRole("button", { name: "More options" }).click();
    await expect(page.getByLabel("Relay URL")).toBeVisible();
    await expectScreenshot(page, "settings-cloud-not-connected");
  });

  test("Updates, after a check", async ({ page }) => {
    await page.goto("/home?settings=_all/updates");
    await page.getByRole("button", { name: "Check for updates" }).click();
    await expect(page.getByText("Up to date")).toBeVisible();
    await expectScreenshot(page, "settings-updates");
  });

  test("Session limits, with a value that blocks work", async ({ page }) => {
    await page.goto("/home?settings=_all/limits");
    await page.getByLabel("Turns at once").fill("0");
    await expect(page.getByText(/can never run a turn/)).toBeVisible();
    await page.getByLabel("Turns at once").blur();
    await expectScreenshot(page, "settings-limits");
  });

  test("Diagnostics", async ({ page }) => {
    await page.goto("/home?settings=_all/diagnostics");
    await expect(
      page.getByRole("switch", { name: "Redact content in trace exports" }),
    ).toBeChecked();
    await expectScreenshot(page, "settings-diagnostics");
  });
});
