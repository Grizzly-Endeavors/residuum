import type { Locator, Page } from "@playwright/test";
import { expectNoAxeViolations } from "../support/axe";
import { expect, test } from "../support/fixtures";

/**
 * An agent's Schedule: its pulses and scheduled actions. The scenario gives
 * atlas three pulses (Inbox check on, Nightly review paused, Legacy identity
 * failed to load) and two actions (Weekly digest, Review open prs); drifter
 * is stopped and has never run, and gets the same schedule once started.
 */

function pulses(page: Page): Locator {
  return page.getByRole("region", { name: "Pulses" });
}

function actions(page: Page): Locator {
  return page.getByRole("region", { name: "Scheduled actions" });
}

function row(section: Locator, title: string): Locator {
  return section.getByRole("listitem").filter({ hasText: title });
}

test("the Schedule lists pulses and actions with their next runs", async ({ page }) => {
  await page.goto("/agent/atlas/schedule");

  const inbox = row(pulses(page), "Inbox check");
  await expect(inbox.getByText("Every 2 hours, between 08:00 and 22:00")).toBeVisible();
  await expect(inbox.getByText(/^Last ran /)).toBeVisible();
  await expect(row(pulses(page), "Nightly review").getByText("Paused")).toBeVisible();
  await expect(
    row(pulses(page), "Legacy identity").getByRole("switch", { name: "Legacy identity" }),
  ).toBeDisabled();
  await expect(row(actions(page), "Review open prs").getByText("Skill: researcher")).toBeVisible();
  await expectNoAxeViolations(page);
});

test("a pulse's switch pauses and resumes it", async ({ page }) => {
  await page.goto("/agent/atlas/schedule");
  const inbox = row(pulses(page), "Inbox check");
  const toggle = inbox.getByRole("switch", { name: "Inbox check" });
  await expect(toggle).toHaveAttribute("aria-checked", "true");

  await toggle.click();
  await expect(toggle).toHaveAttribute("aria-checked", "false");
  await expect(inbox.getByText("Paused")).toBeVisible();

  // The change was saved, not just shown.
  await page.reload();
  await expect(toggle).toHaveAttribute("aria-checked", "false");

  await toggle.click();
  await expect(toggle).toHaveAttribute("aria-checked", "true");
  await expect(inbox.getByText("Paused")).toHaveCount(0);
});

test("Cancel removes a scheduled action", async ({ page }) => {
  await page.goto("/agent/atlas/schedule");
  await expect(row(actions(page), "Weekly digest")).toBeVisible();

  await page.getByRole("button", { name: "Cancel Weekly digest" }).click();
  await expect(row(actions(page), "Weekly digest")).toHaveCount(0);
  await expect(row(actions(page), "Review open prs")).toBeVisible();

  await page.reload();
  await expect(row(actions(page), "Review open prs")).toBeVisible();
  await expect(row(actions(page), "Weekly digest")).toHaveCount(0);
});

test("a failed load shows the error and Try again, never the empty lists", async ({ page }) => {
  await page.route("**/api/agents/atlas/scheduled/pulses", (route) =>
    route.fulfill({ status: 500, json: { error: "boom" } }),
  );
  await page.goto("/agent/atlas/schedule");

  const alert = page.getByRole("alert").filter({ hasText: "Couldn't load the schedule." });
  await expect(alert).toBeVisible();
  await expect(page.getByText(/No pulses yet/)).toHaveCount(0);
  await expect(page.getByText(/Nothing scheduled/)).toHaveCount(0);
  await expectNoAxeViolations(page);

  await page.unroute("**/api/agents/atlas/scheduled/pulses");
  await alert.getByRole("button", { name: "Try again" }).click();
  await expect(alert).toHaveCount(0);
  await expect(row(pulses(page), "Inbox check")).toBeVisible();
});

test("a stopped agent's Schedule offers Start, and loads once it runs", async ({ page }) => {
  await page.goto("/agent/drifter/schedule");
  await expect(page.getByRole("heading", { name: "drifter is stopped" })).toBeVisible();
  await expect(page.getByText(/Couldn't load the schedule/)).toHaveCount(0);
  await expectNoAxeViolations(page);

  // An agent the mock starts for the first time gets the sample schedule.
  await page.getByRole("button", { name: "Start drifter" }).click();
  await expect(row(pulses(page), "Inbox check")).toBeVisible();
  await expect(row(actions(page), "Weekly digest")).toBeVisible();
  await expect(page.getByRole("heading", { name: "drifter is stopped" })).toHaveCount(0);
});

test("an edit to HEARTBEAT.yml reloads the Schedule", async ({ page, mock }) => {
  let loads = 0;
  page.on("request", (request) => {
    if (request.url().endsWith("/api/agents/atlas/scheduled/pulses")) loads += 1;
  });
  await page.goto("/agent/atlas/schedule");
  await expect(row(pulses(page), "Inbox check")).toBeVisible();
  const before = loads;

  // The watch goes out once the socket opens, so edit until a change lands.
  let edit = 0;
  await expect
    .poll(async () => {
      edit += 1;
      await mock.post("/api/mock/agent-file", {
        params: { agent: "atlas" },
        data: { path: "HEARTBEAT.yml", content: `# edit ${String(edit)}\npulses: []\n` },
      });
      return loads;
    })
    .toBeGreaterThan(before);
});
