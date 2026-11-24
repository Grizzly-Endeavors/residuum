import { readFileSync } from "node:fs";
import { resolve } from "node:path";

export const WEB_ROOT = resolve(import.meta.dirname, "..");

export function loadAsset(filename: string): string {
  try {
    return readFileSync(resolve(WEB_ROOT, "..", "assets", filename), "utf-8");
  } catch {
    return `# Could not load ${filename}`;
  }
}
