import type { Page } from "@playwright/test";
import { expectNoAxeViolations } from "../support/axe";
import { expect, test } from "../support/fixtures";

const SHELL_CACHE_PREFIX = "residuum-shell:";

async function openWithWorker(page: Page): Promise<void> {
  await page.goto("/home");
  await expect(page.getByRole("heading", { name: "Home", level: 1 })).toBeVisible();
  await expect
    .poll(() =>
      page.evaluate(async (prefix) => {
        const shell = (await caches.keys()).find((name) => name.startsWith(prefix));
        const document =
          shell === undefined ? null : await (await caches.open(shell)).match("/index.html");
        return navigator.serviceWorker.controller !== null && document !== null;
      }, SHELL_CACHE_PREFIX),
    )
    .toBe(true);
}

function cachedPaths(page: Page): Promise<Record<string, string[]>> {
  return page.evaluate(async () => {
    const held: Record<string, string[]> = {};
    for (const name of await caches.keys()) {
      const requests = await (await caches.open(name)).keys();
      held[name] = requests.map((request) => new URL(request.url).pathname).sort();
    }
    return held;
  });
}

async function shellCaches(page: Page): Promise<string[]> {
  const names = Object.keys(await cachedPaths(page));
  return names.filter((name) => name.startsWith(SHELL_CACHE_PREFIX)).sort();
}

function activatedVersions(page: Page): Promise<{ current: string; previous: string | null }> {
  return page.evaluate(async () => {
    const stored = await (await caches.open("residuum-generations")).match("/generations");
    return (await stored?.json()) as { current: string; previous: string | null };
  });
}

async function showPageAgain(page: Page): Promise<void> {
  await page.evaluate(() => document.dispatchEvent(new Event("visibilitychange")));
}

test("the dev server and the mock's dev mode never register a worker", async ({ page }) => {
  await page.goto("/home");
  await expect(page.getByRole("heading", { name: "Home", level: 1 })).toBeVisible();
  await page.waitForLoadState("load");
  const registrations = await page.evaluate(
    async () => (await navigator.serviceWorker.getRegistrations()).length,
  );
  expect(registrations).toBe(0);
});

