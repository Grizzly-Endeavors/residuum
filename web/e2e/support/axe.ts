/**
 * The accessibility scan: axe-core over the page as it stands, failing the test
 * on any serious or critical violation.
 *
 * Scan every place and overlay a change touches, once it has settled into the
 * state under test (open the menu, then scan). Minor and moderate findings are
 * in the attached report but don't fail.
 */
import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";
import { settleAnimations } from "./app";

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
  const builder = new AxeBuilder({ page });
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
