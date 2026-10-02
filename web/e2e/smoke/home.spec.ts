import type { Locator, Page } from "@playwright/test";
import { expectNoAxeViolations } from "../support/axe";
import { expect, test } from "../support/fixtures";
import { expectSettingsOpen } from "../support/lazy";

/**
 * Home: what needs the user, with the fixes that clear it live, the agents
 * board, what happened across the team, and what runs next. The scenario has
 * brittle failed, atlas and scout each with a task to the unreachable laptop,
 * and an unread item in each of their inboxes.
 */

function needsYou(page: Page): Locator {
  return page.getByRole("region", { name: /^Needs you/ });
}

function need(page: Page, title: string | RegExp): Locator {
  return needsYou(page).getByRole("listitem").filter({ hasText: title });
}

function board(page: Page): Locator {
  return page.getByRole("table", { name: "Agents" });
}

function acrossTheTeam(page: Page): Locator {
  return page.getByRole("region", { name: "Across the team" });
}

/** Path and query of the page's URL, decoded, so a test can compare them exactly. */
function address(page: Page): string {
  const url = new URL(page.url());
  return decodeURIComponent(`${url.pathname}${url.search}`);
}

test("Home shows what needs the user, every agent, and what happened across the team", async ({
  page,
}) => {
  await page.goto("/home");

  await expect(page.getByRole("heading", { name: "Home", level: 1 })).toBeVisible();
  await expect(need(page, "brittle couldn't start")).toBeVisible();
  await expect(need(page, "atlas can't reach laptop")).toBeVisible();
  await expect(need(page, "scout can't reach laptop")).toBeVisible();
  await expect(need(page, "Deploy tomorrow")).toHaveCount(2);

  for (const agent of ["atlas", "brittle", "drifter", "scout"]) {
    await expect(board(page).getByRole("link", { name: agent, exact: true })).toBeVisible();
  }
  await expect(acrossTheTeam(page).getByText("Residuum started")).toBeVisible();
  await expect(page.getByRole("region", { name: "Coming up" })).toBeVisible();
  await expectNoAxeViolations(page);
});

test("the rail's Home count is the number of things that need the user", async ({
  page,
  isMobile,
}) => {
  test.skip(isMobile, "The phone's bar carries no Home count.");
  await page.goto("/home");
  await expect(need(page, "Deploy tomorrow")).toHaveCount(2);
  const home = page.getByRole("navigation", { name: "Places and agents" }).getByRole("link", {
    name: /^Home/,
  });
  await expect(home).toHaveAccessibleName("Home 5 things need you");

  await need(page, "atlas can't reach laptop")
    .getByRole("button", { name: "Stop watching" })
    .click();
  await expect(home).toHaveAccessibleName("Home 4 things need you");
});

test("brittle's item clears once its settings are fixed and it restarts", async ({
  page,
  mock,
}) => {
  await page.goto("/home");
  const brittle = need(page, "brittle couldn't start");
  await expect(brittle).toBeVisible();

  // Before the fix a restart fails again, and the item stays.
  await brittle.getByRole("button", { name: "Restart brittle" }).click();
  await expect(page.getByRole("alert").filter({ hasText: "brittle failed" })).toBeVisible();
  await expect(brittle).toBeVisible();

  await mock.post("/api/mock/fix-agent", { params: { agent: "brittle" } });
  await brittle.getByRole("button", { name: "Restart brittle" }).click();

  await expect(brittle).toBeHidden();
  const row = board(page).getByRole("row").filter({ hasText: "brittle" });
  await expect(row.getByText("Running", { exact: true }).filter({ visible: true })).toBeVisible();
});

test("Fix settings opens brittle's settings where the problem can be fixed", async ({ page }) => {
  await page.goto("/home");
  await need(page, "brittle couldn't start").getByRole("button", { name: "Fix settings" }).click();
  await expect.poll(() => address(page)).toMatch(/^\/home\?settings=brittle\//);
  await expectSettingsOpen(page);
});

test("an inbox item's Open goes to that item in the Inbox", async ({ page }) => {
  await page.goto("/home");
  await need(page, "Deploy tomorrow")
    .first()
    .getByRole("link", { name: "Open Deploy tomorrow" })
    .click();
  await expect.poll(() => address(page)).toBe("/inbox?item=atlas:mock_1");
});

test("Stop watching removes an unreachable agent's task", async ({ page }) => {
  await page.goto("/home");
  const atlas = need(page, "atlas can't reach laptop");
  await expect(atlas).toBeVisible();

  await atlas.getByRole("button", { name: "Stop watching" }).click();

  await expect(atlas).toBeHidden();
  await expect(need(page, "scout can't reach laptop")).toBeVisible();
  // The hub's own frame agrees once it arrives.
  await page.reload();
  await expect(need(page, "scout can't reach laptop")).toBeVisible();
  await expect(need(page, "atlas can't reach laptop")).toBeHidden();
});

test("Stop task says when the remote agent can't be reached", async ({ page }) => {
  await page.goto("/home");
  const atlas = need(page, "atlas can't reach laptop");
  await atlas.getByRole("button", { name: "Stop task" }).click();
  await expect(atlas.getByRole("status")).toContainText("Stop watching closes it here instead.");
  await expect(atlas).toBeVisible();
});

test("a turn shows up across the team and on the agent's row", async ({ page, mock }) => {
  await page.goto("/home");
  await expect(acrossTheTeam(page).getByText("Residuum started")).toBeVisible();

  await mock.post("/api/mock/teammate-message", { params: { agent: "atlas" } });

  const replied = acrossTheTeam(page).getByRole("link", {
    name: "atlas replied in your conversation",
  });
  await expect(replied).toBeVisible();
  await expect(
    board(page).getByRole("row").filter({ hasText: "scout asked me to check the wiki index" }),
  ).toHaveCount(1);

  await replied.click();
  await expect.poll(() => address(page)).toBe("/agent/atlas");
});

test("a board row opens the agent's chat", async ({ page }) => {
  await page.goto("/home");
  await board(page).getByRole("link", { name: "drifter", exact: true }).click();
  await expect.poll(() => address(page)).toBe("/agent/drifter");
});

test("Coming up opens the agent's schedule", async ({ page }) => {
  await page.goto("/home");
  const coming = page.getByRole("region", { name: "Coming up" });
  await coming.getByRole("link", { name: "Inbox check" }).first().click();
  await expect.poll(() => address(page)).toBe("/agent/atlas/schedule");
});

test("needs-you says so when nothing is waiting, and fills in as things arrive", async ({
  page,
  mock,
}) => {
  await page.goto("/home");
  await expect(need(page, "Deploy tomorrow")).toHaveCount(2);

  // Clear everything the scenario starts with, the way the user would elsewhere.
  for (const agent of ["atlas", "scout"]) {
    const stopped = await page.request.post(
      `/api/agents/${agent}/a2a/outbound/task-19c2/stop-watching`,
    );
    expect(stopped.ok()).toBe(true);
    const read = await page.request.put(`/api/hub/inbox/${agent}/mock_1/read`);
    expect(read.ok()).toBe(true);
  }
  const removed = await page.request.delete("/api/hub/agents/brittle");
  expect(removed.ok()).toBe(true);

  await expect(needsYou(page).getByText("Nothing is waiting on you.")).toBeVisible();
  await expectNoAxeViolations(page);

  await mock.post("/api/mock/user-inbox-add", {
    params: { agent: "scout" },
    data: { title: "Release notes are ready" },
  });
  await expect(need(page, "Release notes are ready")).toBeVisible();
});
