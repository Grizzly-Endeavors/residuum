import type { Locator, Page } from "@playwright/test";
import { expectNoAxeViolations } from "../support/axe";
import { expect, test } from "../support/fixtures";
import { expectPaletteOpen } from "../support/lazy";

/**
 * Installing the app: the manifest and its icons, the document's
 * metas, and the Install app entry in the palette and the help menu. The
 * `@preview` specs run on the production build, where the manifest is as it
 * ships; the rest run on the dev server.
 */

const IPHONE_USER_AGENT =
  "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1";

interface Manifest {
  id: string;
  start_url: string;
  scope: string;
  display: string;
  background_color: string;
  theme_color: string;
  icons: { src: string; sizes: string; type: string; purpose?: string }[];
  shortcuts: { name: string; url: string }[];
}

/** The width and height a PNG file declares in its header. */
function pngSize(bytes: Buffer): { width: number; height: number } {
  return { width: bytes.readUInt32BE(16), height: bytes.readUInt32BE(20) };
}

/**
 * How far from the centre, as a share of the icon's width, the farthest pixel
 * that differs from the background lies. A maskable icon's content must stay
 * inside the circle of radius 0.4, or a platform's mask cuts it.
 */
async function farthestContent(page: Page, src: string): Promise<number> {
  return page.evaluate(async (url) => {
    const image = new Image();
    image.src = url;
    await image.decode();
    const canvas = document.createElement("canvas");
    canvas.width = image.naturalWidth;
    canvas.height = image.naturalHeight;
    const context = canvas.getContext("2d");
    if (context === null) throw new Error("no 2d canvas");
    context.drawImage(image, 0, 0);
    const { data, width, height } = context.getImageData(0, 0, canvas.width, canvas.height);
    const [bgRed, bgGreen, bgBlue] = [data[0] ?? 0, data[1] ?? 0, data[2] ?? 0];
    let farthest = 0;
    for (let y = 0; y < height; y++) {
      for (let x = 0; x < width; x++) {
        const at = (y * width + x) * 4;
        const differs =
          Math.abs((data[at] ?? 0) - bgRed) +
            Math.abs((data[at + 1] ?? 0) - bgGreen) +
            Math.abs((data[at + 2] ?? 0) - bgBlue) >
          24;
        if (differs)
          farthest = Math.max(farthest, Math.hypot(x + 0.5 - width / 2, y + 0.5 - height / 2));
      }
    }
    return farthest / width;
  }, src);
}

test.describe("the production build", { tag: "@preview" }, () => {
  test("carries the manifest link, the iOS metas and the viewport that fills the screen", async ({
    page,
  }) => {
    await page.goto("/home");
    await expect(page.getByRole("heading", { name: "Home", level: 1 })).toBeVisible();

    const manifest = page.locator('link[rel="manifest"]');
    await expect(manifest).toHaveAttribute("href", "/manifest.webmanifest");
    // Without it the browser fetches the manifest with no cookies, which the relay redirects to its login page.
    await expect(manifest).toHaveAttribute("crossorigin", "use-credentials");
    await expect(page.locator('meta[name="viewport"]')).toHaveAttribute(
      "content",
      /viewport-fit=cover/,
    );
    await expect(page.locator('meta[name="theme-color"]')).toHaveAttribute("content", "#0e0e10");
    await expect(page.locator('meta[name="apple-mobile-web-app-capable"]')).toHaveAttribute(
      "content",
      "yes",
    );
    await expect(
      page.locator('meta[name="apple-mobile-web-app-status-bar-style"]'),
    ).toHaveAttribute("content", "black-translucent");
    await expect(page.locator('link[rel="apple-touch-icon"]')).toHaveAttribute(
      "href",
      "/icons/apple-touch-icon.png",
    );
  });

  test("serves a manifest that makes Home the app's start and a shortcut to the Inbox", async ({
    page,
    request,
  }) => {
    await page.goto("/home");
    const response = await request.get("/manifest.webmanifest");
    expect(response.status()).toBe(200);
    expect(response.headers()["content-type"]).toContain("manifest+json");
    const manifest = (await response.json()) as Manifest;

    expect(manifest).toMatchObject({
      id: "/home",
      start_url: "/home",
      scope: "/",
      display: "standalone",
      background_color: "#0e0e10",
      theme_color: "#0e0e10",
    });
    expect(manifest.shortcuts.map(({ name, url }) => [name, url])).toEqual([
      ["Home", "/home"],
      ["Inbox", "/inbox"],
    ]);
  });

  test("serves icons at the sizes it declares, and a maskable icon whose content survives the mask", async ({
    page,
    request,
  }) => {
    await page.goto("/home");
    const manifest = (await (await request.get("/manifest.webmanifest")).json()) as Manifest;
    const pngs = manifest.icons.filter((icon) => icon.type === "image/png");
    expect(pngs.map((icon) => icon.sizes).sort()).toEqual(["192x192", "512x512", "512x512"]);
    expect(pngs.filter((icon) => icon.purpose === "maskable")).toHaveLength(1);

    for (const icon of manifest.icons) {
      const response = await request.get(icon.src);
      expect(response.status(), icon.src).toBe(200);
      expect(response.headers()["content-type"], icon.src).toContain(icon.type);
      if (icon.type !== "image/png") continue;
      const [width, height] = icon.sizes.split("x").map(Number);
      expect(pngSize(await response.body()), icon.src).toEqual({ width, height });
    }

    const maskable = pngs.find((icon) => icon.purpose === "maskable");
    expect(await farthestContent(page, maskable?.src ?? "")).toBeLessThanOrEqual(0.4);
  });
});

