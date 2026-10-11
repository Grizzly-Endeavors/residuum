import type { Locator, Page } from "@playwright/test";
import { expectNoAxeViolations } from "../support/axe";
import { expect, test } from "../support/fixtures";
import { expectFileOpen } from "../support/lazy";

/**
 * The context panel: opened and closed by link and by Back, resized at wide
 * width, floating at medium width, a full-screen sheet on phones, and an
 * invalid `panel` corrected.
 *
 * The file editor is scanned here and in `files.spec.ts`, a session run in
 * `activity.spec.ts`, and the conversation size in `composer.spec.ts`.
 */

const RUN = "Compare fallback strategies for notification delivery";
const RUN_ROW = new RegExp(`^${RUN}`);

/** The panel named `name`: a column beside the main region, or on a phone a full-screen sheet. */
function contextPanel(page: Page, isMobile: boolean, name: string): Locator {
  return page.getByRole(isMobile ? "dialog" : "complementary", { name });
}

/** The panel's way out: Close beside the main region, Back on a phone's sheet. */
function closeButton(panel: Locator, isMobile: boolean): Locator {
  return panel.getByRole("button", { name: isMobile ? "Back" : "Close panel", exact: true });
}

/** The panel's box once it has finished sliding in. */
async function settledBox(
  panel: Locator,
): Promise<{ x: number; y: number; width: number; height: number }> {
  await panel.evaluate((element) =>
    Promise.all(
      element
        .getAnimations({ subtree: true })
        .filter((animation) => animation.effect?.getComputedTiming().endTime !== Infinity)
        .map((animation) => animation.finished),
    ),
  );
  const box = await panel.boundingBox();
  if (box === null) throw new Error("the panel has no box");
  return box;
}

/** Path and query of the page's URL, decoded, so a test can compare them exactly. */
function address(page: Page): string {
  const url = new URL(page.url());
  return decodeURIComponent(`${url.pathname}${url.search}`);
}

async function scanPanel(page: Page, isMobile: boolean): Promise<void> {
  if (isMobile) await expectNoAxeViolations(page, { within: "[data-overlay-host]" });
  else await expectNoAxeViolations(page);
}

test("a run opens in the panel from Activity, and closing it goes back", async ({
  page,
  isMobile,
}) => {
  await page.goto("/agent/atlas/activity");
  const row = page.getByRole("button", { name: RUN_ROW });
  await row.click();

  await expect
    .poll(() => address(page))
    .toBe("/agent/atlas/activity?panel=session:atlas:run-live-research");
  const panel = contextPanel(page, isMobile, RUN);
  await expect(panel).toBeVisible();
  await expect(panel.getByRole("heading", { name: RUN })).toBeVisible();
  await scanPanel(page, isMobile);

  await closeButton(panel, isMobile).click();
  await expect(panel).toBeHidden();
  await expect.poll(() => address(page)).toBe("/agent/atlas/activity");
  await expect(row).toBeFocused();

  // Closing went back over the entry that opened the panel, so Forward opens it again.
  await page.goForward();
  await expect(panel).toBeVisible();
  await expect
    .poll(() => address(page))
    .toBe("/agent/atlas/activity?panel=session:atlas:run-live-research");
});

test("Back closes the panel before it leaves the place", async ({ page, isMobile }) => {
  await page.goto("/home");
  await page.goto("/agent/atlas/activity");
  await page.getByRole("button", { name: RUN_ROW }).click();
  const panel = contextPanel(page, isMobile, RUN);
  await expect(panel).toBeVisible();

  await page.goBack();
  await expect(panel).toBeHidden();
  await expect.poll(() => address(page)).toBe("/agent/atlas/activity");
  await expect(page.getByRole("heading", { name: /^Running now/ })).toBeVisible();

  await page.goBack();
  await expect.poll(() => address(page)).toBe("/home");
});

