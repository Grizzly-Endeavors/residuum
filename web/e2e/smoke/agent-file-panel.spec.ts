import type { Locator, Page } from "@playwright/test";
import { expect, test } from "../support/fixtures";

/**
 * A file panel that names its agent (`panel=agent-file:<agent>:<path>`), as a
 * mail attachment opens beside its thread: it reads that agent's workspace
 * from the Inbox, whichever agent was viewed last, follows the file on disk
 * while the agent runs, opens its links as the same agent's files without
 * leaving the Inbox, and still loads and saves when the agent is stopped.
 */

/** Path and query of the page's URL, decoded, so a test can compare them exactly. */
function address(page: Page): string {
  const url = new URL(page.url());
  return decodeURIComponent(`${url.pathname}${url.search}`);
}

/** The context panel showing `name`: a column, or on a phone a full-screen sheet. */
function filePanel(page: Page, isMobile: boolean, name: string): Locator {
  return page.getByRole(isMobile ? "dialog" : "complementary", { name });
}

test("an agent's file opens beside the Inbox, follows the disk, and its links stay on the Inbox", async ({
  page,
  isMobile,
  mock,
}) => {
  await mock.post("/api/mock/agent-file", {
    params: { agent: "scout" },
    data: { path: "notes/index.md", content: "# Index\n\nSee [the plan](plan.md).\n" },
  });
  await mock.post("/api/mock/agent-file", {
    params: { agent: "scout" },
    data: { path: "notes/plan.md", content: "# Plan\n\nFirst draft.\n" },
  });

  await page.goto("/inbox?panel=agent-file:scout:notes/index.md");
  const index = filePanel(page, isMobile, "index.md");
  await expect(index.getByRole("heading", { name: "Index", exact: true })).toBeVisible();

  await index.getByRole("link", { name: "the plan" }).click();
  const plan = filePanel(page, isMobile, "plan.md");
  await expect(plan.getByText("First draft.")).toBeVisible();
  expect(address(page)).toBe("/inbox?panel=agent-file:scout:notes/plan.md");

  // The page is told of the change over its own connection to scout, once that is up.
  await expect
    .poll(
      async () => {
        await mock.post("/api/mock/agent-file", {
          params: { agent: "scout" },
          data: { path: "notes/plan.md", content: "# Plan\n\nSecond draft.\n" },
        });
        return plan.getByText("Second draft.").isVisible();
      },
      { message: "the page should be told that plan.md changed on disk" },
    )
    .toBe(true);
});

test("a stopped agent's text file loads as written and saves", async ({ page, isMobile, mock }) => {
  await mock.post("/api/mock/agent-file", {
    params: { agent: "atlas" },
    data: { path: "notes/readme.txt", content: "# kept as typed\n" },
  });
  const stopped = await page.request.post("/api/hub/agents/atlas/stop");
  expect(stopped.ok()).toBe(true);

  await page.goto("/inbox?panel=agent-file:atlas:notes/readme.txt");
  const panel = filePanel(page, isMobile, "readme.txt");
  await expect(panel.getByText("# kept as typed")).toBeVisible();
  await expect(panel.getByRole("heading", { name: "kept as typed" })).toHaveCount(0);

  await panel.getByRole("button", { name: "Edit" }).click();
  await panel.getByRole("textbox", { name: "Contents of readme.txt" }).pressSequentially("more");
  await panel.getByRole("button", { name: "Save", exact: true }).click();
  await expect(page.getByText("Saved readme.txt.")).toBeVisible();

  const saved = await page.request.get(
    `/api/agents/atlas/workspace/file?path=${encodeURIComponent("notes/readme.txt")}`,
  );
  expect(await saved.text()).toMatch(/more$/);
});
