// The Playwright configuration that capture.mjs runs the fixture scenarios with.
// One project and one worker: the reports need the names, messages and
// attachments of the shots, not the pixels of CI's container.

import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig } from "@playwright/test";

const HERE = dirname(fileURLToPath(import.meta.url));
// capture.mjs sets these for every run; none of them has a default.
const SCRATCH = requiredEnv("CAPTURE_SCRATCH");
const SNAPSHOTS = requiredEnv("CAPTURE_SNAPSHOTS");

function requiredEnv(name) {
  const value = process.env[name] ?? "";
  if (value === "") throw new Error(`${name} must be set; run capture.mjs`);
  return value;
}

export default defineConfig({
  testDir: join(HERE, process.env.CAPTURE_TESTS ?? "tests"),
  workers: 1,
  retries: Number(process.env.CAPTURE_RETRIES ?? 0),
  expect: { timeout: 2000 },
  outputDir: join(SCRATCH, "results"),
  snapshotPathTemplate: join(SNAPSHOTS, "{arg}-{projectName}-{platform}{ext}"),
  reporter: [["json", { outputFile: join(SCRATCH, "report.json") }]],
  projects: [
    {
      name: "visual-desktop",
      use: { browserName: "chromium", viewport: { width: 400, height: 300 } },
    },
  ],
});
