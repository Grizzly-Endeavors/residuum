#!/usr/bin/env node
// Turns the Playwright JSON report of the web visual comparisons into a status
// and the Markdown of the pull request comment (.github/workflows/quality-checks.yml,
// job `web-visual`; .github/workflows/web-pr.yml, job `visual-report`).
//
//   scripts/visual-report.mjs classify <playwright-json> <classification-json>
//   scripts/visual-report.mjs render <classification-json> <markdown-file>
//
// `classify` sorts every visual test into one of four outcomes:
//   match   the screenshot equals its baseline
//   differ  the screenshot differs from its baseline
//   new     the shot has no baseline yet
//   errored the test failed for another reason, such as a timeout
// A report that is missing, unreadable or empty is status `error`; a run where
// every test skipped itself is status `skipped`.
//
// Both subcommands append `status`, `differ`, `new`, `errored` and `matched`
// counts to $GITHUB_OUTPUT when it is set. `render` reads ARTIFACT_URL and
// RUN_URL from the environment, writes the Markdown to the file, and appends it
// to $GITHUB_OUTPUT as `body` and to $GITHUB_STEP_SUMMARY when those are set.

import { appendFileSync, readFileSync, writeFileSync } from "node:fs";
import { randomUUID } from "node:crypto";

// The comment carries this marker so the pull request job finds it again and
// updates it in place instead of adding another.
export const MARKER = "<!-- residuum-visual-report -->";

const NO_BASELINE = /A snapshot doesn't exist at/;
const MISMATCH = /Screenshot comparison failed|Expected an image|Image comparison failed/;

/** Titles of a test from the file's describe blocks down to the spec, with its project. */
function shotName(describes, spec, projectName) {
  return [...describes, spec.title, `[${projectName}]`].join(" › ");
}

function lastResult(test) {
  return test.results.length > 0 ? test.results[test.results.length - 1] : undefined;
}

function classifyTest(test) {
  if (test.status === "skipped") return "skipped";
  if (test.status === "expected" || test.status === "flaky") return "matched";
  const result = lastResult(test);
  const errors = result?.errors ?? [];
  const text = errors.map((e) => `${e.message ?? ""}`).join("\n");
  if (NO_BASELINE.test(text)) return "new";
  const hasDiff = (result?.attachments ?? []).some((a) => /-diff\.png$/.test(a.name ?? a.path ?? ""));
  if (hasDiff || MISMATCH.test(text)) return "differ";
  return "errored";
}

