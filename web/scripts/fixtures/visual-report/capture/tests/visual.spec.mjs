// The tests capture.mjs runs. Each scenario selects some of them with --grep and
// sets CAPTURE_MODE, which decides which shots are drawn differently from their
// baselines. The titles and shot names are the ones visual-report.test.ts asserts.

import { expect, test } from "@playwright/test";

const MODE = process.env.CAPTURE_MODE ?? "base";

const box = (color) => `<div id="t" style="width:100px;height:50px;background:${color}"></div>`;

// A box whose colour changes every millisecond, so no two screenshots match and
// the shot never settles, with or without a baseline.
const FLICKER =
  '<div id="t" style="width:100px;height:50px"></div><script>setInterval(()=>{document.getElementById("t").style.background="#"+Math.floor(Math.random()*0xffffff).toString(16).padStart(6,"0")},1)</script>';

// Whether a shot is drawn differently from its baseline on this run.
function changed(testInfo, shot) {
  switch (MODE) {
    case "differ":
      return shot === "chat-turn" || shot === "composer";
    case "mixed":
      return shot === "chat-turn";
    case "retries":
      return shot === "message-details" || (shot === "composer" && testInfo.retry === 0);
    case "repeat":
      return shot === "chat-turn" && testInfo.repeatEachIndex === 1;
    default:
      return false;
  }
}

test.describe("chat", () => {
  test("a finished turn's activity line, open to a step's details", async ({ page }, testInfo) => {
    await page.setContent(box(changed(testInfo, "chat-turn") ? "blue" : "green"));
    await expect(page.locator("#t")).toHaveScreenshot("chat-turn.png");
  });

  test("the composer", async ({ page }, testInfo) => {
    await page.setContent(box(changed(testInfo, "composer") ? "blue" : "red"));
    await expect(page.locator("#t")).toHaveScreenshot("composer.png");
  });

  test("a message's details", async ({ page }, testInfo) => {
    await page.setContent(box(changed(testInfo, "message-details") ? "orange" : "teal"));
    await expect(page.locator("#t")).toHaveScreenshot("message-details.png");
  });
});

test.describe("gallery", () => {
  test("the open menu", async ({ page }) => {
    test.skip(MODE === "base", "no baseline is written for this shot");
    await page.setContent(box("green"));
    await expect(page.locator("#t")).toHaveScreenshot("gallery-menu.png");
  });
});

test.describe("settings", () => {
  test("a settings section", async ({ page }) => {
    test.setTimeout(1000);
    if (MODE === "mixed") {
      await new Promise(() => {});
    }
    await page.setContent(box("purple"));
    await expect(page.locator("#t")).toHaveScreenshot("settings-section.png");
  });
});

test.describe("shell", () => {
  test("the navigation", async ({ page }) => {
    await page.setContent(MODE === "mixed" ? FLICKER : box("navy"));
    await expect(page.locator("#t")).toHaveScreenshot("shell-nav.png");
  });

  test("the header", async ({ page }) => {
    test.skip(MODE === "base", "no baseline is written for this shot");
    await page.setContent(MODE === "mixed" ? FLICKER : box("olive"));
    await expect(page.locator("#t")).toHaveScreenshot("shell-header.png");
  });

  test("the missing element", async ({ page }) => {
    test.skip(MODE === "base", "the locator is never on the page, so there is no baseline");
    await page.setContent("<div></div>");
    await expect(page.locator("#nope")).toHaveScreenshot("missing-element.png");
  });

  test("the phone layout", () => {
    test.skip(true, "phone");
  });
});