/** Fire the event a Chromium browser fires when the app can be installed, counting the times its prompt runs. */
async function offerInstall(page: Page): Promise<void> {
  await page.evaluate(() => {
    const prompt = (): Promise<void> => {
      Reflect.set(window, "installPrompts", Number(Reflect.get(window, "installPrompts") ?? 0) + 1);
      return Promise.resolve();
    };
    window.dispatchEvent(
      Object.assign(new Event("beforeinstallprompt", { cancelable: true }), { prompt }),
    );
  });
}

const installPrompts = (page: Page): Promise<number> =>
  page.evaluate(() => Number(Reflect.get(window, "installPrompts") ?? 0));

async function openPalette(page: Page, isMobile: boolean): Promise<Locator> {
  if (isMobile) {
    await page
      .getByRole("navigation", { name: "Main" })
      .getByRole("button", { name: "Search" })
      .click();
  } else {
    await page.keyboard.press("ControlOrMeta+k");
  }
  const palette = await expectPaletteOpen(page);
  await expect(palette.getByRole("combobox")).toBeFocused();
  return palette;
}

async function openHelpMenu(page: Page, isMobile: boolean): Promise<Locator> {
  if (isMobile) {
    await page
      .getByRole("navigation", { name: "Main" })
      .getByRole("button", { name: "Menu" })
      .click();
  }
  await page
    .getByRole("navigation", { name: "Places and agents" })
    .getByRole("button", { name: "Help" })
    .click();
  const menu = page.getByRole("menu", { name: "Help" });
  await expect(menu).toBeVisible();
  return menu;
}

/** Close the help menu, and on a phone the drawer it opened in. */
async function closeHelpMenu(page: Page, isMobile: boolean): Promise<void> {
  await page.keyboard.press("Escape");
  await expect(page.getByRole("menu", { name: "Help" })).toBeHidden();
  if (!isMobile) return;
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog", { name: "Agents and places" })).toBeHidden();
}

test.describe("Install app", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/home");
    await expect(page.getByRole("heading", { name: "Home", level: 1 })).toBeVisible();
  });

  test("is not listed until the browser offers the install prompt", async ({ page, isMobile }) => {
    const palette = await openPalette(page, isMobile);
    await page.keyboard.type("install");
    await expect(palette.getByRole("option", { name: /Install app/ })).toHaveCount(0);
  });

  test("appears in the palette and the help menu together, and runs the browser's prompt once", async ({
    page,
    isMobile,
  }) => {
    await offerInstall(page);

    const menu = await openHelpMenu(page, isMobile);
    await expect(menu.getByRole("menuitem", { name: "Install app" })).toBeVisible();
    await closeHelpMenu(page, isMobile);

    const palette = await openPalette(page, isMobile);
    await page.keyboard.type("install");
    await expect(palette.getByRole("option", { name: /Install app/ })).toHaveCount(1);
    await page.keyboard.press("Enter");
    await expect(palette).toBeHidden();
    await expect.poll(() => installPrompts(page)).toBe(1);

    // The browser's prompt shows once, so the entry goes with it.
    await openPalette(page, isMobile);
    await page.keyboard.type("install");
    await expect(
      page.getByRole("dialog", { name: "Search and commands" }).getByRole("option", {
        name: /Install app/,
      }),
    ).toHaveCount(0);
  });

  test("goes when the app is installed", async ({ page, isMobile }) => {
    await offerInstall(page);
    await page.evaluate(() => window.dispatchEvent(new Event("appinstalled")));
    const palette = await openPalette(page, isMobile);
    await page.keyboard.type("install");
    await expect(palette.getByRole("option", { name: /Install app/ })).toHaveCount(0);
  });
});

test.describe("Install app without a secure context", () => {
  test("is hidden even when the browser offers its prompt", async ({ page, isMobile }) => {
    // Plain-HTTP LAN access: localhost counts as secure, so the page is told otherwise before it starts.
    await page.addInitScript(() => {
      Object.defineProperty(window, "isSecureContext", { value: false });
    });
    await page.goto("/home");
    await expect(page.getByRole("heading", { name: "Home", level: 1 })).toBeVisible();
    await offerInstall(page);

    const menu = await openHelpMenu(page, isMobile);
    await expect(menu.getByRole("menuitem", { name: "Keyboard shortcuts" })).toBeVisible();
    await expect(menu.getByRole("menuitem", { name: "Install app" })).toHaveCount(0);
    await closeHelpMenu(page, isMobile);

    const palette = await openPalette(page, isMobile);
    await page.keyboard.type("install");
    await expect(palette.getByRole("option", { name: /Install app/ })).toHaveCount(0);
  });
});

test.describe("Install app on an iPhone", () => {
  test.use({ userAgent: IPHONE_USER_AGENT });

  test("opens the Add to Home Screen steps, with no browser prompt to run", async ({
    page,
    isMobile,
  }) => {
    await page.goto("/home");
    await expect(page.getByRole("heading", { name: "Home", level: 1 })).toBeVisible();

    const palette = await openPalette(page, isMobile);
    await page.keyboard.type("install");
    await page.keyboard.press("Enter");
    await expect(palette).toBeHidden();

    const steps = page.getByRole("dialog", { name: "Add Residuum to your Home Screen" });
    await expect(steps).toBeVisible();
    await expect(steps.getByText("Add to Home Screen", { exact: true })).toBeVisible();
    await expectNoAxeViolations(page, { within: "[data-overlay-host]" });

    await steps.getByRole("button", { name: "Done" }).click();
    await expect(steps).toBeHidden();
  });
});
