import type { Locator, Page } from "@playwright/test";
import { expectNoAxeViolations } from "../support/axe";
import { expect, test } from "../support/fixtures";

/**
 * The shell: every place from the rail (the drawer on phones) and the phone's
 * bottom bar, the agent accordion, history, the redirects from old URLs, and
 * the hub banner.
 *
 * Places that haven't been rebuilt host their legacy views, which the units
 * that replace them scan, so the scans here leave them out.
 */

const LEGACY = "[data-legacy-view]";

/** The rail, opened in its drawer on a phone. */
async function openRail(page: Page, isMobile: boolean): Promise<Locator> {
  if (isMobile) {
    await page
      .getByRole("navigation", { name: "Main" })
      .getByRole("button", { name: "Menu" })
      .click();
    await expect(page.getByRole("dialog", { name: "Agents and places" })).toBeVisible();
  }
  return page.getByRole("navigation", { name: "Places and agents" });
}

function agentRow(rail: Locator, name: string): Locator {
  return rail.getByRole("button", { name: new RegExp(`^${name}\\b`) });
}

/** Open an agent's places in the rail, unless they already are. */
async function expandAgent(rail: Locator, name: string): Promise<void> {
  const row = agentRow(rail, name);
  if ((await row.getAttribute("aria-expanded")) !== "true") await row.click();
}

/** Path and query of the page's URL, decoded, so a test can compare them exactly. */
function address(page: Page): string {
  const url = new URL(page.url());
  return decodeURIComponent(`${url.pathname}${url.search}`);
}

interface PlaceCase {
  name: string;
  agent?: string;
  link: RegExp;
  path: string;
  shows: (page: Page) => Locator;
}

const PLACES: readonly PlaceCase[] = [
  {
    name: "Inbox",
    link: /^Inbox/,
    path: "/inbox",
    shows: (page) => page.getByRole("region", { name: "Inbox" }),
  },
  {
    name: "Chat",
    agent: "atlas",
    link: /^Chat$/,
    path: "/agent/atlas",
    shows: (page) => page.getByText("Hi, this is atlas. You are in my conversation, not scout's."),
  },
  {
    name: "Activity",
    agent: "atlas",
    link: /^Activity/,
    path: "/agent/atlas/activity",
    shows: (page) => page.getByRole("heading", { name: "Sessions" }),
  },
  {
    name: "Schedule",
    agent: "atlas",
    link: /^Schedule$/,
    path: "/agent/atlas/schedule",
    shows: (page) => page.getByText("Pulses", { exact: true }),
  },
  {
    name: "Files",
    agent: "atlas",
    link: /^Files$/,
    path: "/agent/atlas/files",
    shows: (page) => page.getByRole("button", { name: /memory/ }),
  },
  {
    name: "Workbench",
    link: /^Workbench$/,
    path: "/team/workbench",
    shows: (page) => page.getByRole("heading", { name: "Workbench", level: 1 }),
  },
  {
    name: "Shared files",
    link: /^Shared files$/,
    path: "/team/files",
    shows: (page) => page.getByRole("heading", { name: "Shared files", level: 1 }),
  },
  {
    name: "Home",
    link: /^Home/,
    path: "/home",
    shows: (page) => page.getByRole("region", { name: "Team" }),
  },
];

test("every place opens from the rail, or the drawer on a phone", async ({ page, isMobile }) => {
  await page.goto("/home");
  await expect(page.getByRole("region", { name: "Team" })).toBeVisible();

  for (const place of PLACES) {
    const rail = await openRail(page, isMobile);
    if (place.agent !== undefined) await expandAgent(rail, place.agent);
    await rail.getByRole("link", { name: place.link }).click();

    await expect.poll(() => address(page), place.name).toBe(place.path);
    await expect(place.shows(page), place.name).toBeVisible();
    await expect(page.getByRole("dialog", { name: "Agents and places" })).toBeHidden();
    if (!isMobile) {
      await expect(rail.getByRole("link", { name: place.link })).toHaveAttribute(
        "aria-current",
        "page",
      );
    }
    await expectNoAxeViolations(page, { exclude: LEGACY });
  }
});

test("the drawer opens on the current place, and Esc and Back close it", async ({
  page,
  isMobile,
}) => {
  test.skip(!isMobile, "The drawer is the phone's rail.");
  await page.goto("/agent/atlas/activity");
  const rail = await openRail(page, isMobile);
  await expect(rail.getByRole("link", { name: /^Activity/ })).toBeFocused();
  await expectNoAxeViolations(page);

  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog", { name: "Agents and places" })).toBeHidden();

  await openRail(page, isMobile);
  await page.goBack();
  await expect(page.getByRole("dialog", { name: "Agents and places" })).toBeHidden();
  expect(address(page)).toBe("/agent/atlas/activity");
});

