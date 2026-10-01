import type { Page } from "@playwright/test";
import { expect, test } from "../support/fixtures";

/**
 * The code that loads only when a feature opens (design §11): Settings, the
 * command palette, the file view with its editor, and the setup wizard. The
 * production build names each chunk after the module it is imported from.
 */

/** The JavaScript files the page has requested from `/assets/`, by file name. */
function trackChunks(page: Page): string[] {
  const requested: string[] = [];
  page.on("request", (request) => {
    const path = new URL(request.url()).pathname;
    if (/^\/assets\/[^/]+\.js$/.test(path)) requested.push(path.slice("/assets/".length));
  });
  return requested;
}

const loaded = (requested: string[], chunk: string): boolean =>
  requested.some((file) => file.startsWith(`${chunk}-`));

test.describe("the production build", { tag: "@preview" }, () => {
  test("loads the Settings code the first time Settings opens, and not before", async ({
    page,
    isMobile,
  }) => {
    const requested = trackChunks(page);
    await page.goto("/agent/atlas");
    await expect(page.getByRole("heading", { name: "atlas", level: 1 })).toBeVisible();
    expect(loaded(requested, "SettingsModal")).toBe(false);

    const control = isMobile
      ? page.getByRole("navigation", { name: "Main" })
      : page.getByRole("navigation", { name: "Places and agents" });
    await control.getByRole("button", { name: "Settings" }).click();
    await expect(page.getByRole("dialog", { name: "Settings" })).toBeVisible();
    expect(loaded(requested, "SettingsModal")).toBe(true);
  });

  test("tells the person when the Settings code can't load, and leaves Settings closed", async ({
    page,
    isMobile,
  }) => {
    await page.goto("/agent/atlas");
    await expect(page.getByRole("heading", { name: "atlas", level: 1 })).toBeVisible();
    await page.route(/\/assets\/SettingsModal-[^/]+\.js$/, (route) => route.abort());

    const control = isMobile
      ? page.getByRole("navigation", { name: "Main" })
      : page.getByRole("navigation", { name: "Places and agents" });
    await control.getByRole("button", { name: "Settings" }).click();

    await expect(page.getByText(/Couldn't open Settings\./)).toBeVisible();
    await expect(page.getByRole("dialog", { name: "Settings" })).toBeHidden();
    // The URL went back to the place, so the next press asks again.
    await expect.poll(() => new URL(page.url()).search).toBe("");
  });

  test("loads the palette's code the first time it opens", async ({ page, isMobile }) => {
    const requested = trackChunks(page);
    await page.goto("/agent/atlas");
    await expect(page.getByRole("heading", { name: "atlas", level: 1 })).toBeVisible();
    expect(loaded(requested, "CommandPalette")).toBe(false);

    if (isMobile) {
      await page
        .getByRole("navigation", { name: "Main" })
        .getByRole("button", { name: "Search" })
        .click();
    } else {
      await page.keyboard.press("ControlOrMeta+k");
    }
    await expect(page.getByRole("dialog", { name: "Search and commands" })).toBeVisible();
    expect(loaded(requested, "CommandPalette")).toBe(true);
  });

  test("loads the file view's code the first time a file opens", async ({ page }) => {
    const requested = trackChunks(page);
    await page.goto("/agent/atlas/files");
    await expect(page.getByRole("button", { name: /memory/ })).toBeVisible();
    expect(loaded(requested, "FilePanel")).toBe(false);

    await page.goto("/agent/atlas/files?panel=file:SOUL.md");
    await expect(page.getByRole("textbox", { name: "Contents of SOUL.md" })).toBeVisible();
    expect(loaded(requested, "FilePanel")).toBe(true);
  });

  test("loads the setup wizard's code only when the hub has no agents", async ({ page, mock }) => {
    const requested = trackChunks(page);
    await page.goto("/home");
    await expect(page.getByRole("heading", { name: "Home", level: 1 })).toBeVisible();
    expect(loaded(requested, "Setup")).toBe(false);

    await mock.post("/api/mock/reset", { data: { setup: true } });
    await page.goto("/");
    await expect(
      page.getByRole("heading", { name: "Welcome to Residuum", level: 1 }),
    ).toBeVisible();
    expect(loaded(requested, "Setup")).toBe(true);
  });
});
