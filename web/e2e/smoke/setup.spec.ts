import type { Locator, Page } from "@playwright/test";
import { expectNoAxeViolations } from "../support/axe";
import { expect, test } from "../support/fixtures";
import { expectSetupOpen } from "../support/lazy";

const DRAFT_KEY = "residuum-setup-draft";

/** The step's heading, which takes focus each time a step opens. */
function stepHeading(page: Page, name: string): Locator {
  return page.getByRole("heading", { level: 1, name });
}

async function next(page: Page, heading: string): Promise<void> {
  await page.getByRole("button", { name: "Next" }).click();
  await expect(stepHeading(page, heading)).toBeFocused();
}

test.beforeEach(async ({ page, mock }) => {
  // A hub with no agents opens the setup wizard.
  await mock.post("/api/mock/reset", { data: { setup: true } });
  await page.goto("/");
  await expectSetupOpen(page);
});

test("completes the wizard and opens the app on the new agent", async ({ page }) => {
  const steps = page.getByRole("list", { name: "Setup steps" });
  await expect(steps.locator("[aria-current=step]")).toHaveText("Welcome");
  await expect(page.getByLabel("Time zone")).toHaveValue("America/New_York");
  await expectNoAxeViolations(page);

  await page.getByLabel("Your name").fill("Ada");
  await page.getByLabel("Agent name").fill("Scout");
  await expect(page.getByLabel("Agent name")).toHaveAccessibleDescription(
    /Use only lowercase letters, digits, and hyphens\./,
  );
  await expect(page.getByRole("button", { name: "Next" })).toBeDisabled();
  await page.getByLabel("Agent name").fill("scout");
  await next(page, "Add model providers");

  await page.getByRole("switch", { name: "OpenAI" }).click();
  await page.getByLabel("OpenAI API key", { exact: true }).fill("sk-test-openai");
  await page.getByLabel("Anthropic API key", { exact: true }).fill("sk-test-anthropic");
  await expectNoAxeViolations(page);
  await next(page, "Assign models");

  const observer = page.getByRole("group", { name: "Observer" });
  await expect(observer.getByLabel("Model")).toHaveValue("claude-sonnet-4-6");
  await expect(page.getByRole("region", { name: "Memory search" })).toBeVisible();
  await expectNoAxeViolations(page);
  await next(page, "Add tool servers");

  await page.getByRole("button", { name: "Add fetch" }).click();
  await expect(page.getByRole("button", { name: "Remove fetch" })).toBeVisible();
  await expectNoAxeViolations(page);
  await next(page, "Connect chat apps");

  const teams = page.getByRole("region", { name: "Microsoft Teams" });
  await teams.getByLabel("App ID").fill("app-1");
  await page.getByRole("button", { name: "Next" }).click();
  // The toast region's alert group is on the page too, empty until an error toast comes.
  await expect(
    page.getByRole("alert").filter({ hasText: /Fill in all three Teams fields/ }),
  ).toBeVisible();
  await expectNoAxeViolations(page);
  await teams.getByLabel("App ID").fill("");
  await next(page, "Save and start");

  const summary = page.locator("dl");
  await expect(summary).toContainText("Ada");
  await expect(summary).toContainText("Anthropic, OpenAI");
  await expect(summary).toContainText("fetch");
  await expectNoAxeViolations(page);

  await page.getByRole("button", { name: "Save and start" }).click();
  await expect(page.getByText("Saved. Starting scout…")).toBeVisible();

  // The wizard hands over to the app, which now has the agent setup created.
  await expect(stepHeading(page, "Save and start")).toBeHidden();
  expect(await page.evaluate((key) => localStorage.getItem(key), DRAFT_KEY)).toBeNull();
  const agents = (await (await page.request.get("/api/hub/agents")).json()) as {
    agents: { name: string }[];
  };
  expect(agents.agents.map((agent) => agent.name)).toEqual(["scout"]);
});

test("a reload mid-way restores the draft, without its keys", async ({ page }) => {
  await page.getByLabel("Your name").fill("Ada");
  await page.getByLabel("Agent name").fill("night-owl");
  await next(page, "Add model providers");
  await page.getByRole("switch", { name: "Ollama" }).click();
  await page.getByLabel("Anthropic API key", { exact: true }).fill("sk-test-anthropic");

  // The draft is written half a second after the last change.
  await expect
    .poll(() => page.evaluate((key) => localStorage.getItem(key) ?? "", DRAFT_KEY))
    .toContain('"ollama"');
  await page.reload();

  await expect(stepHeading(page, "Add model providers")).toBeVisible();
  await expect(page.getByText(/Keys and tokens aren't kept in the draft/)).toBeVisible();
  await expect(page.getByRole("switch", { name: "Ollama" })).toBeChecked();
  await expect(page.getByLabel("Anthropic API key", { exact: true })).toHaveValue("");
  await expectNoAxeViolations(page);

  await page.getByRole("button", { name: "Dismiss" }).click();
  await expect(page.getByText(/Keys and tokens aren't kept in the draft/)).toBeHidden();

  await page.getByRole("button", { name: "Back" }).click();
  await expect(stepHeading(page, "Welcome to Residuum")).toBeFocused();
  await expect(page.getByLabel("Your name")).toHaveValue("Ada");
  await expect(page.getByLabel("Agent name")).toHaveValue("night-owl");
});
