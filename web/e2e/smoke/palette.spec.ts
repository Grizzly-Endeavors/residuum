import type { Locator, Page } from "@playwright/test";
import { expectNoAxeViolations } from "../support/axe";
import { expect, test } from "../support/fixtures";
import { expectPaletteOpen } from "../support/lazy";

/**
 * The action registry's surfaces: the command palette (⌘K or Ctrl+K, the
 * rail's search row, the phone's Search tab), the composer's `/` menu, the
 * help actions and their dialogs, and the drawer getting out of the way of
 * whatever an action opens.
 */

const OVERLAYS = "[data-overlay-host]";

function address(page: Page): string {
  const url = new URL(page.url());
  return decodeURIComponent(`${url.pathname}${url.search}`);
}

function palette(page: Page): Locator {
  return page.getByRole("dialog", { name: "Search and commands" });
}

/** Open the palette the way this size offers it: the keyboard on a desktop, the Search tab on a phone. */
async function openPalette(page: Page, isMobile: boolean): Promise<Locator> {
  if (isMobile) {
    await page
      .getByRole("navigation", { name: "Main" })
      .getByRole("button", { name: "Search" })
      .click();
  } else {
    await page.keyboard.press("ControlOrMeta+k");
  }
  const dialog = await expectPaletteOpen(page);
  await expect(dialog.getByRole("combobox")).toBeFocused();
  return dialog;
}

async function openDrawer(page: Page): Promise<Locator> {
  await page
    .getByRole("navigation", { name: "Main" })
    .getByRole("button", { name: "Menu" })
    .click();
  await expect(page.getByRole("dialog", { name: "Agents and places" })).toBeVisible();
  return page.getByRole("navigation", { name: "Places and agents" });
}

test("the palette goes to another agent's place, found by typing", async ({ page, isMobile }) => {
  await page.goto("/home");
  await expect(page.getByRole("heading", { name: "Home", level: 1 })).toBeVisible();
  const dialog = await openPalette(page, isMobile);
  await expectNoAxeViolations(page, { within: OVERLAYS });

  await page.keyboard.type("scout files");
  await expect(dialog.getByRole("option")).toHaveCount(1);
  await expect(dialog.getByRole("option", { name: /Files/ })).toHaveAttribute(
    "aria-selected",
    "true",
  );
  await page.keyboard.press("Enter");
  await expect(dialog).toBeHidden();
  await expect.poll(() => address(page)).toBe("/agent/scout/files");
});

test("the rail's search row opens the palette, and Back closes it", async ({ page, isMobile }) => {
  test.skip(isMobile, "The phone's rail opens in the drawer; its search row is covered below.");
  await page.goto("/agent/atlas");
  await page.getByRole("button", { name: /^Search or jump to/ }).click();
  await expectPaletteOpen(page);
  await page.goBack();
  await expect(palette(page)).toBeHidden();
  expect(address(page)).toBe("/agent/atlas");

  await page.keyboard.press("ControlOrMeta+k");
  await expect(palette(page)).toBeVisible();
  await page.keyboard.press("ControlOrMeta+k");
  await expect(palette(page)).toBeHidden();
});

test("the palette runs Summarize older messages now", async ({ page, isMobile }) => {
  await page.goto("/agent/atlas");
  await expect(
    page.getByText("Hi, this is atlas. You are in my conversation, not scout's."),
  ).toBeVisible();
  const dialog = await openPalette(page, isMobile);
  await page.keyboard.type("summarize older");
  await expect(
    dialog.getByRole("option", { name: /Summarize older messages now/ }),
  ).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("Enter");
  await expect(dialog).toBeHidden();
  await expect(page.getByText("Asked atlas to summarize older messages.")).toBeVisible();
  await expect(page.getByText("Command 'observe' executed. (mock)")).toBeVisible();
});

