/**
 * What "the app is usable" means to every spec (`support/app.ts`): the harness
 * makes `page.goto` return only once it holds, so these check the state a spec
 * finds when navigation returns.
 */
import { waitForApp } from "../support/app";
import { expect, test } from "../support/fixtures";

test("goto returns with the shell drawn and the hub socket connected", async ({ page }) => {
  await page.goto("/home");
  const shell = page.locator(".shell");
  expect(await shell.getAttribute("data-hub")).toBe("connected");
  expect(await page.evaluate(() => document.fonts.status)).toBe("loaded");
});

test("goto returns with the code a URL asks for already mounted", async ({ page }) => {
  await page.goto("/agent/atlas?settings=atlas/runtime");
  expect(await page.getByRole("dialog", { name: "Settings" }).isVisible()).toBe(true);
});

test("goto returns on the setup wizard, which has no hub socket to wait for", async ({
  page,
  mock,
}) => {
  await mock.post("/api/mock/reset", { data: { setup: true } });
  await page.goto("/");
  expect(await page.locator(".setup-wizard").isVisible()).toBe(true);
});

test("the app is ready again once a dropped hub socket has reconnected", async ({ page, mock }) => {
  await page.goto("/home");
  const shell = page.locator(".shell");

  await mock.post("/api/mock/hub-socket", { data: { online: false } });
  await expect(shell).toHaveAttribute("data-hub", "lost");
  // With the hub down, the screenshot helper's wait names the state it expects.
  await waitForApp(page, { hub: "lost" });

  await mock.post("/api/mock/hub-socket", { data: { online: true } });
  await waitForApp(page);
  expect(await shell.getAttribute("data-hub")).toBe("connected");
});
