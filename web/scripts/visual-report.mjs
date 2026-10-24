#!/usr/bin/env node
// Turns the Playwright JSON report of the web visual comparisons into a status
// and the Markdown of the pull request comment (.github/workflows/web-checks.yml,
// job `web-visual`; .github/workflows/ci.yml, job `visual-report`).
//
//   web/scripts/visual-report.mjs classify <playwright-json> <classification-json>
//   web/scripts/visual-report.mjs render <classification-json> <markdown-file>
//
// `classify` sorts every visual shot into one of four outcomes:
//   match   the screenshot equals its baseline
//   differ  the screenshot differs from its baseline
//   new     the shot has no baseline yet
//   errored the test failed for another reason, such as a timeout, or the shot
//           never held still long enough to be compared
// A test that ran more than once (retries, `--repeat-each`) is one shot; the
// worst outcome among its runs counts. A report that is missing, unreadable or
// lists no tests is status `error`; a run where every test skipped itself is
// status `skipped`.
//
// A shot is named by its baseline, taken from the names of the images
// Playwright attaches (`chat-turn-diff.png` is the shot `chat-turn`), because
// that is the name the baseline file, the docs and the report artifact use. A
// test that fails before it attaches any image falls back to its title.
//
// `classify` appends `status` to $GITHUB_OUTPUT when it is set. `render` reads
// ARTIFACT_URL and RUN_URL from the environment, writes the Markdown to the
// file, and appends it to $GITHUB_OUTPUT as `body` and to $GITHUB_STEP_SUMMARY
// when those are set.

import { appendFileSync, readFileSync, writeFileSync } from "node:fs";
import { randomUUID } from "node:crypto";
import { stripVTControlCharacters } from "node:util";

// The comment carries this marker so the pull request job finds it again and
// updates it in place instead of adding another.
const MARKER = "<!-- residuum-visual-report -->";

// Playwright 1.63 wording; web/scripts/visual-report.test.ts runs these against
// reports shaped like the ones that version writes.
const NO_BASELINE = /A snapshot doesn't exist at/;
const UNSTABLE = /Failed to take two consecutive stable screenshots/;
const SIZE_MISMATCH = /Expected an image/;
const ATTACHMENT = /^(.+)-(?:expected|actual|previous|diff)\.(?:png|jpe?g|webp)$/;
const SNAPSHOT_LINE = /^\s*Snapshot:\s*(.+?)\.(?:png|jpe?g|webp)\s*$/m;

const UNSTABLE_DETAIL =
  "the shot never held still: something in it moves or changes; mask or freeze it";

// A failed screenshot assertion opens with this line; its cause is in the
// indented lines that follow, before the `Snapshot:` and `Call log:` sections.
const SCREENSHOT_FAILED = /^Error: expect\(.+\)\.\w+\(.*\) failed$/;
const DETAIL_END = /^\s*(?:Snapshot:|Call log:)/m;
const LOCATOR = /^Locator: (.+)$/m;

// Worst outcome wins when one shot has several runs.
const RANK = { skipped: 0, matched: 1, errored: 2, new: 3, differ: 4 };

function firstLineOf(text) {
  return (
    stripVTControlCharacters(text)
      .split("\n")
      .find((l) => l.trim() !== "")
      ?.trim() ?? "no error message"
  );
}

/** Why a test failed, in one line: its cause and the locator it was waiting for. */
function reasonOf(text) {
  const head = firstLineOf(text);
  if (!SCREENSHOT_FAILED.test(head)) return head;
  const detail = stripVTControlCharacters(text).split(DETAIL_END)[0] ?? "";
  const cause =
    detail
      .split("\n")
      .find((line) => /^\s+\S/.test(line))
      ?.trim() ?? head;
  const locator = LOCATOR.exec(detail)?.[1];
  return locator === undefined ? cause : `${cause} (Locator: ${locator})`;
}

function lastResult(test) {
  return test.results.length > 0 ? test.results[test.results.length - 1] : undefined;
}

/** The baseline name of the shot a result concerns, or undefined when it attached no image. */
function shotOf(result, text) {
  for (const attachment of result?.attachments ?? []) {
    const match = ATTACHMENT.exec(attachment.name ?? "");
    if (match) return match[1];
  }
  return SNAPSHOT_LINE.exec(text)?.[1];
}

/** The outcome of one test, from its final run. */
function outcomeOf(test) {
  if (test.status === "skipped") return { kind: "skipped" };
  if (test.status === "expected" || test.status === "flaky") return { kind: "matched" };
  const result = lastResult(test);
  const errors = result?.errors ?? [];
  const text = stripVTControlCharacters(errors.map((e) => `${e.message ?? ""}`).join("\n"));
  const shot = shotOf(result, text);
  // Checked before the others: a shot that never settles ends with a diff
  // image (the previous take against the last one), with or without a baseline.
  if (UNSTABLE.test(text)) return { kind: "errored", shot, detail: UNSTABLE_DETAIL };
  if (NO_BASELINE.test(text)) return { kind: "new", shot };
  const hasDiff = (result?.attachments ?? []).some((a) => /-diff\.\w+$/.test(a.name ?? ""));
  if (hasDiff || SIZE_MISMATCH.test(text)) return { kind: "differ", shot };
  return {
    kind: "errored",
    shot,
    detail: errors.length > 0 ? reasonOf(text) : "no error message",
  };
}

