import { expect, type Locator, type Page } from "@playwright/test";
import { LOAD_TIMEOUT } from "./app";

async function mounted(target: Locator): Promise<Locator> {
  await expect(target).toBeVisible({ timeout: LOAD_TIMEOUT });
  return target;
}

export async function expectSettingsOpen(page: Page): Promise<Locator> {
  return mounted(page.getByRole("dialog", { name: "Settings" }));
}

export async function expectPaletteOpen(page: Page): Promise<Locator> {
  return mounted(page.getByRole("dialog", { name: "Search and commands" }));
}

export async function expectFileOpen(page: Page, name: string): Promise<Locator> {
  return mounted(page.getByRole("textbox", { name: `Contents of ${name}` }));
}

export async function expectFileRendered(page: Page, heading: string): Promise<Locator> {
  return mounted(page.getByRole("heading", { name: heading, exact: true }));
}

export async function expectSetupOpen(page: Page): Promise<Locator> {
  return mounted(page.getByRole("heading", { name: "Welcome to Residuum", level: 1 }));
}
