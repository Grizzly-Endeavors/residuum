import type { Locator, Page, WebSocketRoute } from "@playwright/test";
import { expectNoAxeViolations } from "../support/axe";
import { expect, test } from "../support/fixtures";

/**
 * The composer: the chat actions under `/`, images attached by
 * button and by drop, a draft kept per agent across navigation and reload,
 * the model and thinking control (a popover, a sheet on phones), messages
 * waiting while the connection is down, and the conversation size in the
 * context panel for a running agent and a stopped one.
 */

const GREETING = "Hi, this is atlas. You are in my conversation, not scout's.";
const OVERLAYS = "[data-overlay-host]";
/** A 1×1 PNG. */
const PNG = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==",
  "base64",
);

function box(page: Page, agent = "atlas"): Locator {
  return page.getByRole("textbox", { name: `Message ${agent}` });
}

async function openChat(page: Page): Promise<void> {
  await page.goto("/agent/atlas");
  await expect(page.getByText(GREETING)).toBeVisible();
}

test.describe("the / menu", () => {
  test("lists the chat actions, narrows as you type, and Enter runs one", async ({ page }) => {
    await openChat(page);
    await box(page).click();
    await page.keyboard.type("/");
    const menu = page.getByRole("listbox", { name: "Chat actions" });
    await expect(menu).toBeVisible();
    await expect(page.getByRole("combobox", { name: "Message atlas" })).toHaveAttribute(
      "aria-expanded",
      "true",
    );
    await expect(menu.getByRole("option", { name: /Stop reply/ })).toContainText(
      "atlas isn't replying right now",
    );
    await expectNoAxeViolations(page, { within: "[role=listbox]" });

    await page.keyboard.type("refl");
    await expect(menu.getByRole("option")).toHaveCount(1);
    await page.keyboard.press("Enter");
    await expect(menu).toBeHidden();
    // That it started, then the agent's own word that it's done.
    await expect(page.getByText("atlas is condensing its memories…")).toBeVisible();
    await expect(page.getByText("Command 'reflect' executed. (mock)")).toBeVisible();
    await expect(box(page)).toHaveValue("");
  });

  test("Tab fills in a command, and a typed command runs with its text", async ({ page }) => {
    await openChat(page);
    await box(page).click();
    await page.keyboard.type("/inb");
    await page.keyboard.press("Tab");
    await expect(box(page)).toHaveValue("/inbox ");
    await page.keyboard.type("water the plants");
    await page.keyboard.press("Enter");
    // One message: the agent's own, once the note is in its inbox.
    await expect(page.getByText("[inbox] item added")).toBeVisible();
  });

  test("a pasted path is a message, and an action that can't run keeps the line", async ({
    page,
  }) => {
    await openChat(page);
    const conversation = page.getByRole("region", { name: "Conversation with atlas" });
    await box(page).click();
    await page.keyboard.type("/home/bear/logs/app.log has the error");
    await page.keyboard.press("Enter");
    await expect(conversation.getByText("/home/bear/logs/app.log has the error")).toBeVisible();
    await expect(box(page)).toHaveValue("");

    // Stop reply names an action, which can't run while atlas isn't replying.
    await page.keyboard.type("/stop now");
    await page.keyboard.press("Enter");
    await expect(
      page.getByText(/^Couldn't run \/stop: atlas isn't replying right now\./),
    ).toBeVisible();
    await expect(box(page)).toHaveValue("/stop now");
  });

  test("the button opens every action, and Esc closes the menu without stopping anything", async ({
    page,
  }) => {
    await openChat(page);
    await page.getByRole("button", { name: "Chat actions" }).click();
    const menu = page.getByRole("listbox", { name: "Chat actions" });
    for (const label of ["Summarize older messages now", "Show conversation size", "Stop reply"]) {
      await expect(menu.getByRole("option", { name: new RegExp(`^${label}`) })).toBeAttached();
    }
    await expect(box(page)).toBeFocused();
    await page.keyboard.press("Escape");
    await expect(menu).toBeHidden();
  });
});

