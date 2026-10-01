import type { Page } from "@playwright/test";
import { expect, test } from "../support/fixtures";
import { expectScreenshot } from "../support/screenshot";

/**
 * The Settings modal's frame: a section with staged changes and its save bar,
 * the Raw config section, and on a phone the section list; and the Memory,
 * Schedule and Runtime sections. Legacy panels hosted in sections not rebuilt
 * yet are hidden, since their own units give them baselines; hiding rather
 * than masking keeps the save bar over them.
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
    await page.getByLabel("Reply time limit").fill("60");
    await expect(page.getByRole("region", { name: "Unsaved changes" })).toBeVisible();
    await page.getByLabel("Reply time limit").blur();
    await frameScreenshot(page, "settings-section");
  });

  test("the install-wide Raw config", async ({ page }) => {
    await page.goto("/home?settings=_all/raw");
    await expect(page.getByRole("textbox", { name: "Contents of config.toml" })).toHaveValue(/\S/);
    await expect(page.getByText("No problems found.")).toBeVisible();
    await frameScreenshot(page, "settings-raw");
  });

  test("Memory with the reviewing switch on", async ({ page }) => {
    await page.goto("/agent/atlas?settings=atlas/memory");
    await page.getByRole("switch", { name: "Review replies" }).click();
    await expect(page.getByLabel("Look in every")).toBeEnabled();
    await expectScreenshot(page, "settings-memory");
  });

  test("Memory's search tuning under More options", async ({ page }) => {
    await page.goto("/agent/atlas?settings=atlas/memory");
    await page.getByRole("button", { name: "More options" }).click();
    await expect(page.getByLabel("Weight of meaning")).toBeVisible();
    await page.getByLabel("Memories lose half their rank after").scrollIntoViewIfNeeded();
    await expectScreenshot(page, "settings-memory-more");
  });

  test("Schedule", async ({ page }) => {
    await page.goto("/agent/atlas?settings=atlas/schedule");
    await expect(page.getByRole("heading", { name: "Schedule", level: 2 })).toBeVisible();
    await expectScreenshot(page, "settings-schedule");
  });

  test("Runtime's stopping rules and idle settings", async ({ page }) => {
    await page.goto("/agent/atlas?settings=atlas/runtime");
    await page.getByRole("switch", { name: "Catch repeated tool calls" }).click();
    await page.getByLabel("Send its updates to").scrollIntoViewIfNeeded();
    await expectScreenshot(page, "settings-runtime");
  });

  test("an agent's Raw config with a problem", async ({ page }) => {
    await page.goto("/agent/atlas?settings=atlas/raw");
    const text = page.getByRole("textbox", { name: "Contents of config.toml" });
    await text.fill(`${await text.inputValue()}\nbroken = \n`);
    await page.getByRole("button", { name: /^line \d+/ }).click();
    await text.blur();
    await page.mouse.move(0, 0);
    await frameScreenshot(page, "settings-raw-problem");
  });

  test("History with a checkpoint open", async ({ page }) => {
    await page.goto("/agent/atlas?settings=atlas/history");
    await page.getByRole("button", { name: /^updated SOUL\.md/ }).click();
    await page.getByRole("button", { name: "Changes to SOUL.md" }).click();
    await expect(page.getByRole("region", { name: "Changes to SOUL.md" })).toBeVisible();
    await page.getByRole("button", { name: "Changes to SOUL.md" }).blur();
    await page.mouse.move(0, 0);
    await frameScreenshot(page, "settings-history");
  });

  test("the phone's section list", async ({ page, isMobile }) => {
    test.skip(!isMobile, "Wider screens show the list beside a section.");
    await page.goto("/agent/atlas?settings=atlas");
    await expect(page.getByRole("navigation", { name: "Settings sections" })).toBeVisible();
    await frameScreenshot(page, "settings-list");
  });
});

/** The agent's Model section, compared whole: it holds nothing legacy. */
test.describe("settings modal: Model", { tag: "@visual" }, () => {
  test("Model as Fix settings opens it, with brittle's model flagged", async ({ page }) => {
    await page.goto("/agent/brittle");
    await page.getByRole("button", { name: "Fix settings" }).click();
    const model = page
      .getByRole("region", { name: "Main model" })
      .getByRole("combobox", { name: "Model" });
    await expect(model).toBeFocused();
    await expect(model.locator("option", { hasText: "gpt-9 (not in the list)" })).toHaveCount(1);
    await expectScreenshot(page, "settings-model-fix");
  });

  test("Model with a job's own model and a provider open", async ({ page }) => {
    await page.goto("/agent/atlas?settings=atlas/model");
    // The scope picker names each agent's state once the hub socket has said it.
    await expect(
      page.locator("[data-scope-picker] option", { hasText: "brittle (couldn't start)" }),
    ).toHaveCount(1);
    await page.getByRole("button", { name: "Add a provider" }).click();
    const form = page.getByRole("form", { name: "Add a provider" });
    await form.getByLabel("Name").fill("work");
    await form.getByLabel("Type").selectOption("openai");
    await form.getByRole("button", { name: "Add provider" }).click();
    await page.getByRole("button", { name: "Edit work" }).click();
    await page.getByRole("button", { name: "Use different models for specific jobs" }).click();
    const summarizing = page.getByRole("group", { name: "Summarizing older messages" });
    await summarizing.getByRole("combobox", { name: "Provider" }).selectOption("work");
    await expect(
      summarizing.getByRole("combobox", { name: "Model" }).locator("option", { hasText: "o3" }),
    ).toHaveCount(1);
    await summarizing.getByRole("combobox", { name: "Provider" }).scrollIntoViewIfNeeded();
    await summarizing.getByRole("combobox", { name: "Provider" }).blur();
    await expectScreenshot(page, "settings-model-jobs");
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
