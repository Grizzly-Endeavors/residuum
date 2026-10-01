import type { Locator, Page } from "@playwright/test";
import { expectNoAxeViolations } from "../support/axe";
import { expect, test } from "../support/fixtures";

/** Atlas's `config.toml`, as the mock holds it. */
async function configText(page: Page): Promise<string> {
  return await (await page.request.get("/api/agents/atlas/config/raw")).text();
}

const modal = (page: Page): Locator => page.getByRole("dialog", { name: "Settings" });
const saveBar = (page: Page): Locator => page.getByRole("region", { name: "Unsaved changes" });
const group = (page: Page, name: string): Locator => modal(page).getByRole("region", { name });

async function save(page: Page): Promise<void> {
  await saveBar(page).getByRole("button", { name: "Save changes" }).click();
  await expect(saveBar(page)).toBeHidden();
}

/** Read the files again, as the user does with the frame's Reload from disk. */
async function reloadFromDisk(page: Page): Promise<void> {
  await modal(page).getByRole("button", { name: "Reload from disk" }).click();
}

test.describe("Connections", () => {
  test("connects Discord with a token that is stored as a secret", async ({ page }) => {
    await page.goto("/agent/atlas?settings=atlas/connections");
    const discord = group(page, "Discord");
    await expect(discord.getByText("Not connected")).toBeVisible();

    await discord.getByLabel("Bot token", { exact: true }).fill("discord-bot-token-123");
    await expect(saveBar(page)).toContainText("You have unsaved changes.");
    await save(page);

    await expect(discord.getByText("Stored securely")).toBeVisible();
    await expect(discord.getByText("Connected", { exact: true })).toBeVisible();
    await reloadFromDisk(page);
    await expect(discord.getByText("Stored securely")).toBeVisible();
    await expect(discord.getByText("Connected", { exact: true })).toBeVisible();

    const file = await configText(page);
    expect(file).toContain('token = "secret:discord"');
    expect(file).not.toContain("discord-bot-token-123");
    const stored = (await (await page.request.get("/api/hub/secrets")).json()) as {
      names: string[];
    };
    expect(stored.names).toContain("discord");
  });

  test("disconnects as a staged change that Discard brings back", async ({ page }) => {
    await page.goto("/agent/atlas?settings=atlas/connections");
    const discord = group(page, "Discord");
    await discord.getByLabel("Bot token", { exact: true }).fill("tok");
    await save(page);

    await discord.getByRole("button", { name: "Disconnect Discord" }).click();
    await expect(discord.getByText("Disconnects when saved")).toBeVisible();
    await saveBar(page).getByRole("button", { name: "Discard" }).click();
    await expect(discord.getByText("Connected", { exact: true })).toBeVisible();

    await discord.getByRole("button", { name: "Disconnect Discord" }).click();
    await save(page);
    await expect(discord.getByText("Not connected")).toBeVisible();
    expect(await configText(page)).not.toContain("secret:discord");
  });

  test("adds a webhook, then finds it again after Reload from disk", async ({ page }) => {
    await page.goto("/agent/atlas?settings=atlas/connections");
    const hooks = group(page, "Incoming webhooks");
    await expect(hooks.getByText("No webhooks yet.")).toBeVisible();

    await hooks.getByRole("button", { name: "Add webhook" }).click();
    await expect(hooks.getByLabel("Name", { exact: true })).toBeFocused();
    await hooks.getByLabel("Name", { exact: true }).fill("github-issues");
    await expect(hooks.getByText("/webhook/atlas/github-issues")).toBeVisible();
    await hooks.getByLabel("Secret", { exact: true }).fill("hook-secret");
    await hooks.getByLabel("Where it goes").fill("agent:code-review");
    await hooks.getByLabel("Fields to read").fill("issue.title, issue.body");
    await save(page);

    await reloadFromDisk(page);
    await expect(hooks.getByLabel("Name", { exact: true })).toHaveValue("github-issues");
    await expect(hooks.getByLabel("Where it goes")).toHaveValue("agent:code-review");
    await expect(hooks.getByLabel("Fields to read")).toHaveValue("issue.title, issue.body");
    await expect(hooks.getByText("Stored securely")).toBeVisible();
    const file = await configText(page);
    expect(file).toContain("[webhooks.github-issues]");
    expect(file).toContain("secret:webhook_github-issues");
    expect(file).not.toContain("hook-secret");
  });

  test("removes a webhook as a staged change, and Save writes the removal", async ({ page }) => {
    await page.goto("/agent/atlas?settings=atlas/connections");
    const hooks = group(page, "Incoming webhooks");
    await hooks.getByRole("button", { name: "Add webhook" }).click();
    await hooks.getByLabel("Name", { exact: true }).fill("deploys");
    await hooks.getByLabel("Where it goes").fill("agent:triage");
    await save(page);

    await hooks.getByRole("button", { name: "Remove deploys" }).click();
    await expect(hooks.getByText("No webhooks yet.")).toBeVisible();
    expect(await configText(page)).toContain("[webhooks.deploys]");
    await saveBar(page).getByRole("button", { name: "Discard" }).click();
    await expect(hooks.getByLabel("Name", { exact: true })).toHaveValue("deploys");

    await hooks.getByRole("button", { name: "Remove deploys" }).click();
    await save(page);
    expect(await configText(page)).not.toContain("[webhooks.deploys]");
  });

  test("has no accessibility violations, with every channel and a webhook open", async ({
    page,
  }) => {
    await page.goto("/agent/atlas?settings=atlas/connections");
    await group(page, "Discord").getByLabel("Bot token", { exact: true }).fill("tok");
    await save(page);
    await group(page, "Telegram").getByLabel("Bot token", { exact: true }).fill("123:abc");
    const teams = group(page, "Microsoft Teams");
    await teams.getByLabel("App ID").fill("app-1");
    await group(page, "Incoming webhooks").getByRole("button", { name: "Add webhook" }).click();
    await group(page, "Incoming webhooks").getByLabel("Name", { exact: true }).fill("deploys");
    await expect(teams.getByText(/Teams stays off/)).toBeVisible();
    await expectNoAxeViolations(page, { within: "[data-overlay-host]" });
  });
});

