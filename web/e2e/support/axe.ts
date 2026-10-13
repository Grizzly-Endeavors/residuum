/**
 * The accessibility scan: axe-core over the page as it stands, failing the test
 * on any serious or critical violation.
 *
 * Scan every place and overlay a change touches, once it has settled into the
 * state under test (open the menu, then scan). Minor and moderate findings are
 * in the attached report but don't fail.
 */
import { createRequire } from "node:module";
import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";
import type axeCore from "axe-core";
import { settleAnimations } from "./app";

/**
 * The axe-core that `@axe-core/playwright` depends on, so the scan runs the
 * same version the builder was released with.
 */
const builderRequire = createRequire(import.meta.resolve("@axe-core/playwright"));
const axe = builderRequire("axe-core") as typeof axeCore;

/**
 * axe-core as the builder injects it, but timed by the browser's own timers.
 * axe waits on `setTimeout` between its rules, and the page's `setTimeout` is
 * the test clock's (see `fixtures.ts`), which in manual time stands still until
 * the spec moves it, so a scan would never end. The page's own code keeps the
 * test clock's timers. A frame without the clock has native timers already.
 */
const AXE_ON_BROWSER_TIMERS = `(function (setTimeout, clearTimeout) {
${axe.source}
}).apply(globalThis, (() => {
  const clock = globalThis.__pwClock;
  return clock === undefined
    ? [setTimeout, clearTimeout]
    : [clock.builtins.setTimeout, clock.builtins.clearTimeout];
})());`;

type Violation = Awaited<ReturnType<AxeBuilder["analyze"]>>["violations"][number];

const BLOCKING_IMPACTS: ReadonlySet<string | null | undefined> = new Set(["serious", "critical"]);

/** How many of a violation's elements are named in the failure. */
const NODES_SHOWN = 3;

export interface AxeScanOptions {
  /** Limit the scan to the part of the page under this selector, such as an open dialog. */
  within?: string;
}

function describeViolation(violation: Violation): string {
  const nodes = violation.nodes
    .slice(0, NODES_SHOWN)
    .map((node) => node.target.join(" "))
    .join(" | ");
  const more =
    violation.nodes.length > NODES_SHOWN ? ` (+${violation.nodes.length - NODES_SHOWN} more)` : "";
  const impact = violation.impact ?? "unknown";
  return `${violation.id} [${impact}]: ${violation.help}, at ${nodes}${more}. ${violation.helpUrl}`;
}

/**
 * Scan `page` with axe and fail on serious and critical violations. Entrance
 * animations finish first (`settleAnimations`); spinners and other endless
 * ones are left running.
 */
export async function expectNoAxeViolations(
  page: Page,
  options: AxeScanOptions = {},
): Promise<void> {
  const { within } = options;
  await settleAnimations(page);
  const builder = new AxeBuilder({ page, axeSource: AXE_ON_BROWSER_TIMERS });
  if (within !== undefined) builder.include(within);
  const results = await builder.analyze();

  await test.info().attach("axe-violations.json", {
    body: JSON.stringify(results.violations, null, 2),
    contentType: "application/json",
  });

  const blocking = results.violations.filter((violation) => BLOCKING_IMPACTS.has(violation.impact));
  expect(blocking.map(describeViolation), "serious or critical accessibility violations").toEqual(
    [],
  );
}
