import type { Locator, Page } from "@playwright/test";
import { expectNoAxeViolations } from "../support/axe";
import { expect, test } from "../support/fixtures";

/**
 * The Inbox: every agent's user inbox in one list. The scenario gives atlas
 * and scout each an unread "Deploy tomorrow", a read "Daily Digest" and an
 * archived "Last week's digest"; drifter and brittle have never run, so their
 * inboxes are empty.
 */

function inbox(page: Page): Locator {
  return page.getByRole("region", { name: "Inbox" });
}

/** An item's row, by its title and its agent. */
function item(page: Page, title: string, agent: string): Locator {
  return inbox(page)
    .getByRole("listitem")
    .filter({ has: page.getByRole("button", { name: new RegExp(`^${title}\\b`) }) })
    .filter({ hasText: agent });
}

function itemButton(page: Page, title: string, agent: string): Locator {
  return item(page, title, agent).getByRole("button", { name: new RegExp(`^${title}\\b`) });
}

/** The Inbox link with its count: in the rail, or in the phone's bottom bar. */
function inboxLink(page: Page, isMobile: boolean): Locator {
  const nav = isMobile
    ? page.getByRole("navigation", { name: "Main" })
    : page.getByRole("navigation", { name: "Places and agents" });
  return nav.getByRole("link", { name: /^Inbox/ });
}

/** Path and query of the page's URL, decoded, so a test can compare them exactly. */
function address(page: Page): string {
  const url = new URL(page.url());
  return decodeURIComponent(`${url.pathname}${url.search}`);
}

test("lists every agent's items, and opening one marks it read and drops the count", async ({
  page,
  isMobile,
}) => {
  await page.goto("/inbox");
  await expect(inbox(page).getByRole("listitem")).toHaveCount(4);
  await expect(inboxLink(page, isMobile)).toHaveAccessibleName("Inbox 2 unread");
  await expect(itemButton(page, "Deploy tomorrow", "atlas")).toHaveAccessibleName(/, unread$/);
  await expectNoAxeViolations(page);

  await itemButton(page, "Deploy tomorrow", "atlas").click();

  await expect.poll(() => address(page)).toBe("/inbox?item=atlas:mock_1");
  const opened = item(page, "Deploy tomorrow", "atlas");
  await expect(opened.getByText("Reminder to trigger the deployment pipeline")).toBeVisible();
  await expect(itemButton(page, "Deploy tomorrow", "atlas")).toHaveAttribute(
    "aria-expanded",
    "true",
  );
  await expect(itemButton(page, "Deploy tomorrow", "atlas")).not.toHaveAccessibleName(/unread/);
  await expect(inboxLink(page, isMobile)).toHaveAccessibleName("Inbox 1 unread");
  await expectNoAxeViolations(page);
});

test("Back closes the open item before it leaves the Inbox", async ({ page }) => {
  await page.goto("/home");
  await page.goto("/inbox");
  await itemButton(page, "Daily Digest", "scout").click();
  await expect.poll(() => address(page)).toBe("/inbox?item=scout:mock_2");

  // Another item takes its place without adding to the history.
  await itemButton(page, "Daily Digest", "atlas").click();
  await expect.poll(() => address(page)).toBe("/inbox?item=atlas:mock_2");

  await page.goBack();
  await expect.poll(() => address(page)).toBe("/inbox");
  await expect(inbox(page).getByRole("button", { expanded: true })).toHaveCount(0);

  await itemButton(page, "Daily Digest", "scout").click();
  await itemButton(page, "Daily Digest", "scout").click();
  await expect.poll(() => address(page)).toBe("/inbox");
  await page.goBack();
  await expect.poll(() => address(page)).toBe("/home");
});

test("archives an item, finds it in the archive, and moves it back", async ({ page }) => {
  await page.goto("/inbox?item=atlas:mock_2");
  await item(page, "Daily Digest", "atlas").getByRole("button", { name: "Archive" }).click();

  await expect(item(page, "Daily Digest", "atlas")).toHaveCount(0);
  await expect(page.getByText("Archived “Daily Digest”.")).toBeVisible();
  await expect.poll(() => address(page)).toBe("/inbox");

  await inbox(page).getByRole("tab", { name: "Archived" }).click();
  await expect.poll(() => address(page)).toBe("/inbox?tab=archived");
  await expect(inbox(page).getByRole("listitem")).toHaveCount(3);
  await itemButton(page, "Daily Digest", "atlas").click();
  await expectNoAxeViolations(page);
  await item(page, "Daily Digest", "atlas")
    .getByRole("button", { name: "Move back to inbox" })
    .click();
  await expect(item(page, "Daily Digest", "atlas")).toHaveCount(0);

  await inbox(page)
    .getByRole("tab", { name: /^Inbox/ })
    .click();
  await expect.poll(() => address(page)).toBe("/inbox");
  await expect(item(page, "Daily Digest", "atlas")).toBeVisible();
});

