// Runs scripts/visual-report.mjs the way CI does, as a subprocess, against
// Playwright JSON reports in fixtures/visual-report/. The reports have the shape
// and wording Playwright 1.63 writes (the JSON reporter in playwright/lib/runner,
// the messages in playwright/lib/matchers/expect.js and playwright-core), so a
// Playwright upgrade that changes either shows up here as a failing test instead
// of a pull request comment that files every changed shot under "errored".

import { spawnSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

interface Shot {
  shot?: string;
  title: string;
  project: string;
  detail?: string;
}

interface Classification {
  status: string;
  matched: number;
  skipped: number;
  differ: Shot[];
  new: Shot[];
  errored: Shot[];
  reason?: string;
}

const SCRIPTS = fileURLToPath(new URL(".", import.meta.url));
const SCRIPT = join(SCRIPTS, "visual-report.mjs");
const fixture = (name: string): string =>
  join(SCRIPTS, "fixtures", "visual-report", `${name}.json`);
const MARKER = "<!-- residuum-visual-report -->";

let dir: string;

beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "visual-report-"));
});

afterEach(() => {
  rmSync(dir, { recursive: true, force: true });
});

interface Run {
  exitCode: number | null;
  stderr: string;
}

function run(script: string, args: string[], env: Record<string, string> = {}): Run {
  const result = spawnSync(process.execPath, [script, ...args], {
    encoding: "utf8",
    env: { PATH: process.env.PATH ?? "", ...env },
  });
  return { exitCode: result.status, stderr: result.stderr };
}

interface Classified {
  classification: Classification;
  githubOutput: string;
}

/** `classify` on a fixture, with $GITHUB_OUTPUT pointed at a file in the temp directory. */
function classify(input: string): Classified {
  const output = join(dir, "classification.json");
  const githubOutput = join(dir, "github-output");
  writeFileSync(githubOutput, "");
  const { exitCode } = run(SCRIPT, ["classify", input, output], { GITHUB_OUTPUT: githubOutput });
  expect(exitCode).toBe(0);
  return {
    classification: JSON.parse(readFileSync(output, "utf8")) as Classification,
    githubOutput: readFileSync(githubOutput, "utf8"),
  };
}

interface Rendered {
  markdown: string;
  githubOutput: string;
  stepSummary: string;
}

/** `render` of the classification of a fixture. */
function renderFixture(name: string, env: Record<string, string> = {}): Rendered {
  const { classification } = classify(fixture(name));
  return render(classification, env);
}

function render(classification: Classification, env: Record<string, string> = {}): Rendered {
  const input = join(dir, "render-input.json");
  const markdownFile = join(dir, "report.md");
  const githubOutput = join(dir, "render-github-output");
  const stepSummary = join(dir, "step-summary");
  writeFileSync(input, JSON.stringify(classification));
  writeFileSync(githubOutput, "");
  writeFileSync(stepSummary, "");
  const { exitCode } = run(SCRIPT, ["render", input, markdownFile], {
    GITHUB_OUTPUT: githubOutput,
    GITHUB_STEP_SUMMARY: stepSummary,
    ...env,
  });
  expect(exitCode).toBe(0);
  return {
    markdown: readFileSync(markdownFile, "utf8"),
    githubOutput: readFileSync(githubOutput, "utf8"),
    stepSummary: readFileSync(stepSummary, "utf8"),
  };
}

const shotsOf = (shots: Shot[]): string[] =>
  shots.map((s) => `${s.shot ?? s.title} (${s.project})`);

