import type { Locator, Page } from "@playwright/test";
import { NO_SECURE_ORIGIN_REASON } from "../../src/lib/workbench";
import { expectNoAxeViolations } from "../support/axe";
import { expect, test, type MockControls } from "../support/fixtures";
import { devServer } from "../support/servers";

/**
 * The Workbench, a launcher: the list and a selected row's detail, Open in a
 * new tab on the artifacts origin, Copy link, the hub's artifact events with
 * no agent running, an artifact's sessions on any agent opening in the panel,
 * delete with Undo, the URL corrections, and the banner when artifacts can't
 * open. Nothing in the app embeds an artifact.
 */

const ARTIFACTS_ORIGIN = `http://localhost:${String(devServer.artifactsPort)}`;
/** The purpose of the mock's sample sessions that `wiki-graph` started, on atlas and on scout. */
const WIKI_RUN = "Write a wiki page summarizing this week's notes on otters";

/** Path and query of the page's URL, decoded, so a test can compare them exactly. */
function address(page: Page): string {
  const url = new URL(page.url());
  return decodeURIComponent(`${url.pathname}${url.search}`);
}

/** The row for the page titled `title`. */
function row(page: Page, title: string): Locator {
  return page
    .getByRole("list", { name: "Pages" })
    .locator(":scope > li")
    .filter({ has: page.getByText(title, { exact: true }) });
}

/** Add the page the mock's sample `artifact:wiki-graph` sessions belong to. */
async function addWikiGraph(mock: MockControls): Promise<void> {
  await mock.post("/api/mock/team-file", {
    data: {
      path: "team/workbench/wiki-graph.html",
      content: "<!doctype html><html><head><title>Wiki graph</title></head><body></body></html>",
    },
  });
}

/** Open `url` and wait for the hub socket: the overview is fetched once it says hello. */
async function openConnected(page: Page, url: string): Promise<void> {
  const hello = page.waitForResponse((r) => new URL(r.url()).pathname === "/api/hub/overview");
  await page.goto(url);
  await hello;
}

test("the list shows each page, and a row selected opens in place until Back", async ({ page }) => {
  await page.goto("/team/workbench");
  await expect(page.getByRole("heading", { name: "Workbench", level: 1 })).toBeVisible();
  const tip = row(page, "Tip Splitter");
  await expect(tip.getByText("/team/workbench/tip-splitter")).toBeVisible();
  await expect(tip.getByText(/^edited /)).toBeVisible();
  await expectNoAxeViolations(page);

  const head = tip.getByRole("button", { name: /^Tip Splitter/ });
  await head.click();
  await expect.poll(() => address(page)).toBe("/team/workbench/tip-splitter");
  await expect(head).toHaveAttribute("aria-expanded", "true");
  await expect(tip.getByText(`${ARTIFACTS_ORIGIN}/tip-splitter/`)).toBeVisible();
  await expect(tip.getByText("Nothing running.", { exact: false })).toBeVisible();
  await expect(page.locator("iframe")).toHaveCount(0);
  await expectNoAxeViolations(page);

  await page.goBack();
  await expect.poll(() => address(page)).toBe("/team/workbench");
  await expect(head).toHaveAttribute("aria-expanded", "false");
});

test("Open opens the page on the artifacts origin in a new tab", async ({ page }) => {
  await page.goto("/team/workbench");
  const opened = page.waitForEvent("popup");
  await row(page, "Tip Splitter").getByRole("link", { name: "Open Tip Splitter" }).click();
  const tab = await opened;
  expect(tab.url()).toBe(`${ARTIFACTS_ORIGIN}/tip-splitter/`);
  await expect(tab.getByRole("heading", { name: "Tip splitter" })).toBeVisible();
  // The app stays where it was.
  await expect.poll(() => address(page)).toBe("/team/workbench");
});

test("Copy link copies the page's address", async ({ page }) => {
  await page.context().grantPermissions(["clipboard-read", "clipboard-write"]);
  await page.goto("/team/workbench/tip-splitter");
  await row(page, "Tip Splitter").getByRole("button", { name: "Copy link" }).click();
  await expect(page.getByText("Copied the link to “Tip Splitter”.")).toBeVisible();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(
    `${ARTIFACTS_ORIGIN}/tip-splitter/`,
  );

  // The row's menu offers it too.
  await page.getByRole("button", { name: "More for Tip Splitter" }).click();
  await expect(page.getByRole("menuitem", { name: "Copy link" })).toBeVisible();
  await expectNoAxeViolations(page);
});

