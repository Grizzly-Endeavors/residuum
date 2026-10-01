import type { Page } from "@playwright/test";
import { expect, test } from "../support/fixtures";
import { expectScreenshot } from "../support/screenshot";

/**
 * The context panel's baselines: beside the chat at wide width, floating at
 * medium width, and a full-screen sheet on phones. Hosted legacy views are
 * left out, since their own units give them baselines when they are rebuilt:
 * the panel's is painted over, and the place's is hidden rather than painted
 * over, because a mask paints over everything in its box, and the panel
 * floats over the place at medium width and covers it on phones.
 */

async function panelScreenshot(page: Page, name: string): Promise<void> {
  await page.addStyleTag({ content: "main [data-legacy-view] { visibility: hidden; }" });
  await expectScreenshot(page, name, {
    mask: [page.locator("aside [data-legacy-view], [data-overlay-host] [data-legacy-view]")],
  });
}

test.describe("context panel", { tag: "@visual" }, () => {
  test("a file beside the chat, or over it on a phone", async ({ page }) => {
    await page.goto("/agent/atlas?panel=file:team/wiki/index.md");
    await expect(page.getByRole("heading", { name: "index.md" })).toBeVisible();
    await expect(page.getByRole("textbox", { name: "Contents of index.md" })).toHaveValue(
      /# Wiki Index/,
    );
    await panelScreenshot(page, "panel-file");
  });

  test("a run on another agent, from the Workbench", async ({ page }) => {
    await page.goto("/team/workbench?panel=session:scout:run-live-research");
    await expect(page.getByRole("heading", { name: "This run is scout's" })).toBeVisible();
    await panelScreenshot(page, "panel-elsewhere");
  });

  test("at medium width", async ({ page, isMobile }) => {
    test.skip(isMobile, "Medium width is a desktop window narrowed.");
    await page.setViewportSize({ width: 1000, height: 760 });
    await page.goto("/agent/scout/schedule?panel=file:SOUL.md");
    await expect(page.getByRole("textbox", { name: "Contents of SOUL.md" })).toHaveValue(/# Soul/);
    await panelScreenshot(page, "panel-medium");
  });
});
