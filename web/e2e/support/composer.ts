import type { Locator, Page } from "@playwright/test";

/**
 * The chat socket's first reconnect delay. In manual time the page's timers
 * wait for the mock's time, so a spec moves time this far for the page to come
 * back.
 */
export { FIRST_RECONNECT_MS } from "../../src/lib/reconnect";

/**
 * Send what is in a message box the way the device sends it: Enter where
 * there is a keyboard, the send button on a touch screen, where Enter starts
 * a new line. `sendLabel` names that button: the chat's Send, or a session
 * box's own, such as "Send to this session".
 */
export async function sendFromComposer(box: Locator, sendLabel = "Send"): Promise<void> {
  const touch = await box.evaluate(() => window.matchMedia("(pointer: coarse)").matches);
  if (touch) await box.page().getByRole("button", { name: sendLabel, exact: true }).click();
  else await box.press("Enter");
}

/**
 * The composer's "Reconnecting" line, which shows from the moment the page
 * sees its chat socket close, when it also sets the timer for its next try,
 * until the socket is open again.
 */
export function reconnectingLine(page: Page): Locator {
  return page.getByRole("status").filter({ hasText: /^Reconnecting/ });
}
