import type { Locator, Page } from "@playwright/test";
import { expectNoAxeViolations } from "../support/axe";
import { expect, test } from "../support/fixtures";

/**
 * Activity and the session panel: what an agent is running and has finished,
 * a run opened in the panel from Activity, a chat card and the Workbench,
 * messaging, stopping and resuming a run, a run on another agent streaming
 * in through the hub's relay, outbound tasks, and the panel keeping its
 * state across a breakpoint.
 */

const RESEARCH = "Compare fallback strategies for notification delivery";
const FAMILY = "Conversation in Family chat";

/** Path and query of the page's URL, decoded. */
function address(page: Page): string {
  const url = new URL(page.url());
  return decodeURIComponent(`${url.pathname}${url.search}`);
}

/** The panel named `name`: a column beside the main region, or on a phone a full-screen sheet. */
function sessionPanel(page: Page, isMobile: boolean, name: string): Locator {
  return page.getByRole(isMobile ? "dialog" : "complementary", { name });
}

function messageBox(panel: Locator): Locator {
  return panel.getByRole("textbox", { name: "Message this session" });
}

async function scanPanel(page: Page, isMobile: boolean): Promise<void> {
  if (isMobile) await expectNoAxeViolations(page, { within: "[data-overlay-host]" });
  else await expectNoAxeViolations(page);
}