test.describe("the phone's bottom bar", () => {
  test("opens Inbox, Home and Settings", async ({ page, isMobile }) => {
    const bar = page.getByRole("navigation", { name: "Main" });
    await page.goto("/agent/atlas");
    if (!isMobile) {
      await expect(bar).toBeHidden();
      return;
    }

    await bar.getByRole("link", { name: /^Inbox/ }).click();
    await expect.poll(() => address(page)).toBe("/inbox");
    await expect(bar.getByRole("link", { name: /^Inbox/ })).toHaveAttribute("aria-current", "page");

    await bar.getByRole("link", { name: "Home" }).click();
    await expect.poll(() => address(page)).toBe("/home");

    await bar.getByRole("button", { name: "Settings" }).click();
    await expect.poll(() => address(page)).toBe("/home?settings=_all");
    await expect(page.getByRole("dialog", { name: "Settings" })).toBeVisible();
    await page.keyboard.press("Escape");
    await expect.poll(() => address(page)).toBe("/home");
  });
});

test.describe("the agent accordion", () => {
  test("opens one agent at a time, closes the open one on a second press, and never navigates", async ({
    page,
    isMobile,
  }) => {
    await page.goto("/agent/atlas");
    const rail = await openRail(page, isMobile);

    // The viewed agent starts open.
    await expect(agentRow(rail, "atlas")).toHaveAttribute("aria-expanded", "true");
    await expect(rail.getByRole("link", { name: /^Chat$/ })).toHaveAttribute(
      "aria-current",
      "page",
    );

    await agentRow(rail, "scout").click();
    await expect(agentRow(rail, "scout")).toHaveAttribute("aria-expanded", "true");
    await expect(agentRow(rail, "atlas")).toHaveAttribute("aria-expanded", "false");
    await expect(rail.getByRole("link", { name: /^Chat$/ })).toHaveAttribute(
      "href",
      "/agent/scout",
    );

    await agentRow(rail, "scout").click();
    await expect(agentRow(rail, "scout")).toHaveAttribute("aria-expanded", "false");
    await expect(rail.getByRole("link", { name: /^Chat$/ })).toHaveCount(0);

    // Collapsed, the viewed agent keeps its highlight, and the page stays.
    await expect(agentRow(rail, "atlas")).toHaveAttribute("data-viewed");
    expect(address(page)).toBe("/agent/atlas");
    await expectNoAxeViolations(page, { exclude: LEGACY });
  });

  test("Enter and Space toggle a row, and Up and Down move between rows", async ({
    page,
    isMobile,
  }) => {
    await page.goto("/home");
    const rail = await openRail(page, isMobile);
    await agentRow(rail, "atlas").focus();

    await page.keyboard.press("Enter");
    await expect(agentRow(rail, "atlas")).toHaveAttribute("aria-expanded", "true");
    await page.keyboard.press("ArrowDown");
    await expect(rail.getByRole("link", { name: /^Chat$/ })).toBeFocused();
    await page.keyboard.press("ArrowUp");
    await expect(agentRow(rail, "atlas")).toBeFocused();
    await page.keyboard.press(" ");
    await expect(agentRow(rail, "atlas")).toHaveAttribute("aria-expanded", "false");
    await page.keyboard.press("ArrowDown");
    await expect(agentRow(rail, "brittle")).toBeFocused();
    expect(address(page)).toBe("/home");
  });
});

test("Back and Forward move between places", async ({ page, isMobile }) => {
  await page.goto("/home");
  let rail = await openRail(page, isMobile);
  await expandAgent(rail, "atlas");
  await rail.getByRole("link", { name: /^Chat$/ }).click();
  await expect.poll(() => address(page)).toBe("/agent/atlas");

  rail = await openRail(page, isMobile);
  await rail.getByRole("link", { name: /^Activity/ }).click();
  await expect.poll(() => address(page)).toBe("/agent/atlas/activity");

  await page.goBack();
  await expect.poll(() => address(page)).toBe("/agent/atlas");
  await expect(
    page.getByText("Hi, this is atlas. You are in my conversation, not scout's."),
  ).toBeVisible();
  await page.goBack();
  await expect.poll(() => address(page)).toBe("/home");
  await expect(page.getByRole("region", { name: "Team" })).toBeVisible();
  await page.goForward();
  await expect.poll(() => address(page)).toBe("/agent/atlas");
});

