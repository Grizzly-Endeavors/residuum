import type { Locator, Page } from "@playwright/test";
import { expect, test } from "../support/fixtures";

/**
 * What an agent has running, said once: the Chat header's pill, the rail's
 * Activity row and Activity's Running now heading show the same number, and
 * the pill goes with the agent.
 */

function pill(page: Page): Locator {
  return page.getByRole("button", { name: /^\d+ running, open Activity$/ });
}

/** The rail's Activity row for atlas, with the rail opened in its drawer on a phone. */
async function railActivity(page: Page, isMobile: boolean): Promise<Locator> {
  if (isMobile) {
    await page
      .getByRole("navigation", { name: "Main" })
      .getByRole("button", { name: "Menu" })
      .click();
    await expect(page.getByRole("dialog", { name: "Agents and places" })).toBeVisible();
  }
  const rail = page.getByRole("navigation", { name: "Places and agents" });
  const agent = rail.getByRole("button", { name: /^atlas\b/ });
  if ((await agent.getAttribute("aria-expanded")) !== "true") await agent.click();
  return rail.getByRole("link", { name: /^Activity/ }).first();
}

/** The number in a name like "5 running" or "Activity, 5 running". */
function runningIn(name: string | null): number {
  const count = /(\d+) running/.exec(name ?? "")?.[1];
  if (count === undefined) throw new Error(`no running count in "${name ?? ""}"`);
  return Number(count);
}

test("the header pill, the rail and Activity count the same runs", async ({ page, isMobile }) => {
  await page.goto("/agent/atlas");
  await expect(pill(page)).toBeVisible();
  const fromPill = runningIn(await pill(page).getAttribute("aria-label"));
  expect(fromPill).toBeGreaterThan(0);

  const activity = await railActivity(page, isMobile);
  expect(runningIn(await activity.textContent())).toBe(fromPill);

  if (isMobile) {
    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog", { name: "Agents and places" })).toBeHidden();
  }
  await pill(page).click();
  await expect(page).toHaveURL(/\/agent\/atlas\/activity$/);
  const heading = page.getByRole("heading", { name: /^Running now/ });
  expect(runningIn(await heading.textContent())).toBe(fromPill);
});

test("the pill goes when the agent stops", async ({ page }) => {
  await page.goto("/agent/atlas");
  await expect(pill(page)).toBeVisible();

  await page.request.post("/api/hub/agents/atlas/stop");
  await expect(page.getByRole("region", { name: "atlas is stopped" })).toBeVisible();
  await expect(pill(page)).toHaveCount(0);
});