test("Show conversation size opens the agent's chat with the size in the panel", async ({
  page,
  isMobile,
}) => {
  await page.goto("/home");
  await openPalette(page, isMobile);
  await page.keyboard.type("conversation size");
  await page.keyboard.press("Enter");
  await expect.poll(() => address(page)).toBe("/agent/atlas?panel=size");
  await expect(page.getByRole("heading", { name: "Conversation size" })).toBeVisible();
});

test("an action that needs a running agent says why it can't run", async ({ page, isMobile }) => {
  await page.goto("/agent/drifter");
  const dialog = await openPalette(page, isMobile);
  await page.keyboard.type("condense");
  const option = dialog.getByRole("option", { name: /Condense memories now/ });
  await expect(option).toHaveAttribute("aria-disabled", "true");
  await expect(option).toContainText("Start drifter first");
  await page.keyboard.press("Enter");
  await expect(dialog).toBeVisible();
  await expectNoAxeViolations(page, { within: OVERLAYS });
});

test("an agent's own palette lists its places and sessions, and opens a session in the panel", async ({
  page,
  isMobile,
}) => {
  await page.goto("/home");
  const dialog = await openPalette(page, isMobile);
  await expect(dialog.getByRole("group", { name: "In atlas" })).toBeVisible();
  await expect(dialog.getByRole("group", { name: "In scout" })).toHaveCount(0);
  const running = dialog.getByRole("group", { name: "Running now" });
  await expect(running).toBeVisible();
  await running.getByRole("option").first().click();
  await expect.poll(() => address(page)).toMatch(/^\/agent\/atlas\/activity\?panel=session:atlas:/);
});