test("an agent's edit marks the page as updating now, with no agent running", async ({
  page,
  mock,
}) => {
  for (const agent of ["atlas", "scout"]) {
    expect((await page.request.post(`/api/hub/agents/${agent}/stop`)).ok()).toBe(true);
  }
  await openConnected(page, "/team/workbench");
  const tip = row(page, "Tip Splitter");
  await expect(tip.getByText(/^edited /)).toBeVisible();

  await mock.post("/api/mock/team-file", {
    data: {
      path: "team/workbench/tip-splitter.html",
      content:
        "<!doctype html><html><head><title>Tip Splitter</title></head><body>v2</body></html>",
    },
  });
  await expect(tip.getByText("updating now")).toBeVisible();
  await expect(tip.getByText(/^edited /)).toBeVisible();

  // A page an agent writes appears the same way.
  await addWikiGraph(mock);
  await expect(row(page, "Wiki graph").getByText("updating now")).toBeVisible();
});

test("a page's running sessions, on any agent, are listed and open in the panel", async ({
  page,
  mock,
  isMobile,
}) => {
  await addWikiGraph(mock);
  await page.goto("/team/workbench");
  const graph = row(page, "Wiki graph");
  await expect(graph.getByText("2 sessions running")).toBeVisible();

  await graph.getByRole("button", { name: /^Wiki graph/ }).click();
  await expect(graph.getByText("On atlas", { exact: true })).toBeVisible();
  await graph.getByRole("button", { name: new RegExp(`^${WIKI_RUN} On scout`) }).click();
  await expect
    .poll(() => address(page))
    .toBe("/team/workbench/wiki-graph?panel=session:scout:run-live-wiki-graph");
  const panel = page.getByRole(isMobile ? "dialog" : "complementary", { name: WIKI_RUN });
  await expect(panel.getByText("On scout", { exact: true })).toBeVisible();
  if (isMobile) await expectNoAxeViolations(page, { within: "[data-overlay-host]" });
  else await expectNoAxeViolations(page);
});

test("Delete removes the page at once, and Undo brings it back", async ({ page }) => {
  await page.goto("/team/workbench/tip-splitter");
  const tip = row(page, "Tip Splitter");
  await expect(tip).toBeVisible();

  await page.getByRole("button", { name: "More for Tip Splitter" }).click();
  await page.getByRole("menuitem", { name: "Delete" }).click();
  await expect(tip).toHaveCount(0);
  await expect.poll(() => address(page)).toBe("/team/workbench");
  await expect(page.getByText("Nothing on the bench yet")).toBeVisible();

  await page.getByRole("button", { name: "Undo" }).click();
  await expect(page.getByText("Restored.")).toBeVisible();
  await expect(row(page, "Tip Splitter")).toBeVisible();
});

test("an unknown page goes back to the list, with a toast", async ({ page }) => {
  await page.goto("/team/workbench/ghost");
  await expect.poll(() => address(page)).toBe("/team/workbench");
  await expect(page.getByText(`There's no workbench page named "ghost".`)).toBeVisible();
});

test("the old full view parameter is dropped by replace", async ({ page }) => {
  await page.goto("/home");
  await page.goto("/team/workbench/tip-splitter?full");
  await expect.poll(() => address(page)).toBe("/team/workbench/tip-splitter");
  await expect(
    row(page, "Tip Splitter").getByRole("button", { name: /^Tip Splitter/ }),
  ).toHaveAttribute("aria-expanded", "true");
  await page.goBack();
  await expect.poll(() => address(page)).toBe("/home");
});

test("over HTTPS with no relay origin, a banner says why pages can't open", async ({
  page,
  request,
}) => {
  // The app served from an HTTPS origin, the way a reverse proxy or Residuum
  // Cloud serves it, with every request answered by the mock.
  const secure = "https://residuum.test";
  await page.route(`${secure}/**`, async (route) => {
    const sent = route.request();
    const url = new URL(sent.url());
    const { host: _host, ...headers } = sent.headers();
    const response = await request.fetch(`${url.pathname}${url.search}`, {
      method: sent.method(),
      headers,
      data: sent.postDataBuffer() ?? undefined,
    });
    await route.fulfill({ response });
  });

  await page.goto(`${secure}/team/workbench`);
  await expect(page.getByText("Workbench pages can't open right now.")).toBeVisible();
  await expect(page.getByText(NO_SECURE_ORIGIN_REASON)).toBeVisible();
  await expect(
    row(page, "Tip Splitter").getByRole("button", { name: "Open Tip Splitter" }),
  ).toBeDisabled();
});
