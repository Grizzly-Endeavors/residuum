import type { Locator, Page } from "@playwright/test";

export { FIRST_RECONNECT_MS } from "../../src/lib/reconnect";

export async function sendFromComposer(box: Locator, sendLabel = "Send"): Promise<void> {
  const touch = await box.evaluate(() => window.matchMedia("(pointer: coarse)").matches);
  if (touch) await box.page().getByRole("button", { name: sendLabel, exact: true }).click();
  else await box.press("Enter");
}

export function reconnectingLine(page: Page): Locator {
  return page.getByRole("status").filter({ hasText: /^Reconnecting/ });
}