describe("classify", () => {
  it("calls a run where every shot equals its baseline a match, and counts the skipped phone test apart", () => {
    const { classification, githubOutput } = classify(fixture("all-match"));
    expect(classification).toMatchObject({
      status: "match",
      matched: 5,
      skipped: 1,
      differ: [],
      new: [],
      errored: [],
    });
    expect(githubOutput).toBe("status=match\n");
  });

  it("names a differing shot by its baseline, for a pixel difference and for a size mismatch", () => {
    const { classification, githubOutput } = classify(fixture("differ"));
    expect(classification.status).toBe("differ");
    expect(classification.differ).toEqual([
      {
        shot: "chat-turn",
        title: "chat › a finished turn's activity line, open to a step's details",
        project: "visual-desktop",
      },
      { shot: "composer", title: "chat › the composer", project: "visual-desktop" },
    ]);
    expect(classification.new).toEqual([]);
    expect(classification.errored).toEqual([]);
    expect(classification.matched).toBe(4);
    expect(githubOutput).toBe("status=differ\n");
  });

  it("calls a shot with no baseline new, and that makes the status differ", () => {
    const { classification } = classify(fixture("new-baseline"));
    expect(classification.status).toBe("differ");
    expect(classification.new).toEqual([
      { shot: "gallery-menu", title: "gallery › the open menu", project: "visual-desktop" },
    ]);
    expect(classification.differ).toEqual([]);
  });

  it("calls a timeout errored, named by its title because no image was attached, and the status error", () => {
    const { classification, githubOutput } = classify(fixture("errored"));
    expect(classification.status).toBe("error");
    expect(classification.errored).toEqual([
      {
        title: "settings › a settings section",
        project: "visual-desktop",
        detail: "Error: Test timeout of 30000ms exceeded.",
      },
    ]);
    expect(classification.differ).toEqual([]);
    expect(githubOutput).toBe("status=error\n");
  });

  it("calls a shot that never held still errored, with or without a baseline, though Playwright attaches a diff", () => {
    const { classification } = classify(fixture("unstable"));
    expect(classification.status).toBe("error");
    expect(classification.differ).toEqual([]);
    expect(classification.new).toEqual([]);
    expect(classification.errored.map((e) => e.shot)).toEqual(["shell-nav", "shell-header"]);
    for (const e of classification.errored) {
      expect(e.detail).toBe(
        "the shot never held still: something in it moves or changes; mask or freeze it",
      );
    }
  });

  it("counts a shot that passed on a retry as a match and judges a failing one by its last run", () => {
    const { classification } = classify(fixture("retries"));
    expect(classification.status).toBe("differ");
    expect(classification.matched).toBe(3);
    expect(classification.errored).toEqual([]);
    expect(classification.differ).toEqual([
      { shot: "message-details", title: "chat › a message's details", project: "visual-phone" },
    ]);
  });

  it("counts the repeats of one shot once", () => {
    const { classification } = classify(fixture("repeat-each"));
    expect(shotsOf(classification.differ)).toEqual(["chat-turn (visual-desktop)"]);
    expect(classification.matched).toBe(3);
    expect(classification.errored).toEqual([]);
  });

  it("reports an error of the run itself", () => {
    const { classification } = classify(fixture("run-error"));
    expect(classification.status).toBe("error");
    expect(classification.errored).toEqual([
      {
        title: "the run",
        project: "",
        detail: "Error: Cannot find module './missing-helper'",
      },
    ]);
  });

  it("calls a report with no tests an error, not a match", () => {
    const { classification } = classify(fixture("no-tests"));
    expect(classification.status).toBe("error");
    expect(classification.reason).toBe("the Playwright report lists no tests");
  });

  it("calls a run where every test skipped itself skipped", () => {
    const { classification, githubOutput } = classify(fixture("all-skipped"));
    expect(classification.status).toBe("skipped");
    expect(classification.matched).toBe(0);
    expect(classification.skipped).toBe(2);
    expect(githubOutput).toBe("status=skipped\n");
  });

  it("sorts a mixed run into every outcome", () => {
    const { classification } = classify(fixture("mixed"));
    expect(classification.status).toBe("differ");
    expect(shotsOf(classification.differ)).toEqual(["chat-turn (visual-desktop)"]);
    expect(shotsOf(classification.new)).toEqual(["gallery-menu (visual-desktop)"]);
    expect(shotsOf(classification.errored)).toEqual([
      "settings › a settings section (visual-desktop)",
      "shell-nav (visual-desktop)",
    ]);
  });

  it("reports a missing report as an error instead of failing", () => {
    const { classification, githubOutput } = classify(join(dir, "missing.json"));
    expect(classification.status).toBe("error");
    expect(classification.reason).toBe("the Playwright report is missing or unreadable: ENOENT");
    expect(githubOutput).toBe("status=error\n");
  });

  it("reports an unparsable report as an error instead of failing", () => {
    const broken = join(dir, "broken.json");
    writeFileSync(broken, "{ not json");
    const { classification } = classify(broken);
    expect(classification.status).toBe("error");
    expect(classification.reason).toBe(
      "the Playwright report is missing or unreadable: SyntaxError",
    );
  });

  it("runs through a symlinked path, which resolves the script's own URL differently from argv", () => {
    const link = join(dir, "linked-scripts");
    symlinkSync(SCRIPTS, link);
    const output = join(dir, "classification.json");
    const githubOutput = join(dir, "github-output");
    writeFileSync(githubOutput, "");
    const { exitCode } = run(
      join(link, "visual-report.mjs"),
      ["classify", fixture("all-match"), output],
      {
        GITHUB_OUTPUT: githubOutput,
      },
    );
    expect(exitCode).toBe(0);
    expect(readFileSync(githubOutput, "utf8")).toBe("status=match\n");
    expect(existsSync(output)).toBe(true);
  });
});

