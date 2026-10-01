import type { Locator, Page } from "@playwright/test";
import { parse as parseToml } from "smol-toml";
import { expectNoAxeViolations } from "../support/axe";
import { expect, test } from "../support/fixtures";

/**
 * The agent's Model section: Fix settings for brittle's model that its
 * provider doesn't offer, through Save and a restart from its state card;
 * adding and removing a provider; a job's own model; and thinking for every
 * model. Each is read back from the file on disk.
 */

const overlay = { within: "[data-overlay-host]" } as const;
const saveBar = (page: Page): Locator => page.getByRole("region", { name: "Unsaved changes" });
const mainModel = (page: Page): Locator => page.getByRole("region", { name: "Main model" });
const jobsToggle = (page: Page): Locator =>
  page.getByRole("button", { name: "Use different models for specific jobs" });

async function fileText(page: Page, agent: string, file: "providers" | "config"): Promise<string> {
  return (await page.request.get(`/api/agents/${agent}/${file}/raw`)).text();
}

async function saveChanges(page: Page, file: "providers.toml" | "config.toml"): Promise<void> {
  await saveBar(page).getByRole("button", { name: "Save changes" }).click();
  await expect(page.getByRole("status").filter({ hasText: `Saved ${file}.` })).toBeVisible();
  await expect(saveBar(page)).toBeHidden();
}

test("Fix settings flags brittle's model, and after Save a restart from its card runs it", async ({
  page,
}) => {
  await page.goto("/agent/brittle");
  const failed = page.getByRole("region", { name: "brittle couldn't start" });
  await failed.getByRole("button", { name: "Fix settings" }).click();

  await expect(page).toHaveURL(/\/agent\/brittle\?settings=brittle\/model$/);
  const model = mainModel(page).getByRole("combobox", { name: "Model" });
  await expect(model).toBeFocused();
  await expect(model).toHaveAttribute("aria-invalid", "true");
  await expect(
    mainModel(page).getByText("model 'gpt-9' is not offered by provider 'openai'"),
  ).toBeVisible();
  await expect(model).toHaveValue("gpt-9");
  await expectNoAxeViolations(page, overlay);

  await model.selectOption("gpt-4o");
  await saveChanges(page, "providers.toml");
  await expect(model).not.toHaveAttribute("aria-invalid");
  expect(await fileText(page, "brittle", "providers")).toContain('main = "openai/gpt-4o"');
  await expect(page.getByText("Saved. brittle is still stopped.")).toBeVisible();

  await page.getByRole("button", { name: "Close settings" }).click();
  await failed.getByRole("button", { name: "Restart brittle" }).click();
  await expect(failed).toHaveCount(0);
  await expect(page.getByRole("textbox", { name: "Send a message..." })).toBeVisible();
});

test("a provider is added with its key stored, then removed", async ({ page }) => {
  await page.goto("/agent/atlas?settings=atlas/model");
  await page.getByRole("button", { name: "Add a provider" }).click();
  const form = page.getByRole("form", { name: "Add a provider" });
  await form.getByLabel("Name").fill("work");
  await form.getByLabel("Type").selectOption("openai");
  await form.getByLabel("API key", { exact: true }).fill("sk-test");
  await expectNoAxeViolations(page, overlay);
  await form.getByRole("button", { name: "Add provider" }).click();

  const providers = page.getByRole("list", { name: "Providers" });
  await expect(providers.getByText("work", { exact: true })).toBeVisible();
  await expect(
    mainModel(page).getByRole("combobox", { name: "Provider" }).locator("option", {
      hasText: "work (OpenAI)",
    }),
  ).toHaveCount(1);
  await saveChanges(page, "providers.toml");
  expect(parseToml(await fileText(page, "atlas", "providers"))).toMatchObject({
    providers: { work: { type: "openai", api_key: "secret:work" } },
  });
  await expect(providers).toContainText("key stored securely");

  await page.getByRole("button", { name: "Remove work" }).click();
  await expect(
    page.getByRole("status").filter({ hasText: "Removed work. Save changes to keep it removed." }),
  ).toBeVisible();
  await expect(providers).toHaveCount(0);
  await saveChanges(page, "providers.toml");
  expect(await fileText(page, "atlas", "providers")).not.toContain("[providers.work]");
});

test("a job's own model is saved with its thinking, and reads back from disk", async ({ page }) => {
  await page.goto("/agent/atlas?settings=atlas/model");
  await expect(jobsToggle(page)).toHaveAttribute("aria-expanded", "false");
  await jobsToggle(page).click();

  const summarizing = page.getByRole("group", { name: "Summarizing older messages" });
  await expect(summarizing.getByRole("combobox", { name: "Provider" })).toHaveValue("");
  await summarizing.getByRole("combobox", { name: "Provider" }).selectOption("openai");
  const model = summarizing.getByRole("combobox", { name: "Model" });
  await expect(model.locator("option", { hasText: "o3" })).toHaveCount(1);
  await model.selectOption("o3");
  await summarizing.getByRole("radio", { name: "Low" }).click();
  await expectNoAxeViolations(page, overlay);
  await saveChanges(page, "providers.toml");

  expect(parseToml(await fileText(page, "atlas", "providers"))).toMatchObject({
    models: { observer: { model: "openai/o3", thinking: "low" } },
  });
  await page.getByRole("button", { name: "Reload from disk" }).click();
  await expect(jobsToggle(page)).toHaveAttribute("aria-expanded", "true");
  await expect(model).toHaveValue("o3");
  await expect(summarizing.getByRole("radio", { name: "Low" })).toHaveAttribute(
    "aria-checked",
    "true",
  );
});

test("thinking for every model is saved to config.toml", async ({ page }) => {
  await page.goto("/agent/atlas?settings=atlas/model");
  const every = page.getByRole("region", { name: "Every model" });
  await every.getByRole("radio", { name: "Medium" }).click();
  await every.getByLabel("Temperature").fill("0.4");
  await saveChanges(page, "config.toml");

  const text = await fileText(page, "atlas", "config");
  expect(text).toContain('thinking = "medium"');
  expect(text).toContain("temperature = 0.4");
  await expect(
    mainModel(page).getByText("Default is medium, as set under Every model."),
  ).toBeVisible();
});