function walk(suite, describes, visit) {
  for (const spec of suite.specs ?? []) {
    for (const test of spec.tests ?? []) visit(describes, spec, test);
  }
  for (const child of suite.suites ?? []) walk(child, [...describes, child.title], visit);
}

function classify(report) {
  const shots = new Map();
  let tests = 0;
  for (const file of report.suites ?? []) {
    // The first level is the spec file; its title is a path, not a describe.
    walk(file, [], (describes, spec, test) => {
      tests += 1;
      const outcome = outcomeOf(test);
      const title = [...describes, spec.title].join(" › ");
      const project = test.projectName ?? "";
      // Every repeat of a test is its own spec with the same file and title, so
      // keying on those puts the runs of one test together.
      const key = `${project}\u0000${file.title}\u0000${title}`;
      const known = shots.get(key);
      if (known === undefined || RANK[outcome.kind] > RANK[known.kind]) {
        shots.set(key, { ...outcome, title, project });
      }
    });
  }

  const out = { status: "match", matched: 0, skipped: 0, differ: [], new: [], errored: [] };
  for (const { kind, shot, title, project, detail } of shots.values()) {
    if (kind === "matched") out.matched += 1;
    else if (kind === "skipped") out.skipped += 1;
    else if (kind === "errored") out.errored.push({ shot, title, project, detail });
    else out[kind].push({ shot, title, project });
  }
  for (const error of report.errors ?? []) {
    out.errored.push({
      title: "the run",
      project: "",
      detail: firstLineOf(`${error.message ?? ""}`),
    });
  }

  if (out.differ.length + out.new.length > 0) out.status = "differ";
  else if (out.errored.length > 0) out.status = "error";
  else if (tests === 0) {
    out.status = "error";
    out.reason = "the Playwright report lists no tests";
  } else if (out.matched === 0) out.status = "skipped";
  return out;
}

/** `chat-turn` on `visual-desktop` and the test it comes from, or the title alone when the shot has no name. */
function label({ shot, title, project }) {
  const where = project === "" ? "" : ` (${project})`;
  return shot === undefined || shot === null
    ? `\`${title}\`${where}`
    : `\`${shot}\`${where}: ${title}`;
}

function list(items, detailed) {
  return items
    .map((item) => {
      const line = `- ${label(item)}`;
      return detailed ? `${line}\n  - ${item.detail.replaceAll("`", "'")}` : line;
    })
    .join("\n");
}

function render(c, { artifactUrl = "", runUrl = "" } = {}) {
  const runLink = runUrl === "" ? "the workflow run" : `[the workflow run](${runUrl})`;
  const artifactLink =
    artifactUrl === ""
      ? `The images are in the report artifact of ${runLink}.`
      : `The expected, actual and diff images and the HTML report are in [the report artifact](${artifactUrl}).`;
  const parts = [MARKER, "### Web visual comparisons"];
  if (c.status === "match") {
    parts.push(
      `${c.matched === 1 ? "The shot matches its" : `All ${c.matched} shots match their`} baseline${c.matched === 1 ? "" : "s"}. (${runLink})`,
    );
  } else if (c.status === "skipped") {
    parts.push(`No visual comparison ran: every test skipped itself. See ${runLink}.`);
  } else if (c.status === "error" && c.differ.length + c.new.length + c.errored.length === 0) {
    parts.push(
      `The visual comparisons produced no results${c.reason ? ` (${c.reason})` : ""}. See ${runLink}.`,
    );
  } else {
    if (c.differ.length > 0) {
      const n = c.differ.length;
      parts.push(`**${n} ${n === 1 ? "shot differs" : "shots differ"} from the baseline:**`);
      parts.push(list(c.differ, false));
    }
    if (c.new.length > 0) {
      const n = c.new.length;
      parts.push(`**${n} ${n === 1 ? "shot has" : "shots have"} no baseline yet:**`);
      parts.push(list(c.new, false));
    }
    if (c.errored.length > 0) {
      const n = c.errored.length;
      parts.push(`**${n} ${n === 1 ? "shot" : "shots"} could not be compared:**`);
      parts.push(list(c.errored, true));
    }
    parts.push(artifactLink);
    parts.push(
      'If the change is intended, re-shoot the baselines in the Playwright container (needs Docker) and commit them: `just web-e2e-update`. To re-shoot some shots only, add a filter that matches part of the test title shown above, for example `just web-e2e-update -g "finished turn"`.',
    );
    parts.push(
      "These comparisons are a design reference for a human to look at. They do not block this pull request.",
    );
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
    emit(process.env.GITHUB_OUTPUT, { status: result.status });
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

process.exitCode = main(process.argv.slice(2));
