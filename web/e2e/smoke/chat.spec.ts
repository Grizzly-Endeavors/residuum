import { sendFromComposer } from "../support/composer";
import { expect, test } from "../support/fixtures";

test("atlas answers a message sent from its chat", async ({ page }) => {
  await page.goto("/agent/atlas");

  await expect(
    page.getByText("Hi, this is atlas. You are in my conversation, not scout's."),
  ).toBeVisible();

  const composer = page.getByRole("combobox", { name: "Message atlas" });
  await composer.fill("What does the observer keep?");
  await sendFromComposer(composer);

  const conversation = page.getByRole("region", { name: "Conversation with atlas" });
  await expect(conversation.getByText("What does the observer keep?")).toBeVisible();
  await expect(
    conversation.getByText("I've looked into that and here's what I found:"),
  ).toBeVisible();
});