test.describe("the composer's / menu", () => {
  test("lists the chat actions, narrows as you type, and Enter runs one", async ({ page }) => {
    await page.goto("/agent/atlas");
    const box = page.locator(".chat-input");
    await box.click();
    await page.keyboard.type("/");
    const menu = page.getByRole("listbox", { name: "Chat actions" });
    await expect(menu).toBeVisible();
    await expect(menu.getByRole("option", { name: /Stop reply/ })).toContainText(
      "atlas isn't replying right now",
    );
    await expectNoAxeViolations(page, {
      within: "[role=listbox]",
      allow: [
        {
          rule: "scrollable-region-focusable",
          reason:
            "the arrow keys in the message box scroll the menu, but axe only sees that when a combobox controls it, and the legacy message box is a textarea, which can't be one; the composer's rebuild (W25) settles its field",
        },
      ],
    });

    await page.keyboard.type("refl");
    await expect(menu.getByRole("option")).toHaveCount(1);
    await page.keyboard.press("Enter");
    await expect(menu).toBeHidden();
    await expect(page.getByText("Asked atlas to condense its memories.")).toBeVisible();
    await expect(box).toHaveValue("");
  });

  test("Tab fills in a command, and a typed command runs with its text", async ({ page }) => {
    await page.goto("/agent/atlas");
    const box = page.locator(".chat-input");
    await box.click();
    await page.keyboard.type("/inb");
    await page.keyboard.press("Tab");
    await expect(box).toHaveValue("/inbox ");
    await page.keyboard.type("water the plants");
    await page.keyboard.press("Enter");
    await expect(page.getByText("Added a note to atlas's inbox.")).toBeVisible();

    await page.keyboard.type("/nope");
    await page.keyboard.press("Enter");
    await expect(page.getByText(/There's no \/nope\./)).toBeVisible();
  });
});

test("Add a note asks for its text when run from the palette", async ({ page, isMobile }) => {
  await page.goto("/agent/atlas");
  await openPalette(page, isMobile);
  await page.keyboard.type("add a note");
  await page.keyboard.press("Enter");
  const prompt = page.getByRole("dialog", { name: "Add a note to atlas's inbox" });
  await expect(prompt).toBeVisible();
  await expectNoAxeViolations(page, { within: OVERLAYS });
  await prompt.getByRole("textbox", { name: "Note" }).fill("Check the wiki index");
  await prompt.getByRole("button", { name: "Add note" }).click();
  await expect(prompt).toBeHidden();
  await expect(page.getByText("Added a note to atlas's inbox.")).toBeVisible();
});

test.describe("on a phone", () => {
  test("the Search tab opens the palette full screen, and Close closes it", async ({
    page,
    isMobile,
  }) => {
    test.skip(!isMobile, "The Search tab is the phone's.");
    await page.goto("/agent/atlas");
    const dialog = await openPalette(page, isMobile);
    const box = await dialog.boundingBox();
    expect(box?.width ?? 0).toBeCloseTo(page.viewportSize()?.width ?? -1, 0);
    await dialog.getByRole("button", { name: "Close" }).click();
    await expect(dialog).toBeHidden();
    expect(address(page)).toBe("/agent/atlas");
  });

  test("running an action from the drawer closes the drawer", async ({ page, isMobile }) => {
    test.skip(!isMobile, "The drawer is the phone's rail.");
    await page.goto("/home");
    let rail = await openDrawer(page);
    await rail.getByRole("button", { name: "Help" }).click();
    await page.getByRole("menuitem", { name: "Keyboard shortcuts" }).click();
    await expect(page.getByRole("dialog", { name: "Keyboard shortcuts" })).toBeVisible();
    await expect(page.getByRole("dialog", { name: "Agents and places" })).toBeHidden();
    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog", { name: "Keyboard shortcuts" })).toBeHidden();
    expect(address(page)).toBe("/home");

    rail = await openDrawer(page);
    await rail.getByRole("button", { name: /^Search or jump to/ }).click();
    await expectPaletteOpen(page);
    await expect(page.getByRole("dialog", { name: "Agents and places" })).toBeHidden();
  });
});

test("the shortcuts dialog lists Esc for stopping a reply and what / does", async ({
  page,
  isMobile,
}) => {
  await page.goto("/home");
  await expect(page.getByRole("heading", { name: "Home", level: 1 })).toBeVisible();
  if (isMobile) {
    const rail = await openDrawer(page);
    await rail.getByRole("button", { name: "Help" }).click();
    await page.getByRole("menuitem", { name: "Keyboard shortcuts" }).click();
  } else {
    await page.locator("body").press("?");
  }
  const dialog = page.getByRole("dialog", { name: "Keyboard shortcuts" });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByText("Stop the reply while the agent is replying")).toBeVisible();
  await expect(dialog.getByText(/As the first character, list the chat actions/)).toBeVisible();
  await expectNoAxeViolations(page, { within: OVERLAYS });
});

test("feedback keeps its draft, sends, and shows the reference", async ({ page, isMobile }) => {
  await page.goto("/home");
  const rail = isMobile
    ? await openDrawer(page)
    : page.getByRole("navigation", { name: "Places and agents" });
  await rail.getByRole("button", { name: "Help" }).click();
  await page.getByRole("menuitem", { name: "Report a bug" }).click();

  let dialog = page.getByRole("dialog", { name: "Report a bug" });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole("button", { name: "Send report" })).toBeDisabled();
  await dialog.getByRole("textbox", { name: "What happened?" }).fill("The palette froze");
  await expectNoAxeViolations(page, { within: OVERLAYS });

  await dialog.getByRole("radio", { name: "Send feedback" }).click();
  dialog = page.getByRole("dialog", { name: "Send feedback" });
  await dialog.getByRole("textbox", { name: "Your feedback" }).fill("Love the new search");
  await dialog.getByRole("radio", { name: "Report a bug" }).click();
  dialog = page.getByRole("dialog", { name: "Report a bug" });
  await expect(dialog.getByRole("textbox", { name: "What happened?" })).toHaveValue(
    "The palette froze",
  );

  await dialog.getByRole("radio", { name: "Send feedback" }).click();
  dialog = page.getByRole("dialog", { name: "Send feedback" });
  await dialog.getByRole("button", { name: "Send feedback" }).click();
  await expect(dialog.getByRole("status")).toContainText("RR-MOCK-FBK-01");
  await dialog.getByRole("button", { name: "Done" }).click();
  await expect(dialog).toBeHidden();
});