test("Activity lists what's running and what finished, and opens a run in the panel", async ({
  page,
  isMobile,
}) => {
  await page.goto("/agent/atlas/activity");
  const running = page.getByRole("region", { name: /^Running now/ });
  await expect(running.getByRole("button", { name: new RegExp(`^${RESEARCH}`) })).toContainText(
    "Started by atlas",
  );
  await expect(running.getByText("Sent to research-buddy")).toBeVisible();
  await expect(running.getByText(/^Can't reach laptop for /)).toBeVisible();
  const finished = page.getByRole("region", { name: /^Finished/ });
  await expect(finished.getByRole("heading")).toHaveText("Finished 25+");
  await expectNoAxeViolations(page);

  await running.getByRole("button", { name: new RegExp(`^${RESEARCH}`) }).click();
  await expect
    .poll(() => address(page))
    .toBe("/agent/atlas/activity?panel=session:atlas:run-live-research");
  const panel = sessionPanel(page, isMobile, RESEARCH);
  await expect(panel.getByText("Starting with what's already in the wiki.")).toBeVisible();
  await panel.getByRole("button", { name: "Details" }).click();
  await expect(panel.getByText("run-live-research")).toBeVisible();
  await scanPanel(page, isMobile);
});

test("the Finished list filters by kind and pages in older runs", async ({ page }) => {
  await page.goto("/agent/atlas/activity");
  const finished = page.getByRole("region", { name: /^Finished/ });
  const rows = finished.getByRole("listitem");
  await expect(rows).toHaveCount(25);
  await finished.getByRole("button", { name: "Show older" }).click();
  await expect(rows).toHaveCount(33);
  await expect(finished.getByRole("button", { name: "Show older" })).toHaveCount(0);

  await finished.getByRole("combobox", { name: "Kind of run" }).selectOption("scheduled");
  await expect(rows.first()).toContainText("Scheduled");
  for (const row of await rows.all()) await expect(row).toContainText("Scheduled");
});

test("a live run is messaged and stopped from the panel", async ({ page, isMobile }) => {
  await page.goto(`/agent/atlas/activity?panel=session:atlas:run-live-research`);
  const panel = sessionPanel(page, isMobile, RESEARCH);
  await expect(panel.getByText("Starting with what's already in the wiki.")).toBeVisible();

  await messageBox(panel).fill("Weigh safety over speed");
  await messageBox(panel).press("Enter");
  await expect(panel.getByText("Delivered.")).toBeVisible();
  await expect(panel.getByText('Understood: "Weigh safety over speed".')).toBeVisible();
  await expect(messageBox(panel)).toHaveValue("");

  await panel.getByRole("button", { name: "Stop", exact: true }).click();
  await expect(panel.getByText("Session stopped.")).toBeVisible();
  await expect(panel.getByRole("button", { name: "Stop", exact: true })).toHaveCount(0);
  await expect(messageBox(panel)).toHaveAttribute("placeholder", "Message it to start it again…");
  // Activity moved it to Finished.
  if (!isMobile) {
    await expect(
      page.getByRole("region", { name: /^Finished/ }).getByRole("button", { name: /^Compare/ }),
    ).toContainText("Stopped after");
  }
});

test("a finished run is resumed by a message, and the panel follows it", async ({
  page,
  isMobile,
}) => {
  await page.goto("/agent/atlas/activity");
  await page
    .getByRole("region", { name: /^Finished/ })
    .getByRole("button", { name: new RegExp(`^${FAMILY}`) })
    .click();
  const panel = sessionPanel(page, isMobile, FAMILY);
  await expect(panel.getByText("Done. Nothing needed your attention.")).toBeVisible();

  await messageBox(panel).fill("Any news?");
  await messageBox(panel).press("Enter");
  await expect(
    panel.getByText("This session had finished, so your message started a new run."),
  ).toBeVisible();
  await expect(panel.getByText("Picking this back up.")).toBeVisible();
  await expect.poll(() => address(page)).toMatch(/\?panel=session:atlas:run-resumed-http-\d+$/);
  await expect(panel.getByRole("separator", { name: "New run" })).toBeAttached();
});

test("a chat card's Open session opens its run in the panel", async ({ page, isMobile }) => {
  await page.goto("/agent/atlas");
  await page
    .getByRole("article", { name: "Background session: spawned-research-3f9a" })
    .getByRole("button", { name: "Open session" })
    .click();
  await expect.poll(() => address(page)).toBe("/agent/atlas?panel=session:atlas:run-live-research");
  await expect(
    sessionPanel(page, isMobile, RESEARCH).getByText("Starting with what's already in the wiki."),
  ).toBeVisible();
});

test("a run on another agent streams into the panel through the hub", async ({
  page,
  isMobile,
}) => {
  // scout's socket is the one open: atlas's run comes through the hub's relay.
  await page.goto("/agent/scout");
  await page.goto("/team/workbench?panel=session:atlas:run-live-research");
  const panel = sessionPanel(page, isMobile, RESEARCH);
  await expect(panel.getByText("On atlas", { exact: true })).toBeVisible();
  await expect(panel.getByText("Starting with what's already in the wiki.")).toBeVisible();

  await messageBox(panel).fill("Hello from the bench");
  await messageBox(panel).press("Enter");
  await expect(panel.getByText('Understood: "Hello from the bench".')).toBeVisible();
  await expect(panel.getByText("Checking the notes first.")).toBeVisible();
});

test("a lagging relay reads the run again", async ({ page, mock, isMobile }) => {
  await page.goto("/agent/atlas/activity?panel=session:atlas:run-live-research");
  const panel = sessionPanel(page, isMobile, RESEARCH);
  await expect(panel.getByText("Starting with what's already in the wiki.")).toBeVisible();
  const reread = page.waitForRequest((request) =>
    request.url().endsWith("/api/agents/atlas/sessions/runs/run-live-research/transcript"),
  );
  await mock.post("/api/mock/session-relay-lag");
  await reread;
  await expect(panel.getByText("Starting with what's already in the wiki.")).toBeVisible();
});

test("an unreachable task can't be stopped, so it is stopped watching", async ({ page }) => {
  await page.goto("/agent/atlas/activity");
  const running = page.getByRole("region", { name: /^Running now/ });
  await running.getByRole("button", { name: "Stop the task sent to laptop" }).click();
  await expect(running.getByText(/^Couldn't reach laptop to cancel the task\./)).toBeVisible();

  await running.getByRole("button", { name: "Stop watching the task sent to laptop" }).click();
  await expect(running.getByText("Sent to laptop")).toHaveCount(0);
  await expect(running.getByText("Sent to research-buddy")).toBeVisible();
});

test.describe("at wide width", () => {
  test.skip(({ isMobile }) => isMobile, "The panel changes layout as a desktop window narrows.");

  test("the panel keeps its run and draft across breakpoints", async ({ page }) => {
    await page.goto("/agent/atlas/activity?panel=session:atlas:run-live-research");
    const transcripts: string[] = [];
    const panel = (name: "complementary" | "dialog"): Locator =>
      page.getByRole(name, { name: RESEARCH });
    await expect(panel("complementary").getByText("Starting with what's")).toBeVisible();
    // Past the reads the panel makes as it subscribes.
    await page.waitForLoadState("networkidle");
    page.on("request", (request) => {
      if (request.url().includes("/transcript")) transcripts.push(request.url());
    });
    await messageBox(panel("complementary")).fill("A draft in progress");

    await page.setViewportSize({ width: 1000, height: 760 });
    await expect(messageBox(panel("complementary"))).toHaveValue("A draft in progress");
    await page.setViewportSize({ width: 390, height: 844 });
    await expect(messageBox(panel("dialog"))).toHaveValue("A draft in progress");
    await expect(panel("dialog").getByText("Starting with what's")).toBeVisible();
    expect(transcripts).toEqual([]);
  });
});

test.describe("on a phone", () => {
  test.skip(({ isMobile }) => !isMobile, "The panel is a full-screen sheet on phones.");

  test("a run is a full-screen sheet, and Back closes it", async ({ page }) => {
    await page.goto("/agent/atlas/activity");
    await page.getByRole("button", { name: new RegExp(`^${RESEARCH}`) }).click();
    const sheet = sessionPanel(page, true, RESEARCH);
    await expect(sheet.getByText("Starting with what's already in the wiki.")).toBeVisible();
    await expect
      .poll(async () => sheet.boundingBox())
      .toEqual({
        x: 0,
        y: 0,
        width: 390,
        height: 844,
      });
    await page.goBack();
    await expect(sheet).toBeHidden();
    await expect.poll(() => address(page)).toBe("/agent/atlas/activity");
  });
});
