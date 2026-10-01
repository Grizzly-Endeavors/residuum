import type { Page } from "@playwright/test";
import { expect, test } from "../support/fixtures";
import { expectScreenshot } from "../support/screenshot";

/**
 * The Workbench's baselines: the list with a page selected and its sessions
 * on two agents, and the banner over an empty bench when pages can't open.
 */

/**
 * Open `url` once the hub socket is up (the overview is fetched when it says
 * hello), so a slow connection's "Can't reach Residuum" banner isn't shot.
 */
async function openConnected(page: Page, url: string): Promise<void> {
  const hello = page.waitForResponse((r) => new URL(r.url()).pathname === "/api/hub/overview");
  await page.goto(url);
  await hello;
  await expect(page.getByText("Can't reach Residuum.")).toBeHidden();
}

test.describe("workbench", { tag: "@visual" }, () => {
  test("the list, with a page selected and its sessions", async ({ page, mock }) => {
    await mock.post("/api/mock/team-file", {
      data: {
        path: "team/workbench/wiki-graph.html",
        content: "<!doctype html><html><head><title>Wiki graph</title></head><body></body></html>",
      },
    });
    await openConnected(page, "/team/workbench/wiki-graph");
    await expect(page.getByText("2 sessions running")).toBeVisible();
    await expect(page.getByText("On scout", { exact: true })).toBeVisible();
    // The address carries the artifacts port, which a run on other ports changes.
    await expectScreenshot(page, "workbench", { mask: [page.locator(".link-text")] });
  });

  test("pages can't open, over an empty bench", async ({ page, mock }) => {
    await mock.post("/api/mock/team-file", {
      data: { path: "team/workbench/tip-splitter.html", content: null },
    });
    await page.route("**/api/team/workbench/info", (route) =>
      route.fulfill({
        json: {
          port: null,
          unavailable_reason:
            "Residuum couldn't start the workbench artifacts listener: no free port after 7701.",
          relay: null,
        },
      }),
    );
    await openConnected(page, "/team/workbench");
    await expect(page.getByText("Workbench pages can't open right now.")).toBeVisible();
    await expect(page.getByText("Nothing on the bench yet")).toBeVisible();
    await expectScreenshot(page, "workbench-unavailable");
  });
});
