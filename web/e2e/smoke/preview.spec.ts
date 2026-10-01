import { expect, test } from "../support/fixtures";

// `@preview` specs run against the production build served with the mock, the
// place for what the dev server can't show: the service worker, installability,
// and the bundle as it ships.
test(
  "the production build serves the app and the mock API",
  { tag: "@preview" },
  async ({ page }) => {
    const requested: string[] = [];
    page.on("request", (request) => {
      requested.push(new URL(request.url()).pathname);
    });

    await page.goto("/agent/atlas");
    await expect(page.getByText("Residuum connected")).toBeVisible();

    expect(requested.some((path) => /^\/assets\/index-.+\.js$/.test(path))).toBe(true);
    expect(
      requested.filter((path) => path.startsWith("/@vite") || path.startsWith("/src/")),
    ).toEqual([]);
  },
);
