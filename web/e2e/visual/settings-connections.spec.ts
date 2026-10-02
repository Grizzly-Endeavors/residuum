import type { Locator, Page } from "@playwright/test";
import { expect, test } from "../support/fixtures";
import { expectScreenshot } from "../support/screenshot";

/** Atlas with Discord connected, two webhooks, a skill folder and Brave as its search service. */
const CONFIG = `timeout_secs = 120

[discord]
token = "secret:discord"
respond_to_others = true

[webhooks.github-issues]
secret = "secret:webhook_github-issues"
routing = "agent:code-review"
content_fields = ["issue.title", "issue.body"]

[webhooks.deploys]
format = "raw"

[skills]
dirs = ["~/my-skills"]

[tools]
path = ["/opt/residuum-tools"]

[web_search]
backend = "brave"

[web_search.brave]
api_key = "\${BRAVE_API_KEY}"
`;

/** Bring a heading to the top of the section's pane, with the frame's sticky top bar clear of it. */
async function scrollUnderTheTopBar(heading: Locator): Promise<void> {
  await heading.evaluate((element) => {
    element.scrollIntoView({ block: "start" });
    element.closest(".settings-body")?.scrollBy(0, -64);
  });
}

async function openSection(page: Page, section: string): Promise<void> {
  expect((await page.request.put("/api/agents/atlas/config/raw", { data: CONFIG })).ok()).toBe(
    true,
  );
  await page.goto(`/agent/atlas?settings=atlas/${section}`);
}

test.describe("settings connections and tools", { tag: "@visual" }, () => {
  test("Connections, with Discord connected", async ({ page }) => {
    await openSection(page, "connections");
    await expect(
      page.getByRole("region", { name: "Discord" }).getByText("Connected"),
    ).toBeVisible();
    await expectScreenshot(page, "settings-connections");
  });

  test("Connections, the webhooks", async ({ page }) => {
    await openSection(page, "connections");
    const webhooks = page.getByRole("region", { name: "Incoming webhooks" });
    await expect(webhooks.getByText("/webhook/atlas/deploys")).toBeVisible();
    await scrollUnderTheTopBar(webhooks.getByRole("heading", { name: "Incoming webhooks" }));
    await expectScreenshot(page, "settings-connections-webhooks");
  });

  test("Tools & skills", async ({ page }) => {
    await openSection(page, "tools");
    await expect(page.getByText("~/my-skills")).toBeVisible();
    await expect(page.getByLabel("Search service")).toHaveValue("brave");
    await expectScreenshot(page, "settings-tools");
  });

  test("Tools & skills, web search and the provider options", async ({ page }) => {
    await openSection(page, "tools");
    await page.getByRole("button", { name: "Search options for each model provider" }).click();
    await scrollUnderTheTopBar(page.getByRole("heading", { name: "Web search" }));
    await expectScreenshot(page, "settings-tools-search");
  });
});
