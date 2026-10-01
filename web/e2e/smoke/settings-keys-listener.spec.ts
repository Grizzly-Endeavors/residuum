import type { Locator, Page } from "@playwright/test";
import { expectNoAxeViolations } from "../support/axe";
import { expect, test } from "../support/fixtures";
import { expectSettingsOpen } from "../support/lazy";

/**
 * The All agents scope's Saved keys, Agent-to-agent listener (with its caller
 * keys), Notifications, Raw config and History sections. The keys act at once
 * through their own endpoints; the listener's switch and port are staged.
 */

const HUB_CONFIG = "/api/hub/config/raw";
const OVERLAY = "[data-overlay-host]";

const saveBar = (page: Page): Locator => page.getByRole("region", { name: "Unsaved changes" });
const toastWith = (page: Page, text: string | RegExp): Locator =>
  page.getByRole("status").filter({ hasText: text });
const undo = (page: Page): Locator => page.getByRole("button", { name: "Undo" });

async function openSection(page: Page, section: string): Promise<void> {
  await page.goto(`/home?settings=_all/${section}`);
  await expectSettingsOpen(page);
}

async function names(page: Page, path: string, key: string): Promise<string[]> {
  const body = (await (await page.request.get(path)).json()) as Record<string, unknown[]>;
  return (body[key] ?? []).map((entry) =>
    typeof entry === "string" ? entry : (entry as { name: string }).name,
  );
}