function firstLine(test) {
  const errors = lastResult(test)?.errors ?? [];
  const message = (errors[0]?.message ?? "").replace(/\u001b\[[0-9;]*m/g, "");
  return message.split("\n").find((l) => l.trim() !== "")?.trim() ?? "no error message";
}

function walk(suite, describes, visit) {
  for (const spec of suite.specs ?? []) {
    for (const test of spec.tests ?? []) visit(describes, spec, test);
  }
  for (const child of suite.suites ?? []) walk(child, [...describes, child.title], visit);
}

export function classify(report) {
  const out = { status: "match", matched: 0, skipped: 0, differ: [], new: [], errored: [] };
  for (const file of report.suites ?? []) {
    // The first level is the spec file; its title is a path, not a describe.
    walk(file, [], (describes, spec, test) => {
      const kind = classifyTest(test);
      const name = shotName(describes, spec, test.projectName);
      if (kind === "matched") out.matched += 1;
      else if (kind === "skipped") out.skipped += 1;
      else if (kind === "errored") out.errored.push({ name, detail: firstLine(test) });
      else out[kind].push({ name });
    });
  }
  for (const error of report.errors ?? []) {
    const message = `${error.message ?? ""}`.replace(/\u001b\[[0-9;]*m/g, "");
    out.errored.push({
      name: "the run",
      detail: message.split("\n").find((l) => l.trim() !== "")?.trim() ?? "no error message",
    });
  }
  if (out.differ.length + out.new.length > 0) out.status = "differ";
  else if (out.errored.length > 0) out.status = "error";
  else if (out.matched === 0) out.status = "skipped";
  return out;
}

function counts(c) {
  return {
    status: c.status,
    matched: c.matched,
    differ: c.differ.length,
    new: c.new.length,
    errored: c.errored.length,
  };
}

function list(items, line) {
  return items.map((item) => `- ${line(item)}`).join("\n");
}

export function render(c, { artifactUrl = "", runUrl = "" } = {}) {
  const runLink = runUrl === "" ? "the workflow run" : `[the workflow run](${runUrl})`;
  const artifactLink =
    artifactUrl === ""
      ? `The images are in the report artifact of ${runLink}.`
      : `The expected, actual and diff images and the HTML report are in [the report artifact](${artifactUrl}).`;
  const parts = [MARKER, "### Web visual comparisons"];
  if (c.status === "match") {
    parts.push(`${c.matched === 1 ? "The shot matches its" : `All ${c.matched} shots match their`} baseline${c.matched === 1 ? "" : "s"}. (${runLink})`);
  } else if (c.status === "skipped") {
    parts.push(`No visual comparison ran: every test skipped itself. See ${runLink}.`);
  } else if (c.status === "error" && c.differ.length + c.new.length + c.errored.length === 0) {
    parts.push(`The visual comparisons produced no results${c.reason ? ` (${c.reason})` : ""}. See ${runLink}.`);
  } else {
    if (c.differ.length > 0) {
      const n = c.differ.length;
      parts.push(`**${n} ${n === 1 ? "shot differs" : "shots differ"} from the baseline:**`);
      parts.push(list(c.differ, (s) => `\`${s.name}\``));
    }
    if (c.new.length > 0) {
      const n = c.new.length;
      parts.push(`**${n} ${n === 1 ? "shot has" : "shots have"} no baseline yet:**`);
      parts.push(list(c.new, (s) => `\`${s.name}\``));
    }
    if (c.errored.length > 0) {
      const n = c.errored.length;
      parts.push(`**${n} ${n === 1 ? "shot" : "shots"} failed for another reason and could not be compared:**`);
      parts.push(list(c.errored, (s) => `\`${s.name}\`: ${s.detail}`));
    }
    parts.push(artifactLink);
    parts.push(
      "If the change is intended, re-shoot the baselines in the Playwright container (needs Docker) and commit them: `just web-e2e-update`. To re-shoot some shots only, add a filter: `just web-e2e-update -g \"<part of the shot name>\"`.",
    );
    parts.push("These comparisons are a design reference for a human to look at. They do not block this pull request.");
  }
  return parts.join("\n\n") + "\n";
}

function emit(file, pairs) {
  if (!file) return;
  for (const [key, value] of Object.entries(pairs)) {
    const text = `${value}`;
    if (text.includes("\n")) {
      const delimiter = `EOF_${randomUUID()}`;
      appendFileSync(file, `${key}<<${delimiter}\n${text}\n${delimiter}\n`);
    } else {
      appendFileSync(file, `${key}=${text}\n`);
    }
  }
}

function main(argv) {
  const [command, a, b] = argv;
  if (command === "classify" && a && b) {
    let result;
    try {
      result = classify(JSON.parse(readFileSync(a, "utf8")));
    } catch (error) {
      result = {
        status: "error",
        matched: 0,
        skipped: 0,
        differ: [],
        new: [],
        errored: [],
        reason: `the Playwright report is missing or unreadable: ${error.code ?? error.name}`,
      };
    }
    writeFileSync(b, JSON.stringify(result, null, 2));
    emit(process.env.GITHUB_OUTPUT, counts(result));
    return 0;
  }
  if (command === "render" && a && b) {
    const c = JSON.parse(readFileSync(a, "utf8"));
    const body = render(c, {
      artifactUrl: process.env.ARTIFACT_URL ?? "",
      runUrl: process.env.RUN_URL ?? "",
    });
    writeFileSync(b, body);
    emit(process.env.GITHUB_OUTPUT, { body });
    if (process.env.GITHUB_STEP_SUMMARY) {
      appendFileSync(process.env.GITHUB_STEP_SUMMARY, body.replace(MARKER + "\n\n", ""));
    }
    return 0;
  }
  process.stderr.write(
    "usage: visual-report.mjs classify <playwright-json> <classification-json>\n       visual-report.mjs render <classification-json> <markdown-file>\n",
  );
  return 2;
}

if (import.meta.url === `file://${process.argv[1]}`) {
  process.exitCode = main(process.argv.slice(2));
}