test.describe("Tools & skills", () => {
  test("adds a skill folder, then finds it again after Reload from disk", async ({ page }) => {
    await page.goto("/agent/atlas?settings=atlas/tools");
    const skills = group(page, "Skill folders");
    await expect(skills.getByText("No extra skill folders.")).toBeVisible();

    await skills.getByLabel("Skill folder to add").fill("~/my-skills");
    await skills.getByLabel("Skill folder to add").press("Enter");
    await expect(skills.getByText("~/my-skills")).toBeVisible();
    await save(page);

    await reloadFromDisk(page);
    await expect(skills.getByText("~/my-skills")).toBeVisible();
    expect(await configText(page)).toMatch(/dirs = \[\s*"~\/my-skills",?\s*\]/);
  });

  test("adds and removes a tool folder", async ({ page }) => {
    await page.goto("/agent/atlas?settings=atlas/tools");
    const tools = group(page, "Tool folders");
    await tools.getByLabel("Tool folder to add").fill("/opt/residuum-tools");
    await tools.getByRole("button", { name: "Add" }).click();
    await save(page);
    await reloadFromDisk(page);
    await expect(tools.getByText("/opt/residuum-tools")).toBeVisible();

    await tools.getByRole("button", { name: "Remove /opt/residuum-tools" }).click();
    await expect(tools.getByText("No extra tool folders.")).toBeVisible();
    await save(page);
    await reloadFromDisk(page);
    await expect(tools.getByText("No extra tool folders.")).toBeVisible();
  });

  test("sets a web search backend and stores its key as a secret", async ({ page }) => {
    await page.goto("/agent/atlas?settings=atlas/tools");
    const search = group(page, "Web search");
    await search.getByLabel("Search service").selectOption("brave");
    await search.getByLabel("Brave API key", { exact: true }).fill("brave-key-123");
    await save(page);

    await reloadFromDisk(page);
    await expect(search.getByLabel("Search service")).toHaveValue("brave");
    await expect(search.getByText("Stored securely")).toBeVisible();
    const file = await configText(page);
    expect(file).toContain('backend = "brave"');
    expect(file).toContain("secret:ws_brave");
    expect(file).not.toContain("brave-key-123");
  });

  test("keeps each provider's search options behind a disclosure and saves them", async ({
    page,
  }) => {
    await page.goto("/agent/atlas?settings=atlas/tools");
    await modal(page)
      .getByRole("button", { name: "Search options for each model provider" })
      .click();
    await group(page, "Anthropic").getByLabel("Searches per reply").fill("4");
    await group(page, "OpenAI").getByLabel("Search context size").selectOption("high");
    await save(page);
    await reloadFromDisk(page);
    await expect(group(page, "Anthropic").getByLabel("Searches per reply")).toHaveValue("4");
    await expect(group(page, "OpenAI").getByLabel("Search context size")).toHaveValue("high");
  });

  test("has no accessibility violations, with the web search and provider options open", async ({
    page,
  }) => {
    await page.goto("/agent/atlas?settings=atlas/tools");
    await group(page, "Skill folders").getByLabel("Skill folder to add").fill("~/my-skills");
    await group(page, "Skill folders").getByRole("button", { name: "Add" }).click();
    await group(page, "Web search").getByLabel("Search service").selectOption("ollama");
    await modal(page)
      .getByRole("button", { name: "Search options for each model provider" })
      .click();
    await expectNoAxeViolations(page, { within: "[data-overlay-host]" });
  });
});
