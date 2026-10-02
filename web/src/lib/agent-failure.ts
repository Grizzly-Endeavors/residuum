// What to say about an agent that couldn't start, and where its fix is.
// Home's needs-you item and the agent's state card read the same words.

import {
  cacheKeyConfigRaw,
  cacheKeyProvidersRaw,
  fetchConfigRaw,
  fetchProvidersRaw,
  validateConfig,
  validateProviders,
} from "./api";
import { invalidate } from "./cache";
import type { AgentErrorKind } from "./hub-types";
import type * as SettingsFields from "./settings-fields";
import type { FieldFile, FieldRef } from "./settings-fields";
import { RAW_SECTION, type SectionId } from "./settings-sections";
import type { Diagnostic } from "./types";

const FAILURE_LINES: Readonly<Record<AgentErrorKind, string>> = {
  config: "Something in its settings needs fixing before it can run.",
  port_conflict: "Another program is using a port its Teams connection needs.",
  crash: "It stopped unexpectedly while it was running.",
  other: "Something went wrong while it was starting.",
};

/** One plain-language line about why an agent couldn't start, chosen by the kind of failure. */
export function failureLine(kind: AgentErrorKind | undefined): string {
  return FAILURE_LINES[kind ?? "other"];
}

/** Where an agent's failing setting is fixed. */
export interface SettingsFix {
  section: SectionId;
  /** The field to flag, and the file whose problems flag it. Null when no problem names a field, which leaves Raw config to show them. */
  field: { ref: FieldRef; file: FieldFile; problems: Diagnostic[] } | null;
}

/**
 * Where an agent whose settings stop it starting is fixed: the section and
 * field of the first problem the validate endpoints report on a setting a
 * form holds, checking `providers.toml` and then `config.toml`. Raw config,
 * where every problem shows, when none does or the files can't be read.
 */
export async function findSettingsFix(agent: string): Promise<SettingsFix> {
  const files = [
    {
      file: "providers",
      read: () => fetchProvidersRaw(agent),
      validate: (text: string) => validateProviders(agent, text),
      cacheKey: cacheKeyProvidersRaw(agent),
    },
    {
      file: "config",
      read: () => fetchConfigRaw(agent),
      validate: (text: string) => validateConfig(agent, text),
      cacheKey: cacheKeyConfigRaw(agent),
    },
  ] as const;
  for (const { file, read, validate, cacheKey } of files) {
    // The file may have changed since it was last read; the fix has to see it as it is.
    invalidate(cacheKey);
    let result;
    try {
      result = await validate(await read());
    } catch {
      // Raw config shows the file and its problems, whatever stopped this check.
      continue;
    }
    const problems = result.diagnostics ?? [];
    if (problems.length === 0) continue;
    // Placing a problem reads the settings forms' fields, which load with Settings.
    let placeDiagnostic: typeof SettingsFields.placeDiagnostic;
    try {
      ({ placeDiagnostic } = await import("./settings-fields"));
    } catch {
      break;
    }
    for (const diagnostic of problems) {
      const placed = placeDiagnostic("agent", file, diagnostic);
      if (placed.field !== null && placed.section !== null) {
        return { section: placed.section, field: { ref: placed.field, file, problems } };
      }
    }
  }
  return { section: RAW_SECTION, field: null };
}