test.describe("the service worker", { tag: "@preview" }, () => {
  test("is served from the root as JavaScript that is checked again on every load", async ({
    request,
  }) => {
    const response = await request.get("/sw.js");
    expect(response.status()).toBe(200);
    expect(response.headers()["content-type"]).toMatch(/javascript/);
    expect(response.headers()["cache-control"]).toBe("no-cache");
    expect(await response.text()).toMatch(/^\/\* residuum-sw [0-9a-f]{12} \*\//);
  });

  test("registers after the first load, controls the whole app and holds the shell", async ({
    page,
  }) => {
    await openWithWorker(page);

    const registration = await page.evaluate(async () => {
      const found = await navigator.serviceWorker.getRegistration();
      return { scope: found?.scope, script: found?.active?.scriptURL };
    });
    const { origin } = new URL(page.url());
    expect(registration).toEqual({ scope: `${origin}/`, script: `${origin}/sw.js` });

    const caches = await cachedPaths(page);
    const [name, ...others] = await shellCaches(page);
    expect(others).toEqual([]);
    const shell = caches[name ?? ""] ?? [];
    expect(shell).toContain("/index.html");
    expect(shell).toContain("/icons/icon-192.png");
    expect(shell.some((path) => /^\/assets\/index-.+\.js$/.test(path))).toBe(true);
    expect(shell.some((path) => /^\/assets\/SettingsModal-.+\.js$/.test(path))).toBe(true);
    expect(shell.some((path) => /^\/assets\/onest-latin-400-.+\.woff2$/.test(path))).toBe(true);
    expect(shell).not.toContain("/manifest.webmanifest");
    expect(shell).not.toContain("/mcp-catalog.json");
  });

  test("never answers or caches /api", async ({ page }) => {
    await openWithWorker(page);
    const answered: { path: string; fromWorker: boolean }[] = [];
    page.on("response", (response) => {
      answered.push({
        path: new URL(response.url()).pathname,
        fromWorker: response.fromServiceWorker(),
      });
    });

    await page.goto("/agent/atlas");
    await expect(page.getByRole("heading", { name: "atlas", level: 1 })).toBeVisible();
    await page.goto("/inbox");
    await expect(page.getByRole("heading", { name: "Inbox", level: 1 })).toBeVisible();
    await page.evaluate(() => fetch("/api/hub/agents").then((response) => response.status));
    await page.evaluate(() =>
      fetch("/api/agents/atlas/chat/history").then((response) => response.status),
    );

    const api = answered.filter((response) => response.path.startsWith("/api/"));
    expect(api.length).toBeGreaterThan(0);
    expect(api.filter((response) => response.fromWorker)).toEqual([]);
    expect(
      answered.some((response) => response.path.startsWith("/assets/") && response.fromWorker),
    ).toBe(true);

    for (const [name, paths] of Object.entries(await cachedPaths(page))) {
      expect(
        paths.filter((path) => path.startsWith("/api")),
        name,
      ).toEqual([]);
    }
  });

  test("opens the shell and the hub banner on a reload with no network", async ({
    page,
    context,
    mock,
  }) => {
    await openWithWorker(page);
    await mock.manualTime();

    await context.setOffline(true);
    await page.reload();

    await expect(page.getByRole("heading", { name: "Home", level: 1 })).toBeVisible();
    const banner = page.getByRole("status").filter({ hasText: "Can't reach Residuum." });
    await expect(banner).toBeVisible();
    await expectNoAxeViolations(page);

    const retry = banner.getByRole("button", { name: "Retry" });
    await expect(retry).not.toHaveAttribute("aria-busy", "true");
    await context.setOffline(false);
    await retry.click();
    await expect(banner).toBeHidden();
  });

  test("opens any client route with no network, not only the first page", async ({
    page,
    context,
  }) => {
    await openWithWorker(page);
    await context.setOffline(true);
    await page.goto("/agent/atlas/files");
    await expect(
      page.getByRole("status").filter({ hasText: "Can't reach Residuum." }),
    ).toBeVisible();
    expect(new URL(page.url()).pathname).toBe("/agent/atlas/files");
  });

  test("shows Not Found for a page load of a file that isn't cached, with no network", async ({
    page,
    context,
  }) => {
    await openWithWorker(page);
    await context.setOffline(true);
    await page.goto("/foo.json");
    await expect(
      page.getByRole("status").filter({ hasText: "Can't reach Residuum." }),
    ).toBeVisible();
    await expect(page.getByRole("heading", { name: "Page not found", level: 1 })).toBeVisible();
    expect(new URL(page.url()).pathname).toBe("/foo.json");
  });
});

test.describe("an app update", { tag: "@preview" }, () => {
  async function reload(page: Page): Promise<void> {
    const loaded = page.waitForEvent("load");
    await page
      .getByRole("status")
      .filter({ hasText: "Update ready." })
      .getByRole("button", { name: "Reload" })
      .click();
    await loaded;
  }

  test("shows Update ready when the app was rebuilt, and Reload makes the new version active", async ({
    page,
    mock,
  }) => {
    await openWithWorker(page);
    const first = (await activatedVersions(page)).current;
    await expect(page.getByText("Update ready.")).toHaveCount(0);

    await mock.post("/api/mock/rebuild");
    await showPageAgain(page);

    const banner = page.getByRole("status").filter({ hasText: "Update ready." });
    await expect(banner).toBeVisible();
    await expectNoAxeViolations(page);
    expect((await activatedVersions(page)).current).toBe(first);
    const waiting = await page.evaluate(
      async () => (await navigator.serviceWorker.getRegistration())?.waiting != null,
    );
    expect(waiting).toBe(true);

    await reload(page);
    await expect(page.getByRole("heading", { name: "Home", level: 1 })).toBeVisible();
    await expect(banner).toBeHidden();

    expect(await activatedVersions(page)).toEqual({
      current: `${first}-rebuild-1`,
      previous: first,
    });
    expect(await shellCaches(page)).toEqual(
      [`${SHELL_CACHE_PREFIX}${first}`, `${SHELL_CACHE_PREFIX}${first}-rebuild-1`].sort(),
    );
  });

  test("keeps a page opened before the rebuild working after another window updates", async ({
    page,
    context,
    mock,
  }) => {
    await openWithWorker(page);
    const first = (await activatedVersions(page)).current;
    const held = (await cachedPaths(page))[`${SHELL_CACHE_PREFIX}${first}`] ?? [];
    const chunk = held.find((path) => /^\/assets\/SettingsModal-.+\.js$/.test(path)) ?? "";
    expect(chunk).not.toBe("");

    await mock.post("/api/mock/rebuild");
    await showPageAgain(page);
    await expect(page.getByText("Update ready.")).toBeVisible();

    await page.evaluate(async () => {
      const registration = await navigator.serviceWorker.getRegistration();
      const changed = new Promise((resolve) => {
        navigator.serviceWorker.addEventListener("controllerchange", resolve, { once: true });
      });
      registration?.waiting?.postMessage({ type: "skip-waiting" });
      await changed;
    });
    await expect
      .poll(async () => (await activatedVersions(page)).current)
      .toBe(`${first}-rebuild-1`);

    await page.evaluate(async ({ name, path }) => (await caches.open(name)).delete(path), {
      name: `${SHELL_CACHE_PREFIX}${first}-rebuild-1`,
      path: chunk,
    });
    await context.setOffline(true);

    const answer = await page.evaluate(
      (path) => fetch(path).then((response) => response.status),
      chunk,
    );
    expect(answer).toBe(200);
    await expect(page.getByText("Update ready.")).toBeVisible();
  });

  test("asks before reloading over unsaved edits", async ({ page, isMobile, mock }) => {
    test.skip(isMobile, "On a phone the editor is a sheet over the banner; the guard is the same.");
    await openWithWorker(page);
    await page.goto("/agent/atlas/files?panel=file:SOUL.md");
    await page
      .getByRole("complementary", { name: "SOUL.md" })
      .getByRole("button", { name: "Edit", exact: true })
      .click();
    const editor = page.getByRole("textbox", { name: "Contents of SOUL.md" });
    await editor.pressSequentially("Unsaved.");
    const first = (await activatedVersions(page)).current;

    await mock.post("/api/mock/rebuild");
    await showPageAgain(page);
    const banner = page.getByRole("status").filter({ hasText: "Update ready." });
    await banner.getByRole("button", { name: "Reload" }).click();

    const question = page.getByRole("alertdialog", { name: "Discard unsaved changes?" });
    await expect(question).toContainText("Unsaved changes to SOUL.md");
    await question.getByRole("button", { name: "Keep editing" }).click();
    await expect(editor).toHaveValue(/Unsaved\.$/);
    expect((await activatedVersions(page)).current).toBe(first);
    await expect(banner).toBeVisible();

    const loaded = page.waitForEvent("load");
    await banner.getByRole("button", { name: "Reload" }).click();
    await question.getByRole("button", { name: "Discard and leave" }).click();
    await loaded;
    expect((await activatedVersions(page)).current).toBe(`${first}-rebuild-1`);
  });
});
