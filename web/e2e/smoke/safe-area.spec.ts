import type { Locator, Page } from "@playwright/test";
import { expect, test } from "../support/fixtures";
import { expectPaletteOpen, expectSettingsOpen } from "../support/lazy";

/**
 * The safe areas: with `viewport-fit=cover` a phone's notch,
 * status bar and home indicator sit over the page, and the shell and every
 * surface pinned to an edge keep their content out of them. Chromium reports
 * no insets, so each spec sets the four `--safe-*` tokens the stylesheets
 * read: a notched phone in portrait for the phone project, and a phone on its
 * side (insets at the left and right too) for the desktop one.
 */

interface Insets {
  top: number;
  right: number;
  bottom: number;
  left: number;
}

const PORTRAIT: Insets = { top: 47, right: 0, bottom: 34, left: 0 };
const LANDSCAPE: Insets = { top: 24, right: 47, bottom: 21, left: 47 };

const px = (value: number): string => `${String(value)}px`;

async function applyInsets(page: Page, insets: Insets): Promise<void> {
  await page.addStyleTag({
    content: `:root {
      --safe-top: ${px(insets.top)};
      --safe-right: ${px(insets.right)};
      --safe-bottom: ${px(insets.bottom)};
      --safe-left: ${px(insets.left)};
    }`,
  });
}

/** The element's box once its own entrance has finished, so it is where it stays. */
async function settledBox(
  locator: Locator,
): Promise<{ x: number; y: number; width: number; height: number }> {
  await locator.evaluate(async (element) => {
    await Promise.all(element.getAnimations().map((animation) => animation.finished));
  });
  const box = await locator.boundingBox();
  if (box === null) throw new Error("the element has no box on screen");
  return box;
}

function viewport(page: Page): { width: number; height: number } {
  const size = page.viewportSize();
  if (size === null) throw new Error("the page has no viewport");
  return size;
}

let insets = PORTRAIT;

test.beforeEach(async ({ page, isMobile }) => {
  insets = isMobile ? PORTRAIT : LANDSCAPE;
  await page.goto("/agent/atlas");
  await expect(page.getByRole("heading", { name: "atlas", level: 1 })).toBeVisible();
  await applyInsets(page, insets);
});

test("the shell keeps the places inside the insets on every side", async ({ page, isMobile }) => {
  const screen = viewport(page);
  const main = await settledBox(page.getByRole("main"));
  expect(main.y).toBe(insets.top);
  expect(main.x + main.width).toBe(screen.width - insets.right);

  if (isMobile) {
    // The bottom bar covers the home indicator; the place ends where the bar starts.
    const bar = await settledBox(page.getByRole("navigation", { name: "Main" }));
    expect(bar.y + bar.height).toBe(screen.height);
    expect(bar.height).toBe(60 + insets.bottom);
    expect(main.y + main.height).toBe(bar.y);
    const settings = await settledBox(
      page.getByRole("navigation", { name: "Main" }).getByRole("button", { name: "Settings" }),
    );
    expect(settings.y + settings.height).toBeLessThanOrEqual(screen.height - insets.bottom);
  } else {
    const rail = await settledBox(page.getByRole("navigation", { name: "Places and agents" }));
    expect(rail.x).toBe(insets.left);
    expect(main.y + main.height).toBe(screen.height - insets.bottom);
  }
});

test("the context panel keeps inside the insets when it opens", async ({ page, isMobile }) => {
  await page.goto("/agent/atlas/files?panel=file:SOUL.md");
  const panel = page.getByRole(isMobile ? "dialog" : "complementary", { name: "SOUL.md" });
  await expect(panel).toBeVisible();
  await applyInsets(page, insets);

  const screen = viewport(page);
  const box = await settledBox(panel);
  const heading = await settledBox(panel.getByRole("heading", { level: 2 }));
  expect(heading.y).toBeGreaterThanOrEqual(insets.top);
  if (isMobile) {
    // A phone's sheet fills the screen and pads its own content.
    expect(box).toMatchObject({ x: 0, y: 0, width: screen.width, height: screen.height });
  } else {
    expect(box.x + box.width).toBe(screen.width - insets.right);
    expect(box.y + box.height).toBe(screen.height - insets.bottom);
  }
});

test.describe("on a phone", () => {
  test.beforeEach(({ isMobile }) => {
    test.skip(
      !isMobile,
      "The drawer, the full-screen palette and the settings list are the phone's.",
    );
  });

  test("the drawer, the palette and Settings keep their content below the status bar", async ({
    page,
  }) => {
    const bar = page.getByRole("navigation", { name: "Main" });

    await bar.getByRole("button", { name: "Menu" }).click();
    const drawer = page.getByRole("dialog", { name: "Agents and places" });
    await expect(drawer).toBeVisible();
    await settledBox(drawer);
    const search = await settledBox(drawer.getByRole("button", { name: /^Search or jump to/ }));
    expect(search.y).toBeGreaterThanOrEqual(insets.top);
    await page.keyboard.press("Escape");
    await expect(drawer).toBeHidden();

    await bar.getByRole("button", { name: "Search" }).click();
    const palette = await expectPaletteOpen(page);
    await settledBox(palette);
    const field = await settledBox(palette.getByRole("combobox"));
    expect(field.y).toBeGreaterThanOrEqual(insets.top);
    await page.keyboard.press("Escape");
    await expect(palette).toBeHidden();

    await bar.getByRole("button", { name: "Settings" }).click();
    const settings = await expectSettingsOpen(page);
    await settledBox(settings);
    const sections = await settledBox(page.getByRole("navigation", { name: "Settings sections" }));
    expect(sections.y).toBeGreaterThanOrEqual(insets.top);
    expect(sections.y + sections.height).toBeLessThanOrEqual(viewport(page).height - insets.bottom);
  });
});