test("Undo on the archive toast brings the item back", async ({ page }) => {
  await page.goto("/inbox?item=scout:mock_2");
  await item(page, "Daily Digest", "scout").getByRole("button", { name: "Archive" }).click();
  await expect(item(page, "Daily Digest", "scout")).toHaveCount(0);

  await page.getByRole("button", { name: "Undo" }).click();

  await expect(item(page, "Daily Digest", "scout")).toBeVisible();
});

test("the agent filter shows one agent's items", async ({ page }) => {
  await page.goto("/inbox");
  const filter = inbox(page).getByRole("combobox", { name: "Show items from" });
  await expect(filter.getByRole("option", { name: "atlas (1 unread)" })).toBeAttached();

  await filter.selectOption("scout");

  await expect.poll(() => address(page)).toBe("/inbox?agent=scout");
  await expect(inbox(page).getByRole("listitem")).toHaveCount(2);
  await expect(item(page, "Deploy tomorrow", "atlas")).toHaveCount(0);

  await filter.selectOption("drifter");
  await expect(inbox(page).getByText("Nothing from drifter right now.")).toBeVisible();

  await filter.selectOption("");
  await expect.poll(() => address(page)).toBe("/inbox");
  await expect(inbox(page).getByRole("listitem")).toHaveCount(4);
});

test("a link opens its item, and one to an item that's gone says so", async ({ page }) => {
  await page.goto("/inbox?agent=scout&item=scout:mock_1");
  await expect(
    item(page, "Deploy tomorrow", "scout").getByText("Reminder to trigger the deployment"),
  ).toBeVisible();
  await expect(itemButton(page, "Deploy tomorrow", "scout")).not.toHaveAccessibleName(/unread/);

  await page.goto("/inbox?item=atlas:no_such_item");
  await expect(page.getByText("That item isn't in atlas's inbox any more.")).toBeVisible();
  await expect.poll(() => address(page)).toBe("/inbox");
});

test("a link to an item the list doesn't hold opens it above the list", async ({ page }) => {
  // An archived item, linked from the inbox's tab.
  await page.goto("/inbox?item=atlas:mock_archived_1");
  const apart = inbox(page).getByRole("list", { name: "Opened item" });
  await expect(apart.getByText("Here was last week's summary.")).toBeVisible();
});

test("a stopped agent's item opens, and its attachment downloads", async ({ page, mock }) => {
  const { id } = (await mock.post("/api/mock/user-inbox-add", {
    params: { agent: "atlas" },
    data: {
      title: "Quarterly numbers",
      body: "The **report** is attached.",
      attachments: [{ filename: "report.txt" }],
    },
  })) as { id: string };
  const stopped = await page.request.post("/api/hub/agents/atlas/stop");
  expect(stopped.ok()).toBe(true);

  await page.goto(`/inbox?item=atlas:${id}`);
  const opened = item(page, "Quarterly numbers", "atlas");
  await expect(opened.getByText("report", { exact: true })).toBeVisible();
  const file = opened.getByRole("link", { name: /report\.txt/ });
  await expect(file).toBeVisible();

  const download = page.waitForEvent("download");
  await file.click();
  expect((await download).suggestedFilename()).toBe("report.txt");
  const served = await page.request.get((await file.getAttribute("href")) ?? "");
  expect(await served.text()).toBe("Mock attachment: report.txt\n");
});

test("a new item appears while the Inbox is open", async ({ page, mock }) => {
  await page.goto("/inbox");
  await expect(inbox(page).getByRole("listitem")).toHaveCount(4);

  await mock.post("/api/mock/user-inbox-add", {
    params: { agent: "scout" },
    data: { title: "Build is green" },
  });

  await expect(itemButton(page, "Build is green", "scout")).toBeVisible();
});

test("a list that can't load says so, and Try again loads it", async ({ page }) => {
  await page.route(/\/api\/hub\/inbox\?/, (route) =>
    route.fulfill({ status: 500, contentType: "application/json", body: '{"error":"boom"}' }),
  );
  await page.goto("/inbox");
  const problem = inbox(page).getByRole("alert").filter({ hasText: "Couldn't load your inbox." });
  await expect(problem).toBeVisible();
  await expectNoAxeViolations(page);

  await page.unroute(/\/api\/hub\/inbox\?/);
  await problem.getByRole("button", { name: "Try again" }).click();
  await expect(inbox(page).getByRole("listitem")).toHaveCount(4);
});

test("an archive that fails stays put and can be tried again", async ({ page }) => {
  await page.route(/\/api\/hub\/inbox\/[^/]+\/[^/]+\/archive$/, (route) =>
    route.fulfill({ status: 503, contentType: "application/json", body: '{"error":"busy"}' }),
  );
  await page.goto("/inbox?item=atlas:mock_2");
  const row = item(page, "Daily Digest", "atlas");
  await row.getByRole("button", { name: "Archive" }).click();

  const problem = row.getByRole("alert");
  await expect(problem).toContainText("Couldn't archive it.");
  await expect(row).toBeVisible();

  await page.unroute(/\/api\/hub\/inbox\/[^/]+\/[^/]+\/archive$/);
  await problem.getByRole("button", { name: "Try again" }).click();
  await expect(row).toHaveCount(0);
});
