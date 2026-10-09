import type { Locator } from "@playwright/test";

/**
 * Send what is in the chat's message box the way the device sends it: Enter
 * where there is a keyboard, the Send button on a touch screen, where Enter
 * starts a new line.
 */
export async function sendFromComposer(box: Locator): Promise<void> {
  const touch = await box.evaluate(() => window.matchMedia("(pointer: coarse)").matches);
  if (touch) await box.page().getByRole("button", { name: "Send" }).click();
  else await box.press("Enter");
}
