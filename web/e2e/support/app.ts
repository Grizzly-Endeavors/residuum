import { expect, type Page } from "@playwright/test";

export const LOAD_TIMEOUT = 20_000;

export type HubState = "connected" | "lost";

export const hubReach = { lost: false };

export const pageClock: { fixedAt: number | null } = { fixedAt: null };

export interface AppReadyOptions {
  hub?: HubState;
}

const APP_ROOT = ".shell, .setup-wizard, .gallery";

const LAZY_LOADING = /^(Opening|Loading settings|Loading the file)$/;

const FAILED_REQUESTS_LISTED = 15;

const failedRequests = new WeakMap<Page, string[]>();

export function recordFailedRequests(page: Page, intended: (url: URL) => boolean): void {
  const failed: string[] = [];
  failedRequests.set(page, failed);
  page.on("request", (request) => {
    if (!request.isNavigationRequest() || request.frame() !== page.mainFrame()) return;
    failed.length = 0;
  });
  page.on("requestfailed", (request) => {
    const url = new URL(request.url());
    if (intended(url)) return;
    const reason = request.failure()?.errorText ?? "failed";
    failed.push(`${request.method()} ${url.pathname} ${reason}`);
  });
}

function explainWithFailedRequests(page: Page, error: unknown): unknown {
  const failed = failedRequests.get(page);
  if (!(error instanceof Error) || failed === undefined || failed.length === 0) return error;
  const listed = failed.slice(0, FAILED_REQUESTS_LISTED);
  const rest = failed.length - listed.length;
  const lines = [
    `${failed.length} request${failed.length === 1 ? "" : "s"} the page made failed:`,
    ...listed.map((line) => `  ${line}`),
  ];
  if (rest > 0) lines.push(`  ...and ${rest} more`);
  error.message = [error.message, ...lines].join("\n");
  return error;
}

async function appRendered(page: Page, hub: HubState): Promise<void> {
  await expect(
    page.locator(APP_ROOT).first(),
    "the app should render its shell, the setup wizard or the gallery",
  ).toBeVisible({ timeout: LOAD_TIMEOUT });

  const shell = page.locator(".shell");
  if ((await shell.count()) === 0) return;
  await expect(shell, `the hub socket should be ${hub}`).toHaveAttribute("data-hub", hub, {
    timeout: LOAD_TIMEOUT,
  });
  if (hub === "connected") {
    await expect(shell, "the hub's overview should have loaded").toHaveAttribute(
      "data-overview",
      "loaded",
      { timeout: LOAD_TIMEOUT },
    );
  }
  if (new URL(page.url()).searchParams.has("settings")) {
    await expect(
      page.getByRole("dialog", { name: "Settings" }),
      "the Settings modal the URL asks for should be open",
    ).toBeVisible({ timeout: LOAD_TIMEOUT });
  }
  await expect(
    page.getByRole("status").filter({ hasText: LAZY_LOADING }),
    "the code a feature opens with should have loaded",
  ).toHaveCount(0, { timeout: LOAD_TIMEOUT });
}

export async function fontsLoaded(page: Page): Promise<void> {
  await page.waitForFunction(() => document.fonts.status === "loaded", undefined, {
    timeout: LOAD_TIMEOUT,
  });
}

export async function imagesLoaded(page: Page): Promise<void> {
  await page.waitForFunction(() => Array.from(document.images).every((image) => image.complete), {
    timeout: LOAD_TIMEOUT,
  });
}

export async function settleAnimations(page: Page): Promise<void> {
  await page.evaluate(() =>
    Promise.all(
      document
        .getAnimations()
        .filter((animation) => animation.effect?.getComputedTiming().endTime !== Infinity)
        .map((animation) => animation.finished.catch(() => undefined)),
    ),
  );
}

export async function waitForApp(page: Page, options: AppReadyOptions = {}): Promise<void> {
  try {
    await appRendered(page, options.hub ?? (hubReach.lost ? "lost" : "connected"));
    await fontsLoaded(page);
  } catch (error) {
    throw explainWithFailedRequests(page, error);
  }
}