test("a linked file opens in the panel, and Esc closes it by replacing the URL", async ({
  page,
  isMobile,
}) => {
  await page.goto("/home");
  await page.goto("/agent/atlas?panel=file:team/wiki/index.md");
  const panel = contextPanel(page, isMobile, "index.md");
  await expect(panel).toBeVisible();
  await expect(panel.getByText("team/wiki/index.md").first()).toBeVisible();
  await expect(await expectFileOpen(page, "index.md")).toHaveValue(/# Wiki Index/);
  await scanPanel(page, isMobile);

  await page.keyboard.press("Escape");
  await expect(panel).toBeHidden();
  await expect.poll(() => address(page)).toBe("/agent/atlas");

  // The page didn't open the panel, so closing replaced its entry: Back leaves the place.
  await page.goBack();
  await expect.poll(() => address(page)).toBe("/home");
});

test("on Shared files, a file is read from the team's folder", async ({ page, isMobile }) => {
  await page.goto("/team/files?panel=file:wiki/index.md");
  await expect(contextPanel(page, isMobile, "index.md")).toBeVisible();
  await expect(await expectFileOpen(page, "index.md")).toHaveValue(/# Wiki Index/);
});

// Each URL, and where it is corrected to.
const CORRECTIONS: readonly (readonly [from: string, to: string])[] = [
  ["/agent/atlas?panel=nonsense", "/agent/atlas"],
  ["/agent/atlas?panel=session:atlas", "/agent/atlas"],
  ["/agent/atlas?panel=file:", "/agent/atlas"],
  ["/agent/atlas/schedule?panel=session:scout:run-live-research", "/agent/atlas/schedule"],
  ["/home?panel=size", "/home"],
  ["/team/workbench?panel=file:SOUL.md", "/team/workbench"],
];

// One test for each URL: every one is a full page load, so a single test over all of them spends its timeout on the sum of the loads.
test.describe("a panel value that is invalid, or that its place can't show, is removed", () => {
  for (const [from, to] of CORRECTIONS) {
    test(`${from} is corrected to ${to}`, async ({ page }) => {
      await page.goto(from);
      await expect.poll(() => address(page), from).toBe(to);
      await expect(page.getByRole("main")).toBeVisible();
      // Home has a complementary landmark of its own, so the panel is told apart by its way out.
      await expect(page.getByRole("button", { name: "Close panel", exact: true })).toHaveCount(0);
      await expect(page.locator("[data-overlay-host] dialog")).toHaveCount(0);
    });
  }
});

test("an invalid panel is corrected by replace", async ({ page }) => {
  await page.goto("/home");
  await page.goto("/agent/atlas?panel=nonsense");
  await expect.poll(() => address(page)).toBe("/agent/atlas");
  await page.goBack();
  await expect.poll(() => address(page)).toBe("/home");
});

test.describe("at wide width", () => {
  test.skip(({ isMobile }) => isMobile, "The panel resizes beside the main region at wide width.");

  test("resizes from its edge, by pointer and by key, and remembers the width", async ({
    page,
  }) => {
    await page.goto("/agent/atlas?panel=size");
    const panel = contextPanel(page, false, "Conversation size");
    const grip = panel.getByRole("separator", { name: "Resize panel" });
    await expect(grip).toHaveAttribute("aria-valuenow", "440");
    await expectNoAxeViolations(page);

    // Drag the edge 120px to the left, from where the panel starts.
    const before = await settledBox(panel);
    const y = before.y + before.height / 2;
    await page.mouse.move(before.x, y);
    await page.mouse.down();
    await page.mouse.move(before.x - 120, y, { steps: 6 });
    await page.mouse.up();
    await expect(grip).toHaveAttribute("aria-valuenow", "560");
    expect((await panel.boundingBox())?.width).toBeCloseTo(560, 0);

    // Half the viewport is as wide as it goes.
    await page.mouse.move(before.x - 120, y);
    await page.mouse.down();
    await page.mouse.move(100, y, { steps: 6 });
    await page.mouse.up();
    await expect(grip).toHaveAttribute("aria-valuenow", "720");

    await grip.focus();
    await page.keyboard.press("Home");
    await expect(grip).toHaveAttribute("aria-valuenow", "360");
    await page.keyboard.press("ArrowLeft");
    await expect(grip).toHaveAttribute("aria-valuenow", "376");
    await page.keyboard.press("Shift+ArrowLeft");
    await expect(grip).toHaveAttribute("aria-valuenow", "440");
    await page.keyboard.press("ArrowLeft");
    await expect(grip).toHaveAttribute("aria-valuenow", "456");

    // The width is this viewer's, across a reload and another panel.
    await page.goto("/agent/atlas/files?panel=file:SOUL.md");
    const file = contextPanel(page, false, "SOUL.md");
    await expect(file).toBeVisible();
    expect((await file.boundingBox())?.width).toBeCloseTo(456, 0);
  });

  test("sits beside the main region", async ({ page }) => {
    await page.goto("/agent/atlas?panel=size");
    const panel = contextPanel(page, false, "Conversation size");
    await expect(panel).toBeVisible();
    const side = await settledBox(panel);
    const main = await page.getByRole("main").boundingBox();
    expect(main === null ? null : main.x + main.width).toBeLessThanOrEqual(side.x);
    expect(side.x + side.width).toBe(1440);
  });
});

test.describe("at medium width", () => {
  test.skip(({ isMobile }) => isMobile, "Medium width is a desktop window narrowed.");
  test.use({ viewport: { width: 1000, height: 760 } });

  test("floats over the main region, at its default width, and Esc closes it", async ({ page }) => {
    await page.goto("/agent/atlas/activity");
    await page.getByRole("button", { name: RUN_ROW }).click();
    const panel = contextPanel(page, false, RUN);
    await expect(panel).toBeVisible();
    await expect(panel.getByRole("separator")).toHaveCount(0);

    expect(await settledBox(panel)).toMatchObject({ x: 560, width: 440 });
    const main = await page.getByRole("main").boundingBox();
    // The main region keeps its width under the panel.
    expect(main === null ? null : main.x + main.width).toBe(1000);
    await expectNoAxeViolations(page);

    await panel.getByRole("button", { name: "Close panel" }).focus();
    await page.keyboard.press("Escape");
    await expect(panel).toBeHidden();
    await expect.poll(() => address(page)).toBe("/agent/atlas/activity");
  });
});

test.describe("on a phone", () => {
  test.skip(({ isMobile }) => !isMobile, "The panel is a full-screen sheet on phones.");

  test("is a full-screen sheet over the bottom bar, and Back closes it", async ({ page }) => {
    await page.goto("/agent/atlas");
    await page.goto("/agent/atlas?panel=size");
    const sheet = contextPanel(page, true, "Conversation size");
    await expect(sheet).toBeVisible();
    expect(await settledBox(sheet)).toEqual({ x: 0, y: 0, width: 390, height: 844 });
    // The page under the sheet, the bottom bar with it, is out of reach.
    expect(
      await page.locator(".shell").evaluate((shell) => shell.closest("[inert]") !== null),
    ).toBe(true);
    await scanPanel(page, true);

    await page.goBack();
    await expect(sheet).toBeHidden();
    await expect.poll(() => address(page)).toBe("/agent/atlas");
    await expect(page.getByRole("navigation", { name: "Main" })).toBeVisible();
  });
});

test("on the Workbench, a run on any agent shows in the panel", async ({ page, isMobile }) => {
  await page.goto("/agent/atlas");
  await page.goto("/team/workbench?panel=session:scout:run-live-research");
  const panel = contextPanel(page, isMobile, RUN);
  await expect(panel.getByText("On scout", { exact: true })).toBeVisible();
  await expect(panel.getByText("Starting with what's already in the wiki.")).toBeVisible();
  await scanPanel(page, isMobile);
});
