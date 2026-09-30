import { readFileSync } from "node:fs";
import { resolve } from "node:path";

/** The web app directory, where `mock/` lives. */
export const WEB_ROOT = resolve(import.meta.dirname, "..");

/** An example config from the repository's `assets/`, or a placeholder comment when it can't be read. */
export function loadAsset(filename: string): string {
  try {
    return readFileSync(resolve(WEB_ROOT, "..", "assets", filename), "utf-8");
  } catch {
    return `# Could not load ${filename}`;
  }
}
