/**
 * The harness's own guarantees: what every spec relies on without saying so.
 */
import { expect, test } from "../support/fixtures";

// The second test only proves the reset if it runs after the first.
test.describe("the mock is reset before each test", () => {
  test.describe.configure({ mode: "serial" });

  test("a test moves the mock clock", async ({ mock }) => {
    const answer = await mock.post("/api/mock/clock/advance", { data: { ms: 3_600_000 } });
    expect(answer).toEqual({ now: "2026-03-14T13:00:00.000Z" });
  });

  test("the next test finds it back at the fixed time", async ({ mock }) => {
    const answer = await mock.post("/api/mock/clock/advance", { data: { ms: 0 } });
    expect(answer).toEqual({ now: "2026-03-14T12:00:00.000Z" });
  });
});

test("a test control stages a teammate's message in the chat", async ({ page, mock }) => {
  await mock.post("/api/mock/teammate-message", { params: { agent: "atlas" } });
  await page.goto("/agent/atlas");
  await expect(page.getByText("scout asked me to check the wiki index")).toBeVisible();
});

test("the page can't reach beyond loopback", async ({ page }) => {
  await page.goto("/");
  const outcome = await page.evaluate(() =>
    fetch("https://example.com/", { mode: "no-cors" }).then(
      () => "reached",
      () => "blocked",
    ),
  );
  expect(outcome).toBe("blocked");
});

test.describe("with a frozen clock", () => {
  test.use({ frozenClock: true });

  test("the page reads the mock's time", async ({ page }) => {
    await page.goto("/");
    expect(await page.evaluate(() => new Date().toISOString())).toBe("2026-03-14T12:00:00.000Z");
  });
});

test("manual time holds the mock still until the spec moves it, and steps until the page shows the outcome", async ({
  page,
  mock,
}) => {
  await page.goto("/agent/atlas");
  await expect(
    page
      .getByRole("region", { name: "Conversation with atlas" })
      .getByText("Hi, this is atlas. You are in my conversation, not scout's."),
  ).toBeVisible();
  await mock.manualTime();
  await page.getByRole("button", { name: "More for atlas" }).click();
  await page.getByRole("menuitem", { name: /^Stop atlas/ }).click();
  const stopping = page.getByRole("region", { name: "Stopping atlas" });
  await expect(stopping).toBeVisible();

  // The stop's wind-down waits on simulated time, which a move of none doesn't pass.
  expect(await mock.advance(0)).toMatchObject({ fired: 0 });
  await expect(stopping).toBeVisible();

  await mock.stepUntil((timeout) =>
    expect(page.getByRole("region", { name: "atlas is stopped" })).toBeVisible({ timeout }),
  );
});