describe("render", () => {
  const links = {
    ARTIFACT_URL: "https://example.test/artifact/1",
    RUN_URL: "https://example.test/runs/2",
  };

  it("says when every shot matches, with the marker first and the run linked", () => {
    const { markdown } = renderFixture("all-match", links);
    expect(markdown).toBe(
      `${MARKER}\n\n### Web visual comparisons\n\nAll 5 shots match their baselines. ([the workflow run](https://example.test/runs/2))\n`,
    );
  });

  it("lists differing shots by baseline name with the project and the test that makes it", () => {
    const { markdown } = renderFixture("differ", links);
    expect(markdown.startsWith(`${MARKER}\n\n### Web visual comparisons\n\n`)).toBe(true);
    expect(markdown).toContain("**2 shots differ from the baseline:**");
    expect(markdown).toContain(
      "- `chat-turn` (visual-desktop): chat › a finished turn's activity line, open to a step's details",
    );
    expect(markdown).toContain("- `composer` (visual-desktop): chat › the composer");
    expect(markdown).toContain("[the report artifact](https://example.test/artifact/1)");
    expect(markdown).toContain('`just web-e2e-update -g "finished turn"`');
    expect(markdown).toContain("They do not block this pull request.");
  });

  it("lists a shot without a baseline apart from the differing ones", () => {
    const { markdown } = renderFixture("new-baseline", links);
    expect(markdown).toContain("**1 shot has no baseline yet:**");
    expect(markdown).toContain("- `gallery-menu` (visual-desktop): gallery › the open menu");
    expect(markdown).not.toContain("differ from the baseline");
  });

  it("shows why a shot could not be compared under its entry", () => {
    const { markdown } = renderFixture("mixed", links);
    expect(markdown).toContain("**2 shots could not be compared:**");
    expect(markdown).toContain(
      "- `settings › a settings section` (visual-desktop)\n  - Error: Test timeout of 30000ms exceeded.",
    );
    expect(markdown).toContain(
      "- `shell-nav` (visual-desktop): shell › the navigation\n  - the shot never held still: something in it moves or changes; mask or freeze it",
    );
  });

  it("lists an error of the run without a project", () => {
    const { markdown } = renderFixture("run-error", links);
    expect(markdown).toContain("- `the run`\n  - Error: Cannot find module './missing-helper'");
  });

  it("says why a report without results has none, linking the run", () => {
    const { markdown } = renderFixture("no-tests", links);
    expect(markdown).toContain(
      "The visual comparisons produced no results (the Playwright report lists no tests). See [the workflow run](https://example.test/runs/2).",
    );
  });

  it("says why a missing report has no results", () => {
    const { classification } = classify(join(dir, "missing.json"));
    const { markdown } = render(classification, links);
    expect(markdown).toContain(
      "The visual comparisons produced no results (the Playwright report is missing or unreadable: ENOENT).",
    );
  });

  it("says when no comparison ran", () => {
    const { markdown } = renderFixture("all-skipped", links);
    expect(markdown).toContain("No visual comparison ran: every test skipped itself.");
  });

  it("names the run and the artifact in words when the environment has no links", () => {
    const { markdown } = renderFixture("differ");
    expect(markdown).toContain("The images are in the report artifact of the workflow run.");
    expect(markdown).not.toContain("](");
  });

  it("singles out a lone matching shot", () => {
    const { markdown } = render({
      status: "match",
      matched: 1,
      skipped: 0,
      differ: [],
      new: [],
      errored: [],
    });
    expect(markdown).toContain("The shot matches its baseline. (the workflow run)");
  });

  it("writes the body to GITHUB_OUTPUT between a unique delimiter and to the step summary without the marker", () => {
    const { markdown, githubOutput, stepSummary } = renderFixture("differ", links);
    const match = /^body<<(EOF_[0-9a-f-]+)\n([\s\S]*)\n\1\n$/.exec(githubOutput);
    expect(match).not.toBeNull();
    expect(match?.[2]).toBe(markdown);
    expect(markdown).not.toContain(match?.[1] ?? "unreachable");
    expect(stepSummary).toBe(markdown.replace(`${MARKER}\n\n`, ""));
    expect(stepSummary).not.toContain(MARKER);
  });
});

describe("command line", () => {
  it("prints the usage and exits 2 without a command, writing nothing", () => {
    const githubOutput = join(dir, "github-output");
    const { exitCode, stderr } = run(SCRIPT, [], { GITHUB_OUTPUT: githubOutput });
    expect(exitCode).toBe(2);
    expect(stderr).toContain("usage: visual-report.mjs classify");
    expect(existsSync(githubOutput)).toBe(false);
  });

  it("prints the usage and exits 2 for a command missing its files", () => {
    const { exitCode, stderr } = run(SCRIPT, ["classify", fixture("all-match")]);
    expect(exitCode).toBe(2);
    expect(stderr).toContain("usage: visual-report.mjs");
  });
});
