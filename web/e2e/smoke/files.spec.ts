import type { Locator, Page } from "@playwright/test";
import { expectNoAxeViolations } from "../support/axe";
import { expect, test } from "../support/fixtures";

/**
 * Files and Shared files: the tree, the editor in the context panel, rename,
 * delete with Undo, history with restore, the save conflict, and the guard
 * that asks before unsaved edits are lost.
 */

/** Path and query of the page's URL, decoded, so a test can compare them exactly. */
function address(page: Page): string {
  const url = new URL(page.url());
  return decodeURIComponent(`${url.pathname}${url.search}`);
}

/** The tree's row for a file or folder. */
function row(page: Page, name: string): Locator {
  return page.getByRole("list", { name: "Files" }).locator(`[data-path$="${name}"]`);
}

/** The context panel showing `name`: a column, or on a phone a full-screen sheet. */
function filePanel(page: Page, isMobile: boolean, name: string): Locator {
  return page.getByRole(isMobile ? "dialog" : "complementary", { name });
}

function editor(panel: Locator, name: string): Locator {
  return panel.getByRole("textbox", { name: `Contents of ${name}` });
}

/** What the mock has on disk for one of atlas's files. */
async function onDisk(page: Page, path: string): Promise<string> {
  const response = await page.request.get(
    `/api/agents/atlas/workspace/file?path=${encodeURIComponent(path)}`,
  );
  return response.text();
}

async function openFile(page: Page, isMobile: boolean, name: string): Promise<Locator> {
  await row(page, name).click();
  const panel = filePanel(page, isMobile, name);
  await expect(editor(panel, name)).toBeVisible();
  return panel;
}

async function rowMenu(page: Page, name: string): Promise<void> {
  await page.getByRole("button", { name: `More for ${name}` }).click();
}

test("a file opens in the panel, and Save writes the edits", async ({ page, isMobile }) => {
  await page.goto("/agent/atlas/files");
  await expect(page.getByRole("heading", { name: "atlas", level: 1 })).toBeVisible();
  await expectNoAxeViolations(page);

  const panel = await openFile(page, isMobile, "SOUL.md");
  await expect.poll(() => address(page)).toBe("/agent/atlas/files?panel=file:SOUL.md");
  await expect(row(page, "SOUL.md")).toHaveAttribute("aria-current", "true");
  if (isMobile) {
    // On a phone the editor is the whole screen, with Back.
    await expect(panel.getByRole("button", { name: "Back" })).toBeVisible();
    const box = await panel.boundingBox();
    expect(box?.width).toBeCloseTo(390, 0);
    expect(box?.height).toBeCloseTo(844, 0);
  }

  await editor(panel, "SOUL.md").press("End");
  await editor(panel, "SOUL.md").pressSequentially("A new line.");
  await expect(panel.getByText("Unsaved changes")).toBeVisible();
  if (isMobile) await expectNoAxeViolations(page, { within: "[data-overlay-host]" });
  else await expectNoAxeViolations(page);

  await panel.getByRole("button", { name: "Save", exact: true }).click();
  await expect(page.getByText("Saved SOUL.md.")).toBeVisible();
  await expect(panel.getByText("Unsaved changes")).toBeHidden();
  expect(await onDisk(page, "SOUL.md")).toMatch(/A new line\.$/);
});

test("a diagnostic shows under the editor, and its line moves the caret there", async ({
  page,
  isMobile,
}) => {
  await page.goto("/agent/atlas/files");
  await row(page, "config").click();
  const panel = await openFile(page, isMobile, "channels.toml");
  const text = editor(panel, "channels.toml");
  await text.press("ControlOrMeta+End");
  await text.pressSequentially("broken = ");

  const problems = panel.getByRole("list", { name: "Problems in channels.toml" });
  await expect(problems).toContainText("Invalid TOML document");
  await problems.getByRole("button", { name: /^line 13/ }).click();
  await expect(text).toBeFocused();
});