test.describe("Saved keys", () => {
  test("an agent key is added, removed at once, and brought back with Undo, never showing its value", async ({
    page,
  }) => {
    await openSection(page, "keys");
    const keys = page.getByRole("list", { name: "Agent keys" });
    await expect(keys).toContainText("github_token");
    await expect(keys).toContainText("$GITHUB_TOKEN");
    await expect(keys.getByText("Saved by the agent")).toBeVisible();
    await expectNoAxeViolations(page, { within: OVERLAY });

    await page.getByRole("button", { name: "Add a key" }).click();
    await expect(page.getByLabel("Name", { exact: true })).toBeFocused();
    await page.getByLabel("Name", { exact: true }).fill("stripe_live");
    await expect(page.getByText("Commands that use it get $STRIPE_LIVE.")).toBeVisible();
    await page.getByLabel("Value", { exact: true }).fill("sk_live_abcdefghij");
    await page.getByLabel("Description").fill("Billing reads");
    await expectNoAxeViolations(page, { within: OVERLAY });

    await page.getByRole("button", { name: "Save key" }).click();
    await expect(
      toastWith(page, "Saved stripe_live. Commands that use it get $STRIPE_LIVE."),
    ).toBeVisible();
    await expect(keys).toContainText("stripe_live");
    await expect(keys).toContainText("Billing reads");
    await expect(page.getByRole("button", { name: "Add a key" })).toBeFocused();
    await expect(saveBar(page)).toBeHidden();
    await expect(page.locator(OVERLAY)).not.toContainText("sk_live_abcdefghij");
    const listed = await (await page.request.get("/api/hub/agent-keys")).text();
    expect(listed).toContain("stripe_live");
    expect(listed).not.toContain("sk_live_abcdefghij");

    await page.getByRole("button", { name: "Remove stripe_live" }).click();
    await expect(toastWith(page, "Removed stripe_live.")).toBeVisible();
    await expect(keys).not.toContainText("stripe_live");
    await expect(page.getByRole("button", { name: "Add a key" })).toBeFocused();
    expect(await names(page, "/api/hub/agent-keys", "keys")).not.toContain("stripe_live");

    await undo(page).click();
    await expect(toastWith(page, "Restored.")).toBeVisible();
    await expect(keys).toContainText("stripe_live");
    expect(await names(page, "/api/hub/agent-keys", "keys")).toContain("stripe_live");
  });

  test("a name the hub would refuse is explained, and a short value is hinted and still saves", async ({
    page,
  }) => {
    await openSection(page, "keys");
    await page.getByRole("button", { name: "Add a key" }).click();
    await page.getByLabel("Name", { exact: true }).fill("Stripe Live");
    await page.getByLabel("Value", { exact: true }).fill("1234");
    await expect(page.getByText(/Start with a lowercase letter/)).toBeVisible();
    await expect(page.getByRole("button", { name: "Save key" })).toBeDisabled();
    await expect(page.getByText(/under 8 characters can't be hidden/)).toBeVisible();

    await page.getByLabel("Name", { exact: true }).fill("github_token");
    await expect(page.getByText(/Saving replaces the existing key\./)).toBeVisible();
    await page.getByLabel("Name", { exact: true }).fill("pin");
    await page.getByRole("button", { name: "Save key" }).click();
    await expect(toastWith(page, "Saved pin. Commands that use it get $PIN.")).toBeVisible();
    await expect(toastWith(page, /hide|redact/i)).toBeVisible();
    expect(await names(page, "/api/hub/agent-keys", "keys")).toContain("pin");
  });

  test("a secret is added, and removing it asks first", async ({ page }) => {
    await openSection(page, "keys");
    const secrets = page.getByRole("list", { name: "Stored secrets" });
    await expect(secrets).toContainText("anthropic_key");

    await page.getByRole("button", { name: "Add a secret" }).click();
    await page.getByLabel("Name", { exact: true }).fill("openai_key");
    await expect(page.getByText("Saving replaces the stored secret with this name.")).toBeVisible();
    await page.getByLabel("Name", { exact: true }).fill("discord");
    await page.getByLabel("Value", { exact: true }).fill("xoxb-not-shown");
    await expectNoAxeViolations(page, { within: OVERLAY });
    await page.getByRole("button", { name: "Save secret" }).click();
    await expect(toastWith(page, "Saved discord.")).toBeVisible();
    await expect(secrets).toContainText("discord");
    await expect(page.locator(OVERLAY)).not.toContainText("xoxb-not-shown");
    expect(await names(page, "/api/hub/secrets", "names")).toContain("discord");

    await page.getByRole("button", { name: "Remove discord" }).click();
    const confirm = page.getByRole("alertdialog", { name: "Remove discord?" });
    await expect(confirm).toBeVisible();
    await expectNoAxeViolations(page, { within: "[role=alertdialog]" });
    await confirm.getByRole("button", { name: "Cancel" }).click();
    await expect(confirm).toBeHidden();
    await expect(secrets).toContainText("discord");

    await page.getByRole("button", { name: "Remove discord" }).click();
    await page.getByRole("alertdialog").getByRole("button", { name: "Remove secret" }).click();
    await expect(toastWith(page, "Removed discord.")).toBeVisible();
    await expect(secrets).not.toContainText("discord");
    expect(await names(page, "/api/hub/secrets", "names")).not.toContain("discord");
  });
});

test.describe("Agent-to-agent listener", () => {
  test("the switch and port are staged and saved with Save changes, and the port dims while it is off", async ({
    page,
  }) => {
    await openSection(page, "listener");
    await expect(page.getByLabel("Listener port")).toHaveAttribute("placeholder", "7702");
    await expectNoAxeViolations(page, { within: OVERLAY });

    await page.getByRole("switch", { name: "Let other agents reach this install" }).click();
    await expect(page.getByLabel("Listener port")).toBeDisabled();
    await expect(page.getByLabel("Your own address")).toBeDisabled();
    await expect(saveBar(page)).toContainText("You have unsaved changes.");
    expect(await (await page.request.get(HUB_CONFIG)).text()).not.toContain("enabled = false");

    await saveBar(page).getByRole("button", { name: "Save changes" }).click();
    await expect(toastWith(page, "Saved the install-wide config.toml.")).toBeVisible();
    await expect(saveBar(page)).toBeHidden();
    expect(await (await page.request.get(HUB_CONFIG)).text()).toContain("enabled = false");

    await page.reload();
    await expect(
      page.getByRole("switch", { name: "Let other agents reach this install" }),
    ).not.toBeChecked();
  });

  test("a caller key is created with its token shown once, then revoked, and Undo brings it back", async ({
    page,
  }) => {
    await openSection(page, "listener");
    const callers = page.getByRole("list", { name: "Caller keys" });
    await expect(callers).toContainText("laptop");

    await page.getByRole("button", { name: "Add a caller key" }).click();
    await page.getByLabel("Name", { exact: true }).fill("laptop");
    await expect(page.getByText("A key named laptop already exists.")).toBeVisible();
    await expect(page.getByRole("button", { name: "Create key" })).toBeDisabled();
    await page.getByLabel("Name", { exact: true }).fill("phone");
    await page.getByLabel("Description").fill("My phone");
    await page.getByRole("button", { name: "Create key" }).click();

    const reveal = page
      .getByText("Key for phone created.")
      .locator("xpath=ancestor::*[@role='status'][1]");
    await expect(reveal).toContainText(/rsdm_a2a_mock\w+/);
    await expect(reveal).toContainText("you won't see it again");
    await expect(page.getByRole("button", { name: "Copy key" })).toBeFocused();
    await expect(callers).toContainText("phone");
    await expectNoAxeViolations(page, { within: OVERLAY });
    const listed = await (await page.request.get("/api/hub/a2a/keys")).text();
    expect(listed).toContain("phone");
    expect(listed).not.toContain("rsdm_a2a_mock");

    await page.getByRole("button", { name: "Done" }).click();
    await expect(page.locator(OVERLAY)).not.toContainText("rsdm_a2a_mock");

    await page.getByRole("button", { name: "Revoke phone" }).click();
    await expect(
      toastWith(page, "Revoked phone. It can no longer reach your agents."),
    ).toBeVisible();
    await expect(callers).not.toContainText("phone");
    expect(await names(page, "/api/hub/a2a/keys", "keys")).not.toContain("phone");

    await undo(page).click();
    await expect(toastWith(page, "Restored.")).toBeVisible();
    await expect(callers).toContainText("phone");
    expect(await names(page, "/api/hub/a2a/keys", "keys")).toContain("phone");
  });

  test("Copy puts the token on the clipboard", async ({ page, browserName }) => {
    test.skip(browserName !== "chromium", "Only Chromium lets a test grant clipboard access.");
    await page.context().grantPermissions(["clipboard-read", "clipboard-write"]);
    await openSection(page, "listener");
    await page.getByRole("button", { name: "Add a caller key" }).click();
    await page.getByLabel("Name", { exact: true }).fill("tablet");
    await page.getByRole("button", { name: "Create key" }).click();
    const token = await page
      .locator("code")
      .filter({ hasText: /^rsdm_a2a_mock/ })
      .innerText();

    await page.getByRole("button", { name: "Copy key" }).click();
    await expect(page.getByRole("button", { name: "Copied" })).toBeVisible();
    expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(token);
  });

  test("the agent page's link opens the install's listener", async ({ page }) => {
    await page.goto("/agent/atlas?settings=atlas/a2a");
    await page.getByRole("button", { name: "Change for all agents" }).click();
    await expect(page.getByRole("heading", { name: "Agent-to-agent", level: 2 })).toBeVisible();
    await expect(
      page.getByRole("switch", { name: "Let other agents reach this install" }),
    ).toBeVisible();
    await expect.poll(() => new URL(page.url()).searchParams.get("settings")).toBe("_all/listener");
  });
});

test.describe("Notifications", () => {
  test("holds its place with nothing to set", async ({ page }) => {
    await openSection(page, "notifications");
    await expect(page.getByRole("heading", { name: "Notifications", level: 2 })).toBeVisible();
    await expect(page.getByText(/aren't available in this version/)).toBeVisible();
    await expectNoAxeViolations(page, { within: OVERLAY });
  });
});

test.describe("Raw config and History", () => {
  test("the install's config.toml is edited and saved as a whole file", async ({ page }) => {
    await openSection(page, "raw");
    const text = page.getByRole("textbox", { name: "Contents of config.toml" });
    await expect(page.getByText("No problems found.")).toBeVisible();
    await expect(page.getByRole("tab")).toHaveCount(0);
    await expectNoAxeViolations(page, { within: OVERLAY });

    const before = await text.inputValue();
    expect(before).toContain('timezone = "America/New_York"');
    await text.fill(before.replace('timezone = "America/New_York"', 'timezone = "Europe/Berlin"'));
    await expect(page.getByText("No problems found.")).toBeVisible();
    await page.getByRole("button", { name: "Save config.toml" }).click();

    await expect(toastWith(page, "Saved config.toml.")).toBeVisible();
    expect(await (await page.request.get(HUB_CONFIG)).text()).toContain(
      'timezone = "Europe/Berlin"',
    );
  });

  test("History shows the shared files and the install's config, and restores a key store through its checkpoint", async ({
    page,
  }) => {
    // A checkpoint holds the key store as it was before the action, so the one before the removal
    // shows the store changed by the key added just ahead of it.
    await page.request.post("/api/hub/agent-keys", {
      data: { name: "tmp_key", value: "abcdefghij", description: "" },
    });
    await page.request.delete("/api/hub/agent-keys/github_token");
    expect(await names(page, "/api/hub/agent-keys", "keys")).not.toContain("github_token");

    await openSection(page, "history");
    await expect(page.getByRole("radio", { name: "Shared files" })).toBeChecked();
    await page.getByRole("radio", { name: "Install-wide config" }).click();
    await expect(page.getByRole("list", { name: "Install-wide config checkpoints" })).toBeVisible();

    await page.getByRole("button", { name: /^delete agent key 'github_token'/ }).click();
    await expect(
      page.getByText("Restores the saved agent keys to how they were at this point."),
    ).toBeVisible();
    await expectNoAxeViolations(page, { within: OVERLAY });

    await page.getByRole("button", { name: "Restore agent-keys.toml.enc as it was then" }).click();
    await expect(toastWith(page, "Restored agent-keys.toml.enc.")).toBeVisible();
    expect(await names(page, "/api/hub/agent-keys", "keys")).toContain("github_token");
  });

  test("History restores the install's config.toml and the raw editor reads it again", async ({
    page,
  }) => {
    await openSection(page, "history");
    await page.getByRole("radio", { name: "Install-wide config" }).click();
    await page.getByRole("button", { name: /^config patch/ }).click();
    await page.getByRole("button", { name: "Changes to config.toml" }).click();
    await expect(page.getByRole("region", { name: "Changes to config.toml" })).toBeVisible();
    const live = await (await page.request.get(HUB_CONFIG)).text();

    await page.getByRole("button", { name: "Restore config.toml as it was then" }).click();

    await expect(toastWith(page, "Restored config.toml.")).toBeVisible();
    const restored = await (await page.request.get(HUB_CONFIG)).text();
    expect(restored).not.toBe(live);
    expect(live.startsWith(restored.trimEnd())).toBe(true);
  });
});
