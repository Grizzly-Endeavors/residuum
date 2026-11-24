import { createRequire } from "node:module";
import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";
import type axeCore from "axe-core";
import { settleAnimations } from "./app";

const builderRequire = createRequire(import.meta.resolve("@axe-core/playwright"));
const axe = builderRequire("axe-core") as typeof axeCore;

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

const NODES_SHOWN = 3;

export interface AxeScanOptions {
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
