import type { Locator, Page } from "@playwright/test";
import { expectNoAxeViolations } from "../support/axe";
import { expect, test } from "../support/fixtures";

/**
 * The agent's Memory, Schedule and Runtime sections: each one's field changed,
 * saved, read back through Reload from disk, and scanned at both sizes.
 */

const overlay = { within: "[data-overlay-host]" } as const;
const saveBar = (page: Page): Locator => page.getByRole("region", { name: "Unsaved changes" });
const saved = (page: Page): Locator =>
  page.getByRole("status").filter({ hasText: "Saved config.toml." });

async function configText(page: Page, agent = "atlas"): Promise<string> {
  return (await page.request.get(`/api/agents/${agent}/config/raw`)).text();
}

async function saveChanges(page: Page): Promise<void> {
  await saveBar(page).getByRole("button", { name: "Save changes" }).click();
  await expect(saved(page)).toBeVisible();
  await expect(saveBar(page)).toBeHidden();
}

async function reloadFromDisk(page: Page): Promise<void> {
  await page.getByRole("button", { name: "Reload from disk" }).click();
}

test("Memory: a threshold is saved, read back from disk, and the reviewing settings follow their switch", async ({
  page,
}) => {
  await page.goto("/agent/atlas?settings=atlas/memory");
  await expect(page.getByRole("heading", { name: "Memory", level: 2 })).toBeVisible();
  await expect(page.getByLabel("Start summarizing at")).toHaveValue("30000");
  await expect(page.getByText("tokens").first()).toBeVisible();

  // The reviewing settings sit dimmed until Review replies is on.
  await expect(page.getByLabel("Look in every")).toBeDisabled();
  await page.getByRole("switch", { name: "Review replies" }).click();
  await expect(page.getByLabel("Look in every")).toBeEnabled();
  await expectNoAxeViolations(page, overlay);

  await page.getByRole("button", { name: "More options" }).click();
  await expect(page.getByLabel("Weight of meaning")).toBeVisible();
  await expectNoAxeViolations(page, overlay);

  await page.getByLabel("Start summarizing at").fill("45000");
  await saveChanges(page);
  await reloadFromDisk(page);

  await expect(page.getByLabel("Start summarizing at")).toHaveValue("45000");
  await expect(page.getByRole("switch", { name: "Review replies" })).toHaveAttribute(
    "aria-checked",
    "true",
  );
  const text = await configText(page);
  expect(text).toContain("observer_threshold_tokens = 45000");
  expect(text).toMatch(/\[subconscious\][^[]*enabled = true/);
});

test("Memory: Choose the model goes to the Model section", async ({ page }) => {
  await page.goto("/agent/atlas?settings=atlas/memory");
  await page.getByRole("button", { name: "Choose the model that reviews replies" }).click();
  await expect(page.getByRole("heading", { name: "Model", level: 2 })).toBeVisible();
});

test("Schedule: pulses are switched off, saved and read back from disk", async ({ page }) => {
  await page.goto("/agent/atlas?settings=atlas/schedule");
  await expect(page.getByRole("heading", { name: "Schedule", level: 2 })).toBeVisible();
  await expect(page.getByLabel("Helper sessions")).toHaveAttribute("placeholder", "10");
  await expectNoAxeViolations(page, overlay);

  const pulses = page.getByRole("switch", { name: "Run pulses" });
  await expect(pulses).toHaveAttribute("aria-checked", "true");
  await pulses.click();
  await page.getByLabel("Pulses, scheduled actions and webhooks").fill("5");
  await saveChanges(page);
  await reloadFromDisk(page);

  await expect(pulses).toHaveAttribute("aria-checked", "false");
  await expect(page.getByLabel("Pulses, scheduled actions and webhooks")).toHaveValue("5");
  const text = await configText(page);
  expect(text).toMatch(/\[pulse\][^[]*enabled = false/);
  expect(text).toContain("idle_timeout_scheduled_minutes = 5");
});

test("Schedule: Open the agent's Schedule leaves settings for the place", async ({ page }) => {
  await page.goto("/agent/atlas?settings=atlas/schedule");
  await page.getByRole("button", { name: "Open atlas's Schedule" }).click();
  await expect(page.getByRole("dialog", { name: "Settings" })).toBeHidden();
  await expect(page).toHaveURL(/\/agent\/atlas\/schedule$/);
});

test("Runtime: a limit and the idle channel are saved and read back from disk", async ({
  page,
}) => {
  await page.goto("/agent/atlas?settings=atlas/runtime");
  await expect(page.getByRole("heading", { name: "Runtime", level: 2 })).toBeVisible();
  await expect(page.getByLabel("Reply time limit")).toHaveValue("120");
  await expect(page.getByLabel("Reply length")).toHaveValue("8192");
  await expectNoAxeViolations(page, overlay);

  await page.getByLabel("Reply length").fill("4096");
  await page.getByLabel("Tool calls per turn").fill("40");
  await page.getByLabel("Send its updates to").selectOption("telegram");
  await saveChanges(page);
  await reloadFromDisk(page);

  await expect(page.getByLabel("Reply length")).toHaveValue("4096");
  await expect(page.getByLabel("Tool calls per turn")).toHaveValue("40");
  await expect(page.getByLabel("Send its updates to")).toHaveValue("telegram");
  const text = await configText(page);
  expect(text).toContain("max_tokens = 4096");
  expect(text).toContain("max_tool_iterations = 40");
  expect(text).toContain('idle_channel = "telegram"');
});

test("Runtime: a refused save names the problem and keeps the number staged", async ({ page }) => {
  await page.goto("/agent/atlas?settings=atlas/runtime");
  await page.getByLabel("Tool calls per turn").fill("0");
  await saveBar(page).getByRole("button", { name: "Save changes" }).click();

  await expect(saveBar(page)).toContainText("agent.max_tool_iterations must be at least 1");
  await expect(page.getByLabel("Tool calls per turn")).toHaveValue("0");
  await expectNoAxeViolations(page, overlay);
});

test("a stopped agent's Memory, Schedule and Runtime stay editable, with nothing asking to start it", async ({
  page,
}) => {
  for (const section of ["memory", "schedule", "runtime"]) {
    await page.goto(`/agent/drifter?settings=drifter/${section}`);
    await expect(page.getByRole("heading", { level: 2 }).first()).toBeVisible();
    await expect(page.getByRole("button", { name: /^Start drifter/ })).toHaveCount(0);
  }
  await page.getByLabel("Reply length").fill("2048");
  await saveChanges(page);
  expect(await configText(page, "drifter")).toContain("max_tokens = 2048");
});
