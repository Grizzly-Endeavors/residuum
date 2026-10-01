import type { Page } from "@playwright/test";
import type { AppReadyOptions } from "../support/app";
import { expect, test } from "../support/fixtures";
import { expectPaletteOpen } from "../support/lazy";
import { expectScreenshot } from "../support/screenshot";

/**
 * The shell's baselines. A hosted legacy view is painted over: its own unit
 * gives it baselines when it is rebuilt. Home's baselines, with every agent
 * closed in the rail, are in `home.spec.ts`.
 */

async function shellScreenshot(
  page: Page,
  name: string,
  options: AppReadyOptions = {},
): Promise<void> {
  await expectScreenshot(page, name, { ...options, mask: [page.locator("[data-legacy-view]")] });
}

test.describe("shell", { tag: "@visual" }, () => {
  test("on an agent's chat", async ({ page }) => {
    await page.goto("/agent/atlas");
    await expect(
      page.getByText("Hi, this is atlas. You are in my conversation, not scout's."),
    ).toBeVisible();
    await shellScreenshot(page, "shell-chat");
  });

  test("with the hub out of reach", async ({ page, mock }) => {
    await page.goto("/agent/atlas/activity");
    await expect(page.getByRole("heading", { name: /^Running now/ })).toBeVisible();
    await mock.post("/api/mock/hub-socket", { data: { online: false } });
    await expect(page.getByText("Can't reach Residuum.")).toBeVisible();
    // Retry shows a spinner while the socket makes an attempt, so shoot between two.
    await expect(page.getByRole("button", { name: "Retry" })).not.toHaveAttribute(
      "aria-busy",
      "true",
    );
    await shellScreenshot(page, "shell-offline", { hub: "lost" });
  });

  test("the help menu", async ({ page, isMobile }) => {
    await page.goto("/agent/atlas/files");
    await expect(page.getByRole("button", { name: /memory/ })).toBeVisible();
    if (isMobile) {
      await page
        .getByRole("navigation", { name: "Main" })
        .getByRole("button", { name: "Menu" })
        .click();
    }
    await page
      .getByRole("navigation", { name: "Places and agents" })
      .getByRole("button", { name: "Help" })
      .click();
    await expect(page.getByRole("menu", { name: "Help" })).toBeVisible();
    // A mask paints over everything in its box, the drawer included, so the
    // phone's drawer shot leaves the dimmed place behind it unmasked.
    if (isMobile) await expectScreenshot(page, "shell-help-menu");
    else await shellScreenshot(page, "shell-help-menu");
  });

  // An overlay's shot is unmasked: a mask paints over everything in its box,
  // the overlay included, and the scrim dims the place behind it. The place is
  // the Schedule, which lays out the same on every run; the chat doesn't.

  test("the command palette", async ({ page, isMobile }) => {
    await page.goto("/agent/atlas/schedule");
    await expect(page.getByRole("heading", { name: "Pulses" })).toBeVisible();
    if (isMobile) {
      await page
        .getByRole("navigation", { name: "Main" })
        .getByRole("button", { name: "Search" })
        .click();
    } else {
      await page.keyboard.press("ControlOrMeta+k");
    }
    await expectPaletteOpen(page);
    await expectScreenshot(page, "shell-palette");
  });

  test("the keyboard shortcuts", async ({ page }) => {
    await page.goto("/agent/atlas/schedule");
    await expect(page.getByRole("heading", { name: "Pulses" })).toBeVisible();
    await page.locator("body").press("?");
    await expect(page.getByRole("dialog", { name: "Keyboard shortcuts" })).toBeVisible();
    await expectScreenshot(page, "shell-shortcuts");
  });

  test("the bug report", async ({ page, isMobile }) => {
    await page.goto("/agent/atlas/schedule");
    await expect(page.getByRole("heading", { name: "Pulses" })).toBeVisible();
    if (isMobile) {
      await page
        .getByRole("navigation", { name: "Main" })
        .getByRole("button", { name: "Menu" })
        .click();
    }
    await page
      .getByRole("navigation", { name: "Places and agents" })
      .getByRole("button", { name: "Help" })
      .click();
    await page.getByRole("menuitem", { name: "Report a bug" }).click();
    await expect(page.getByRole("dialog", { name: "Report a bug" })).toBeVisible();
    await expectScreenshot(page, "shell-bug-report");
  });

  test("the Add to Home Screen steps", async ({ page, isMobile }) => {
    // An iPhone has no install prompt, so Install app opens these steps.
    await page.addInitScript(() => {
      Object.defineProperty(navigator, "userAgent", {
        value:
          "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 Version/18.0 Mobile/15E148 Safari/604.1",
      });
    });
    await page.goto("/agent/atlas/schedule");
    await expect(page.getByRole("heading", { name: "Pulses" })).toBeVisible();
    if (isMobile) {
      await page
        .getByRole("navigation", { name: "Main" })
        .getByRole("button", { name: "Menu" })
        .click();
    }
    await page
      .getByRole("navigation", { name: "Places and agents" })
      .getByRole("button", { name: "Help" })
      .click();
    await page.getByRole("menuitem", { name: "Install app" }).click();
    await expect(
      page.getByRole("dialog", { name: "Add Residuum to your Home Screen" }),
    ).toBeVisible();
    await expectScreenshot(page, "shell-install-steps");
  });

  test("at medium width", async ({ page, isMobile }) => {
    test.skip(isMobile, "Medium width is a desktop window narrowed.");
    await page.setViewportSize({ width: 1000, height: 760 });
    await page.goto("/agent/scout/schedule");
    await expect(page.getByRole("heading", { name: "Pulses" })).toBeVisible();
    await shellScreenshot(page, "shell-medium");
  });
});
