/**
 * The accessibility helper against small pages built for it, so its verdicts
 * don't depend on the app.
 */
import { expectNoAxeViolations } from "../support/axe";
import { expect, test } from "../support/fixtures";

const CLEAN_PAGE = `<!doctype html><html lang="en"><head><title>Clean</title></head>
  <body><main><h1>Clean</h1><button type="button">Save</button></main></body></html>`;

// An image with no text alternative: critical, rule `image-alt`.
const PAGE_WITH_UNLABELED_IMAGE = `<!doctype html><html lang="en"><head><title>Broken</title></head>
  <body><main><h1>Broken</h1><img src="data:image/gif;base64,R0lGODlhAQABAAAAACw="></main></body></html>`;

test("a page with no violations passes", async ({ page }) => {
  await page.setContent(CLEAN_PAGE);
  await expectNoAxeViolations(page);
});

test("a critical violation fails and names the rule and the element", async ({ page }) => {
  await page.setContent(PAGE_WITH_UNLABELED_IMAGE);
  await expect(expectNoAxeViolations(page)).rejects.toThrow(/image-alt \[critical\].*img/s);
});

test("the scan can be limited to one region", async ({ page }) => {
  await page.setContent(PAGE_WITH_UNLABELED_IMAGE);
  await expectNoAxeViolations(page, { within: "h1" });
});
