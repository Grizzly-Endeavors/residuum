import type { Page } from "@playwright/test";
import { expect, test } from "../support/fixtures";
import { expectPaletteOpen } from "../support/lazy";

/**
 * What the page says when the agent reloads its settings: nothing, for the
 * reload the model control asks for (its popover already says a change applies
 * from the next reply), and plain words for a reload the user asked for.
 */

/** The frames the agent's socket delivers, by type, so a spec can tell a message has arrived. */
function watchAgentFrames(page: Page): string[] {
  const seen: string[] = [];
  page.on("websocket", (socket) => {
    if (!/\/api\/agents\/atlas\/ws$/.test(socket.url())) return;
    socket.on("framereceived", ({ payload }) => {
      if (typeof payload !== "string") return;
      const frame = JSON.parse(payload) as { type?: string; message?: string };
      seen.push(frame.type === "notice" ? `notice: ${frame.message ?? ""}` : (frame.type ?? ""));
    });
  });
  return seen;
}

test("changing the model reloads the agent without announcing it", async ({ page }) => {
  const frames = watchAgentFrames(page);
  await page.goto("/agent/atlas");
  await page.getByRole("button", { name: /^Model: Claude Sonnet 4\.6/ }).click();
  const chooser = page.getByRole("dialog", { name: "Model for atlas" });
  await chooser.getByRole("button", { name: "Claude Haiku 4.5" }).click();
  await expect(page.getByRole("button", { name: /^Model: Claude Haiku 4\.5/ })).toBeAttached();

  // The agent has said it is reloading and how that went, and the page has said neither.
  await expect
    .poll(() => frames.some((frame) => /^notice: Configuration reloaded/.test(frame)))
    .toBe(true);
  expect(frames).toContain("reloading");
  await expect(page.getByText(/reloading|reloaded/i)).toHaveCount(0);
});

test("a reload the user asks for says so in plain words", async ({ page, isMobile }) => {
  await page.goto("/agent/atlas");
  if (isMobile) {
    await page
      .getByRole("navigation", { name: "Main" })
      .getByRole("button", { name: "Search" })
      .click();
  } else {
    await page.keyboard.press("ControlOrMeta+k");
  }
  const dialog = await expectPaletteOpen(page);
  await page.keyboard.type("reload settings");
  await dialog.getByRole("option", { name: /Reload settings/ }).click();

  await expect(page.getByText("Reloading settings…")).toBeVisible();
  await expect(page.getByText("Settings reloaded.")).toBeVisible();
  await expect(page.getByText(/Gateway|Configuration reloaded successfully/)).toHaveCount(0);
});