test("rename is inline, and a slash moves the file into a folder", async ({ page }) => {
  await page.goto("/agent/atlas/files");
  await rowMenu(page, "PRESENCE.toml");
  await page.getByRole("menuitem", { name: "Rename" }).click();
  const name = page.getByRole("textbox", { name: "New name for PRESENCE.toml" });
  await name.fill("memory/presence.toml");
  await name.press("Enter");

  await expect(page.getByText("Moved to memory/presence.toml.")).toBeVisible();
  await expect(row(page, "PRESENCE.toml")).toHaveCount(0);
  await row(page, "memory").click();
  await expect(row(page, "memory/presence.toml")).toBeVisible();
});

test("delete is immediate, and Undo brings the file back", async ({ page, isMobile }) => {
  await page.goto("/agent/atlas/files");
  const panel = await openFile(page, isMobile, "PRESENCE.toml");
  if (isMobile) await panel.getByRole("button", { name: "Back" }).click();

  await rowMenu(page, "PRESENCE.toml");
  await page.getByRole("menuitem", { name: "Delete" }).click();
  const toast = page.getByRole("status").filter({ hasText: "Deleted PRESENCE.toml." });
  await expect(toast).toBeVisible();
  await expect(row(page, "PRESENCE.toml")).toHaveCount(0);
  // Beside the tree, the panel that showed it says it is gone.
  if (!isMobile) await expect(panel.getByText("This file doesn't exist")).toBeVisible();

  await toast.getByRole("button", { name: "Undo" }).click();
  await expect(page.getByText("Restored.")).toBeVisible();
  await expect(row(page, "PRESENCE.toml")).toBeVisible();
  if (!isMobile) await expect(editor(panel, "PRESENCE.toml")).toHaveValue(/\[presence\]/);
  expect(await onDisk(page, "PRESENCE.toml")).toContain("[presence]");
});

test("a linked file that doesn't exist says so, and offers nothing else", async ({
  page,
  isMobile,
}) => {
  await page.goto("/agent/atlas?panel=file:notes/missing.md");
  const panel = filePanel(page, isMobile, "missing.md");
  await expect(panel.getByRole("heading", { name: "This file doesn't exist" })).toBeVisible();
  await expect(panel.getByRole("textbox")).toHaveCount(0);
  await expect(panel.getByRole("button", { name: "History" })).toHaveCount(0);
});

test("History restores an earlier version", async ({ page, isMobile }) => {
  await page.goto("/agent/atlas/files?panel=file:SOUL.md");
  const panel = filePanel(page, isMobile, "SOUL.md");
  await expect(editor(panel, "SOUL.md")).toHaveValue(/Craft/);
  await panel.getByRole("button", { name: "History" }).click();

  const dialog = page.getByRole("dialog", { name: "History of SOUL.md" });
  await expect(dialog.getByRole("button", { name: /updated SOUL\.md/ })).toHaveAttribute(
    "aria-current",
    "true",
  );
  await expect(dialog.getByText("+- **Craft**", { exact: false })).toBeVisible();
  await expectNoAxeViolations(page, { within: "[data-overlay-host]" });

  await dialog.getByRole("button", { name: /edits made outside a turn/ }).click();
  await dialog.getByRole("radio", { name: "Whole file" }).click();
  await expect(dialog.getByText(/Core Identity/)).toBeVisible();
  await dialog.getByRole("button", { name: "Restore this version" }).click();
  await expect(page.getByText(/^Restored SOUL\.md to the version from/)).toBeVisible();

  await dialog.getByRole("button", { name: "Close" }).click();
  await expect(editor(panel, "SOUL.md")).not.toHaveValue(/Craft/);
  expect(await onDisk(page, "SOUL.md")).not.toContain("Craft");
});

test("a save after the file changed on disk asks, and overwrites only when told", async ({
  page,
  isMobile,
  mock,
}) => {
  await page.goto("/agent/atlas/files?panel=file:SOUL.md");
  const panel = filePanel(page, isMobile, "SOUL.md");
  await editor(panel, "SOUL.md").press("End");
  await editor(panel, "SOUL.md").pressSequentially("Mine.");
  await mock.post("/api/mock/agent-file", {
    params: { agent: "atlas" },
    data: { path: "SOUL.md", content: "# Soul\n\nTheirs.\n" },
  });
  await expect(panel.getByText("SOUL.md changed on disk while you were editing.")).toBeVisible();

  await panel.getByRole("button", { name: "Save", exact: true }).click();
  const conflict = page.getByRole("alertdialog", { name: "SOUL.md changed on disk" });
  await expect(conflict).toBeVisible();
  await expectNoAxeViolations(page, { within: "[data-overlay-host]" });
  expect(await onDisk(page, "SOUL.md")).toBe("# Soul\n\nTheirs.\n");

  await conflict.getByRole("button", { name: "Overwrite with my edits" }).click();
  await expect(page.getByText("Saved SOUL.md.")).toBeVisible();
  expect(await onDisk(page, "SOUL.md")).toMatch(/Mine\.$/);
});

