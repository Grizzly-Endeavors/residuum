// Writes the Playwright JSON reports that visual-report.test.ts reads, from runs
// of the Playwright version web/package.json pins. Run it from web/ after bumping
// @playwright/test, then run the unit tests:
//
//   node scripts/fixtures/visual-report/capture/capture.mjs
//
// The baselines are written first, into a scratch directory, by a run that
// updates them; each scenario then compares against them. The reports keep
// only what visual-report.mjs reads, so the paths in them are rewritten to
// placeholders and the files are the same on every machine.

import { spawnSync } from "node:child_process";
import { cpSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const CAPTURE = dirname(fileURLToPath(import.meta.url));
const FIXTURES = resolve(CAPTURE, "..");
const WEB = resolve(CAPTURE, "../../../..");
const REPO = resolve(WEB, "..");
const CONFIG = join(CAPTURE, "playwright.config.mjs");
const PLAYWRIGHT = join(WEB, "node_modules", ".bin", "playwright");

// The scenarios and the report each one writes. `grep` selects tests by their
// full title; `mode` is read by tests/visual.spec.mjs.
const SCENARIOS = {
  "all-match": { mode: "base", grep: "chat|the navigation|settings|the phone layout" },
  "all-skipped": { mode: "base", grep: "the open menu|the phone layout" },
  differ: { mode: "differ", grep: "chat" },
  "new-baseline": { mode: "mixed", grep: "the open menu" },
  errored: { mode: "mixed", grep: "a settings section" },
  "missing-locator": { mode: "mixed", grep: "the missing element" },
  mixed: { mode: "mixed", grep: "chat|the open menu|settings|the navigation", unstable: 1 },
  unstable: { mode: "mixed", grep: "the navigation|the header", unstable: 2 },
  retries: {
    mode: "retries",
    grep: "a finished turn|the composer|a message's details|the navigation",
    retries: 1,
  },
  "repeat-each": { mode: "repeat", grep: "a finished turn", repeat: 2, retries: 1 },
  // Playwright reports a grep that selects nothing as a run error, not as a report without tests.
  "no-tests": { mode: "base", grep: "matches no test at all" },
  "run-error": { mode: "base", tests: "broken" },
};

const SCRATCH_PLACEHOLDER = "/capture-scratch";
const REPO_PLACEHOLDER = "/repo";
const UNSTABLE_MESSAGE = "Failed to take two consecutive stable screenshots";
const MAX_ATTEMPTS = 10;

/**
 * Runs one Playwright pass over `snapshots` and returns its JSON report. A pass
 * that finds a shot without a baseline writes its screenshot there, so every
 * scenario gets its own copy of the baselines.
 */
function runPass(scratch, snapshots, { mode, grep, repeat, retries, tests, update }) {
  const args = ["test", "--config", CONFIG, "--project=visual-desktop", "--workers=1"];
  if (grep !== undefined) args.push("--grep", grep);
  if (repeat !== undefined) args.push(`--repeat-each=${repeat}`);
  if (update) args.push("--update-snapshots");
  const report = join(scratch, "report.json");
  rmSync(report, { force: true });
  const run = spawnSync(PLAYWRIGHT, args, {
    cwd: WEB,
    encoding: "utf8",
    env: {
      ...process.env,
      CAPTURE_SCRATCH: scratch,
      CAPTURE_SNAPSHOTS: snapshots,
      CAPTURE_MODE: mode,
      CAPTURE_RETRIES: String(retries ?? 0),
      CAPTURE_TESTS: tests ?? "tests",
    },
  });
  // Playwright exits non-zero when a test fails, which is what these runs are
  // for; only a run that wrote no report is a failed capture.
  try {
    return JSON.parse(readFileSync(report, "utf8"));
  } catch (error) {
    throw new Error(`no report was written (exit ${run.status}): ${run.stderr}`, { cause: error });
  }
}

/** Replaces the machine's own paths with placeholders, so the reports are the same everywhere. */
function sanitize(report, scratch) {
  const text = JSON.stringify(report, null, 2)
    .replaceAll(realpathSync(scratch), SCRATCH_PLACEHOLDER)
    .replaceAll(scratch, SCRATCH_PLACEHOLDER)
    .replaceAll(realpathSync(REPO), REPO_PLACEHOLDER)
    .replaceAll(REPO, REPO_PLACEHOLDER)
    .replaceAll(process.execPath, "node");
  return `${text}\n`;
}

/** The number of tests whose last run never held still, in a Playwright JSON report node. */
function unstableTests(node) {
  let count = 0;
  for (const spec of node.specs ?? []) {
    for (const test of spec.tests ?? []) {
      const last = test.results?.at(-1);
      if ((last?.errors ?? []).some((e) => `${e.message}`.includes(UNSTABLE_MESSAGE))) count += 1;
    }
  }
  for (const child of node.suites ?? []) count += unstableTests(child);
  return count;
}

/**
 * Runs a scenario until it has the shots it was written for. A shot that never
 * holds still is a race between two screenshots landing in one painted frame, so
 * a scenario with `unstable: n` runs again until n tests report one.
 */
function captureScenario(scratch, baselines, snapshots, scenario) {
  const required = scenario.unstable ?? 0;
  for (let attempt = 1; attempt <= MAX_ATTEMPTS; attempt += 1) {
    rmSync(snapshots, { recursive: true, force: true });
    cpSync(baselines, snapshots, { recursive: true });
    const report = runPass(scratch, snapshots, scenario);
    if (unstableTests({ suites: report.suites ?? [] }) >= required) return report;
  }
  throw new Error(`no run of the scenario had ${required} unstable shots in ${MAX_ATTEMPTS} tries`);
}

const scratch = mkdtempSync(join(tmpdir(), "visual-report-capture-"));
const baselines = join(scratch, "baselines");
const snapshots = join(scratch, "snapshots");
try {
  // The baselines: every shot that a scenario compares is drawn in its base form.
  runPass(scratch, baselines, { mode: "base", update: true });
  for (const [name, scenario] of Object.entries(SCENARIOS)) {
    const report = captureScenario(scratch, baselines, snapshots, scenario);
    writeFileSync(join(FIXTURES, `${name}.json`), sanitize(report, scratch));
    process.stdout.write(`wrote ${name}.json\n`);
  }
} finally {
  rmSync(scratch, { recursive: true, force: true });
}