test.describe("images", () => {
  test("attach by button, show as thumbnails, and send with the message", async ({ page }) => {
    await openChat(page);
    await page.locator('input[type="file"]').setInputFiles([
      { name: "one.png", mimeType: "image/png", buffer: PNG },
      { name: "two.png", mimeType: "image/png", buffer: PNG },
    ]);
    const attached = page.getByRole("list", { name: "Attached images" });
    await expect(attached.getByRole("img")).toHaveCount(2);
    await page.getByRole("button", { name: "Remove image 2" }).click();
    await expect(attached.getByRole("img")).toHaveCount(1);
    await expectNoAxeViolations(page);

    await box(page).fill("Here's the screenshot");
    await box(page).press("Enter");
    const conversation = page.getByRole("region", { name: "Conversation with atlas" });
    await expect(conversation.getByRole("img", { name: "Attached image 1" })).toBeVisible();
    await expect(attached).toHaveCount(0);
  });

  test("attach by drop, with the composer marked while a file is over it", async ({ page }) => {
    await openChat(page);
    const composer = page.locator("form.composer");
    const files = await page.evaluateHandle(
      (bytes) => {
        const transfer = new DataTransfer();
        transfer.items.add(new File([new Uint8Array(bytes)], "drop.png", { type: "image/png" }));
        transfer.items.add(new File(["text"], "notes.txt", { type: "text/plain" }));
        return transfer;
      },
      [...PNG],
    );
    await composer.dispatchEvent("dragover", { dataTransfer: files });
    await expect(composer).toHaveAttribute("data-dragging");
    await composer.dispatchEvent("drop", { dataTransfer: files });
    await expect(composer).not.toHaveAttribute("data-dragging");
    await expect(page.getByRole("img", { name: "Image 1" })).toBeVisible();
    await expect(composer.getByRole("alert")).toContainText(
      "notes.txt can't be attached. Attach a JPEG, PNG, GIF or WebP image.",
    );
  });
});

test("a draft is kept for its agent across navigation and reload", async ({ page }) => {
  await openChat(page);
  await box(page).fill("Half a thought for atlas");

  await page.goto("/agent/scout");
  await expect(box(page, "scout")).toHaveValue("");
  await box(page, "scout").fill("Something for scout");

  await page.goBack();
  await expect(box(page)).toHaveValue("Half a thought for atlas");
  await page.reload();
  await expect(box(page)).toHaveValue("Half a thought for atlas");

  await box(page).press("Enter");
  await expect(box(page)).toHaveValue("");
  await page.reload();
  await expect(box(page)).toHaveValue("");
  await page.goto("/agent/scout");
  await expect(box(page, "scout")).toHaveValue("Something for scout");
});

test("the model control switches the model and keeps its failover list", async ({
  page,
  isMobile,
}) => {
  const providers = [
    "[models.main]",
    'model = ["anthropic/claude-sonnet-4-6", "openai/gpt-4o"]',
    'thinking = "low"',
    "",
  ].join("\n");
  const put = await page.request.put("/api/agents/atlas/providers/raw", {
    headers: { "Content-Type": "text/plain" },
    data: providers,
  });
  expect(put.ok()).toBe(true);
  await openChat(page);

  await page.getByRole("button", { name: "Model: Claude Sonnet 4.6, low thinking" }).click();
  const chooser = page.getByRole("dialog", { name: "Model for atlas" });
  await expect(chooser).toBeVisible();
  await expect(chooser.getByRole("button", { name: "Claude Sonnet 4.6" })).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  await expectNoAxeViolations(page, { within: OVERLAYS });
  if (isMobile) {
    // A sheet over the bottom bar, not a popover beside the button.
    const card = await chooser.boundingBox();
    expect(card?.width).toBeCloseTo(390, 0);
  }

  await chooser.getByRole("button", { name: "Claude Haiku 4.5" }).click();
  await expect(
    page.getByRole("button", { name: "Model: Claude Haiku 4.5, low thinking" }),
  ).toBeAttached();
  await chooser.getByRole("button", { name: "Low" }).click();
  await expect(
    page.getByRole("button", { name: "Model: Claude Haiku 4.5", exact: true }),
  ).toBeAttached();

  const raw = await (await page.request.get("/api/agents/atlas/providers/raw")).text();
  expect(raw).toContain("anthropic/claude-haiku-4-5");
  expect(raw).toContain("openai/gpt-4o");
  expect(raw).not.toContain("thinking");
});

