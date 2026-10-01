import type { Locator, Page } from "@playwright/test";
import { expectNoAxeViolations } from "../support/axe";
import { expect, test } from "../support/fixtures";

const saveBar = (page: Page): Locator => page.getByRole("region", { name: "Unsaved changes" });
const servers = (page: Page): Locator => page.getByRole("list", { name: "Servers" });
const catalog = (page: Page): Locator => page.getByRole("list", { name: "Catalog" });

/** The servers in atlas's mcp.json on disk. */
async function serversOnDisk(page: Page): Promise<Record<string, { env?: object }>> {
  const raw = await (await page.request.get("/api/agents/atlas/mcp/raw")).text();
  return (JSON.parse(raw) as { mcpServers: Record<string, { env?: object }> }).mcpServers;
}

test("a catalog server is added with its key and saved, and removing it waits for Save until Discard brings it back", async ({
  page,
}) => {
  await page.goto("/agent/atlas?settings=atlas/servers");
  await catalog(page).getByRole("button", { name: "Add github" }).click();
  const form = page.getByRole("form", { name: "Add github" });
  await form.getByRole("button", { name: "Add github" }).click();
  await expect(form.getByText("Enter the GitHub Personal Access Token.")).toBeVisible();
  await form.getByLabel("GitHub Personal Access Token").fill("ghp_e2e");
  await form.getByRole("button", { name: "Add github" }).click();

  await expect(servers(page).getByText("github", { exact: true })).toBeVisible();
  await expect(catalog(page).getByText("Added")).toHaveCount(2);
  await saveBar(page).getByRole("button", { name: "Save changes" }).click();
  await expect(page.getByRole("status").filter({ hasText: "Saved mcp.json." })).toBeVisible();
  expect((await serversOnDisk(page)).github?.env).toEqual({
    GITHUB_PERSONAL_ACCESS_TOKEN: "ghp_e2e",
  });

  await servers(page).getByRole("button", { name: "Remove github" }).click();
  await expect(servers(page).getByText("github", { exact: true })).toBeHidden();
  await expect(saveBar(page)).toContainText("You have unsaved changes.");
  await expectNoAxeViolations(page, { within: "[data-overlay-host]" });

  await saveBar(page).getByRole("button", { name: "Discard" }).click();
  await expect(servers(page).getByText("github", { exact: true })).toBeVisible();
  await expect(saveBar(page)).toBeHidden();
  expect(Object.keys(await serversOnDisk(page))).toContain("github");
});

test("Undo after saving a removal puts the server back on disk", async ({ page }) => {
  await page.goto("/agent/atlas?settings=atlas/servers");
  await servers(page).getByRole("button", { name: "Remove filesystem" }).click();
  await saveBar(page).getByRole("button", { name: "Save changes" }).click();
  const saved = page.getByRole("status").filter({ hasText: "Saved mcp.json." });
  await expect(saved).toBeVisible();
  expect(Object.keys(await serversOnDisk(page))).not.toContain("filesystem");

  await saved.getByRole("button", { name: "Undo" }).click();
  await expect(servers(page).getByText("filesystem", { exact: true })).toBeVisible();
  expect(Object.keys(await serversOnDisk(page))).toContain("filesystem");
});

test("a catalog that can't be read says so, and Try again reads it", async ({ page }) => {
  let unavailable = true;
  await page.route("**/api/hub/mcp-catalog", (route) =>
    unavailable
      ? route.fulfill({
          status: 503,
          contentType: "application/json",
          body: JSON.stringify({ error: "the catalog is unavailable" }),
        })
      : route.fallback(),
  );
  await page.goto("/agent/atlas?settings=atlas/servers");
  const problem = page
    .getByRole("alert")
    .filter({ hasText: "Couldn't read the tool server catalog." });
  await expect(problem).toBeVisible();
  await expect(page.getByText("The catalog has no servers in it.")).toBeHidden();
  await expectNoAxeViolations(page, { within: "[data-overlay-host]" });

  unavailable = false;
  await problem.getByRole("button", { name: "Try again" }).click();
  await expect(catalog(page).getByRole("button", { name: "Add tavily" })).toBeVisible();
  await expect(problem).toBeHidden();
});

test("visibility applies at once, with no Save, and only the hub writes it", async ({ page }) => {
  await page.goto("/agent/atlas?settings=atlas/a2a");
  const choices = page.getByRole("radiogroup", { name: "Who can find atlas" });
  await expect(choices.getByRole("radio", { name: "Private" })).toHaveAttribute(
    "aria-checked",
    "true",
  );
  await expectNoAxeViolations(page, { within: "[data-overlay-host]" });

  await choices.getByRole("radio", { name: "Public" }).click();
  await expect(page.getByRole("status").filter({ hasText: "atlas is public" })).toBeVisible();
  await expect(choices.getByRole("radio", { name: "Public" })).toBeFocused();
  await expect(saveBar(page)).toBeHidden();
  expect(await (await page.request.get("/api/agents/atlas/config/raw")).text()).toMatch(
    /visibility = "public"/,
  );

  await page.reload();
  await expect(choices.getByRole("radio", { name: "Public" })).toHaveAttribute(
    "aria-checked",
    "true",
  );
});

test("a stopped agent asks to start for its status, card and reachability, and its remote agents stay editable", async ({
  page,
}) => {
  await page.goto("/agent/drifter?settings=drifter/a2a");
  for (const subject of ["its status and address", "its card", "whether they can be reached"]) {
    await expect(page.getByText(`Start drifter to see ${subject}.`, { exact: true })).toBeVisible();
  }
  await expect(page.getByText("No remote agents yet. List one in a2a.json.")).toBeVisible();
  await expectNoAxeViolations(page, { within: "[data-overlay-host]" });

  await page.getByRole("button", { name: "Edit a2a.json" }).click();
  const listed = { agents: { laptop: { url: "https://laptop.example/a2a" } } };
  await page.getByLabel("Contents of a2a.json").fill(JSON.stringify(listed, null, 2));
  await page.getByRole("button", { name: "Save a2a.json" }).click();
  await expect(page.getByRole("status").filter({ hasText: "Saved a2a.json." })).toBeVisible();
  await expect(page.getByRole("list", { name: "Remote agents" })).toContainText(
    "https://laptop.example/a2a",
  );
  expect(await (await page.request.get("/api/agents/drifter/a2a/agents/raw")).text()).toContain(
    "laptop.example",
  );
  await expectNoAxeViolations(page, { within: "[data-overlay-host]" });
});

test("a running agent shows its address, its remote agents' reachability and its card", async ({
  page,
}) => {
  await page.goto("/agent/atlas?settings=atlas/a2a");
  await expect(page.getByText("Listening")).toBeVisible();
  await expect(page.getByText("http://127.0.0.1:7702/agents/atlas")).toBeVisible();
  const remote = page.getByRole("list", { name: "Remote agents" });
  await expect(remote.getByText("Reachable")).toBeVisible();
  await expect(remote.getByText("Your other install")).toBeVisible();
  await expect(
    page.getByText("A personal AI agent, reachable over the Agent2Agent (A2A) protocol."),
  ).toBeVisible();
  await expectNoAxeViolations(page, { within: "[data-overlay-host]" });

  await page.getByRole("button", { name: "Change for all agents" }).click();
  await expect.poll(() => new URL(page.url()).searchParams.get("settings")).toBe("_all/listener");
  await expect(page.getByRole("heading", { name: "Agent-to-agent", level: 2 })).toBeVisible();
});
