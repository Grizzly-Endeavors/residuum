/**
 * Waiting for the parts of the app whose code loads the first time they open:
 * Settings, the command palette, the file view with its editor, and the setup
 * wizard (`src/lib/lazy-component.svelte.ts`).
 *
 * Opening one fetches its chunk, and until the chunk arrives nothing of it is
 * on the page. On the dev server a chunk is dozens of modules, which takes
 * seconds when other suites share the machine, so a spec that opens one waits
 * for it to mount here, with the bound for code that loads
 * (`LOAD_TIMEOUT`), rather than with the default of waits that don't. A page
 * opened by a URL that names one (`?settings=`, `?panel=file:`) has been waited
 * for already by `page.goto`; these are for the ones a spec opens by acting.
 */
import { expect, type Locator, type Page } from "@playwright/test";
import { LOAD_TIMEOUT } from "./app";

async function mounted(target: Locator): Promise<Locator> {
  await expect(target).toBeVisible({ timeout: LOAD_TIMEOUT });
  return target;
}

/** The Settings dialog, once its code has loaded and it has mounted. */
export async function expectSettingsOpen(page: Page): Promise<Locator> {
  return mounted(page.getByRole("dialog", { name: "Settings" }));
}

/** The command palette's dialog, once its code has loaded and it has mounted. */
export async function expectPaletteOpen(page: Page): Promise<Locator> {
  return mounted(page.getByRole("dialog", { name: "Search and commands" }));
}

/** The editor of the file `name` in the context panel, once the file view's code has loaded and it has mounted. */
export async function expectFileOpen(page: Page, name: string): Promise<Locator> {
  return mounted(page.getByRole("textbox", { name: `Contents of ${name}` }));
}

/** The setup wizard's heading, once its code has loaded and it has mounted. */
export async function expectSetupOpen(page: Page): Promise<Locator> {
  return mounted(page.getByRole("heading", { name: "Welcome to Residuum", level: 1 }));
}