test("messages wait while the connection is down, and say so", async ({ page }) => {
  let online = true;
  const open: WebSocketRoute[] = [];
  await page.routeWebSocket(/\/api\/agents\/atlas\/ws$/, (socket) => {
    if (!online) {
      void socket.close();
      return;
    }
    socket.connectToServer();
    open.push(socket);
  });
  await openChat(page);
  await expect(page.getByRole("status").filter({ hasText: "Reconnecting" })).toHaveCount(0);

  online = false;
  for (const socket of open.splice(0)) await socket.close();
  await expect(page.getByText(/^Reconnecting — messages you send now/)).toBeVisible();
  await box(page).fill("Are you there?");
  await box(page).press("Enter");
  await expect(
    page.getByText("Reconnecting — 1 message will send once back online."),
  ).toBeVisible();

  online = true;
  await expect(page.getByText(/^Reconnecting/)).toHaveCount(0, { timeout: 20_000 });
  await expect(page.getByText("I've looked into that and here's what I found:")).toBeVisible();
});

test.describe("the conversation size", () => {
  async function sizePanel(page: Page, isMobile: boolean): Promise<Locator> {
    const panel = isMobile
      ? page.getByRole("dialog", { name: "Conversation size" })
      : page.getByRole("complementary", { name: "Conversation size" });
    await expect(panel).toBeVisible();
    return panel;
  }

  test("shows a running agent's figures in words, and follows its replies", async ({
    page,
    isMobile,
  }) => {
    await page.goto("/agent/atlas");
    await page.goto("/agent/atlas?panel=size");
    const panel = await sizePanel(page, isMobile);
    await expect(panel).toContainText("About 14,000 words go to the model each time atlas replies");
    await expect(panel).toContainText("37 times");
    await panel.getByRole("button", { name: "Token counts" }).click();
    await expect(panel.getByText("412,880")).toBeVisible();
    await expectNoAxeViolations(page, isMobile ? { within: OVERLAYS } : {});

    if (!isMobile) {
      await box(page).fill("Check the wiki index");
      await box(page).press("Enter");
      await expect(panel).toContainText("40 times");
    }
  });

  test("shows a stopped agent's last figures", async ({ page, isMobile }) => {
    await page.request.post("/api/hub/agents/atlas/stop");
    await page.goto("/agent/atlas");
    await page.goto("/agent/atlas?panel=size");
    const panel = await sizePanel(page, isMobile);
    await expect(panel).toContainText(
      "atlas isn't running, so these are the figures from when it last replied.",
    );
    await expect(panel).toContainText("37 times");
    await expect(
      panel.getByRole("button", { name: "Summarize older messages now" }),
    ).toBeDisabled();
    await expect(panel).toContainText("Start atlas first.");
    await expectNoAxeViolations(page, isMobile ? { within: OVERLAYS } : {});
  });

  test("says there is nothing yet for an agent that has never replied", async ({
    page,
    isMobile,
  }) => {
    await page.goto("/agent/drifter");
    await page.goto("/agent/drifter?panel=size");
    const panel = await sizePanel(page, isMobile);
    await expect(panel).toContainText(
      "The figures show once drifter has replied in this conversation.",
    );
  });
});

test("the memory work after a reply shows under it until it's done", async ({ page, mock }) => {
  await mock.post("/api/mock/delays", { data: { scale: 1 } });
  await openChat(page);
  await box(page).fill("remember that the plants need water");
  await box(page).press("Enter");
  const status = page.getByRole("status").filter({ hasText: "Noting what matters" });
  await expect(status).toHaveText("Noting what matters from this conversation", {
    timeout: 15_000,
  });
  await expect(status).toHaveCount(0, { timeout: 15_000 });
});
