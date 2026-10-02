import { expect, test } from "../support/fixtures";

test("atlas answers a message sent from its chat", async ({ page }) => {
  await page.goto("/agent/atlas");

  // atlas's own conversation, not scout's. A message sent before the socket
  // is up queues and goes out once it connects.
  await expect(
    page.getByText("Hi, this is atlas. You are in my conversation, not scout's."),
  ).toBeVisible();

  const composer = page.getByRole("textbox", { name: "Message atlas" });
  await composer.fill("What does the observer keep?");
  await composer.press("Enter");

  await expect(page.getByText("What does the observer keep?")).toBeVisible();
  // The mock's first canned reply; a reset before each test makes it the first.
  await expect(page.getByText("I've looked into that and here's what I found:")).toBeVisible();
});