// Old URLs, read with no last-used agent saved: it is the first agent by name, atlas.
const REDIRECTS: readonly (readonly [from: string, to: string])[] = [
  ["/", "/home"],
  ["/team", "/home"],
  ["/agent/atlas/sessions/run-live-research", "/agent/atlas?panel=session:atlas:run-live-research"],
  [
    "/agent/atlas/sessions/run-live-research?workspace",
    "/agent/atlas?panel=session:atlas:run-live-research",
  ],
  ["/agent/atlas/workspace", "/agent/atlas/files"],
  ["/agent/atlas?workspace", "/agent/atlas/files"],
  ["/agent/atlas/scheduled", "/agent/atlas/schedule"],
  ["/agent/atlas/settings", "/agent/atlas?settings=atlas"],
  ["/agent/atlas/settings/providers", "/agent/atlas?settings=atlas/model"],
  ["/agent/atlas/settings/cloud", "/agent/atlas?settings=_all/cloud"],
  ["/team/settings", "/home?settings=_all"],
  ["/team/settings/secrets", "/home?settings=_all/keys"],
  ["/settings/memory", "/agent/atlas?settings=atlas/memory"],
  ["/scheduled", "/agent/atlas/schedule"],
  ["/sessions/run-live-research", "/agent/atlas?panel=session:atlas:run-live-research"],
  ["/workbench", "/team/workbench"],
  ["/workbench/tip-splitter", "/team/workbench/tip-splitter"],
  ["/notification/abc123", "/agent/atlas/files"],
];

test("old URLs redirect to where they lead now", async ({ page }) => {
  for (const [from, to] of REDIRECTS) {
    await page.goto(from);
    await expect.poll(() => address(page), from).toBe(to);
    await expect(page.getByRole("main")).toBeVisible();
  }
});

test("a redirect replaces the old URL in the history", async ({ page }) => {
  await page.goto("/home");
  await page.goto("/agent/atlas/scheduled");
  await expect.poll(() => address(page)).toBe("/agent/atlas/schedule");
  await page.goBack();
  await expect.poll(() => address(page)).toBe("/home");
});

test("the hub banner shows while the hub can't be reached, and Retry reconnects", async ({
  page,
  mock,
}) => {
  await page.goto("/agent/atlas");
  await expect(page.getByRole("heading", { name: "atlas", level: 1 })).toBeVisible();
  const banner = page.getByRole("status").filter({ hasText: "Can't reach Residuum." });
  await expect(banner).toBeHidden();

  await mock.post("/api/mock/hub-socket", { data: { online: false } });
  await expect(banner).toBeVisible();
  await expectNoAxeViolations(page, { exclude: LEGACY });

  await mock.post("/api/mock/hub-socket", { data: { online: true } });
  await banner.getByRole("button", { name: "Retry" }).click();
  await expect(banner).toBeHidden();
});

test("Settings opens on the viewed agent's scope, or All agents, and closes back", async ({
  page,
  isMobile,
}) => {
  await page.goto("/agent/atlas");
  let rail = await openRail(page, isMobile);
  await rail.getByRole("button", { name: "Settings" }).click();
  await expect.poll(() => address(page)).toBe("/agent/atlas?settings=atlas");
  await expect(page.getByRole("dialog", { name: "Settings" })).toBeVisible();
  await expect(page.getByRole("dialog", { name: "Agents and places" })).toBeHidden();
  await page.keyboard.press("Escape");
  await expect.poll(() => address(page)).toBe("/agent/atlas");

  rail = await openRail(page, isMobile);
  await rail.getByRole("link", { name: /^Home/ }).click();
  rail = await openRail(page, isMobile);
  await rail.getByRole("button", { name: "Settings" }).click();
  await expect.poll(() => address(page)).toBe("/home?settings=_all");
  await expect(page.getByRole("heading", { name: "All agents" })).toBeVisible();
});

test("the help menu opens Recent notifications and the keyboard shortcuts", async ({
  page,
  isMobile,
}) => {
  await page.goto("/home");
  let rail = await openRail(page, isMobile);
  await rail.getByRole("button", { name: "Help" }).click();
  await expect(page.getByRole("menu", { name: "Help" })).toBeVisible();
  await expectNoAxeViolations(page, { exclude: LEGACY });
  await page.getByRole("menuitem", { name: "Recent notifications" }).click();
  const recent = page.getByRole("dialog", { name: "Recent notifications" });
  await expect(recent).toBeVisible();
  await expectNoAxeViolations(page, { within: "[data-overlay-host]" });
  await page.keyboard.press("Escape");
  await expect(recent).toBeHidden();

  if (isMobile) await page.keyboard.press("Escape");
  rail = await openRail(page, isMobile);
  await rail.getByRole("button", { name: "Help" }).click();
  await page.getByRole("menuitem", { name: "Keyboard shortcuts" }).click();
  await expect(page.getByRole("dialog", { name: "Help" })).toBeVisible();
});

test("setup mode has no shell, and ? doesn't open the shortcuts there", async ({ page, mock }) => {
  await mock.post("/api/mock/reset", { data: { setup: true } });
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Welcome to Residuum", level: 1 })).toBeVisible();
  await expect(page.getByRole("navigation", { name: "Places and agents" })).toHaveCount(0);

  await page.locator("body").press("?");
  await expect(page.getByRole("dialog", { name: "Help" })).toHaveCount(0);
});