test.describe("unsaved edits", () => {
  async function edited(page: Page, isMobile: boolean): Promise<Locator> {
    await page.goto("/agent/atlas/files");
    const panel = await openFile(page, isMobile, "SOUL.md");
    await editor(panel, "SOUL.md").pressSequentially("Unsaved.");
    return panel;
  }

  function leaveDialog(page: Page): Locator {
    return page.getByRole("alertdialog", { name: "Discard unsaved changes?" });
  }

  test("are asked about before changing place", async ({ page, isMobile }) => {
    test.skip(isMobile, "On a phone the editor covers the places; Back is how to leave it.");
    await edited(page, false);
    const rail = page.getByRole("navigation", { name: "Places and agents" });

    await rail.getByRole("link", { name: /^Home/ }).click();
    await expect(leaveDialog(page)).toContainText("Unsaved changes to SOUL.md");
    await leaveDialog(page).getByRole("button", { name: "Keep editing" }).click();
    await expect.poll(() => address(page)).toBe("/agent/atlas/files?panel=file:SOUL.md");

    // Another file is another place for the edits too.
    await row(page, "HEARTBEAT.yml").click();
    await leaveDialog(page).getByRole("button", { name: "Keep editing" }).click();
    await expect.poll(() => address(page)).toBe("/agent/atlas/files?panel=file:SOUL.md");

    await rail.getByRole("link", { name: /^Home/ }).click();
    await leaveDialog(page).getByRole("button", { name: "Discard and leave" }).click();
    await expect.poll(() => address(page)).toBe("/home");
  });

  test("are asked about before the panel closes", async ({ page, isMobile }) => {
    const panel = await edited(page, isMobile);
    await panel.getByRole("button", { name: isMobile ? "Back" : "Close panel" }).click();
    await leaveDialog(page).getByRole("button", { name: "Keep editing" }).click();
    await expect(editor(panel, "SOUL.md")).toHaveValue(/Unsaved\.$/);

    await page.keyboard.press("Escape");
    await expect(leaveDialog(page)).toBeVisible();
    await leaveDialog(page).getByRole("button", { name: "Discard and leave" }).click();
    await expect(panel).toBeHidden();
    await expect.poll(() => address(page)).toBe("/agent/atlas/files");
  });

  test("are asked about on Back", async ({ page, isMobile }) => {
    await edited(page, isMobile);
    await page.goBack();
    await leaveDialog(page).getByRole("button", { name: "Keep editing" }).click();
    await expect.poll(() => address(page)).toBe("/agent/atlas/files?panel=file:SOUL.md");

    await page.goBack();
    await leaveDialog(page).getByRole("button", { name: "Discard and leave" }).click();
    await expect.poll(() => address(page)).toBe("/agent/atlas/files");
  });
});

test("Shared files follows the team's folder live", async ({ page, isMobile, mock }) => {
  await page.goto("/team/files");
  await expect(page.getByRole("heading", { name: "Shared files", level: 1 })).toBeVisible();
  await row(page, "wiki").click();
  await expect(row(page, "wiki/index.md")).toBeVisible();
  await expectNoAxeViolations(page);

  await mock.post("/api/mock/team-file", {
    data: { path: "team/wiki/routing.md", content: "# Routing\n" },
  });
  await expect(row(page, "wiki/routing.md")).toBeVisible();

  // An identity file is tinted here too.
  await expect(page.locator('[data-identity] [data-path="AGENTS.md"]')).toBeVisible();
  const panel = await openFile(page, isMobile, "routing.md");
  await expect(editor(panel, "routing.md")).toHaveValue("# Routing\n");
});
