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

export function failureLine(kind: AgentErrorKind | undefined): string {
  return FAILURE_LINES[kind ?? "other"];
}

export interface SettingsFix {
  section: SectionId;
  field: { ref: FieldRef; file: FieldFile; problems: Diagnostic[] } | null;
}

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
    invalidate(cacheKey);
    let result;
    try {
      result = await validate(await read());
    } catch {
      continue;
    }
    const problems = result.diagnostics ?? [];
    if (problems.length === 0) continue;
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
