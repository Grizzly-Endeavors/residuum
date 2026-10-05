// ── Settings field map ───────────────────────────────────────────────
//
// Every field a settings form edits, and the key it saves to. Diffing a form
// into a patch (`settings-toml.ts`) and placing a diagnostic on a field both
// read this one map, so a field that saves to a key is also the field that
// shows that key's error.
//
// A field is named by a `FieldRef`. Fields of the config files are plain
// (`config`, `role`); the entries of a collection (`provider`, `webhook`,
// `mcp`) are named by the entry's name, with no `field` for the entry as a
// whole.

import type { AgentConfigFileName } from "./config-coordinator";
import { sectionForConfigKey, type ScopeKind, type SectionId } from "./settings-sections";
import {
  CONFIG_FIELD_MAP,
  MODEL_ROLE_MAP,
  configFieldOwner,
  type ConfigFields,
  type WebhookFormEntry,
} from "./settings-toml";
import type { Diagnostic, McpServerEntry, ModelRoleKey, SettingsProviderEntry } from "./types";

/** A config file of one scope. The hub's only one is `config`. */
export type FieldFile = AgentConfigFileName;

/** The fields of a collection's entry. The entry's `name` is its key in the file, not a field.
 * `secretEnvKeys` is excluded too: it's wizard-only bookkeeping, never a field the Settings
 * forms edit or save. */
type EntryField<T> = Exclude<keyof T, "name" | "secretEnvKeys">;

export type FieldRef =
  /** A field of `ConfigFields`, including its lists and the `webhooks` collection as a whole. */
  | { kind: "config"; field: keyof ConfigFields }
  | { kind: "webhook"; name: string; field?: EntryField<WebhookFormEntry> }
  | { kind: "provider"; name: string; field?: EntryField<SettingsProviderEntry> }
  /** A model role. `field` names its temperature or thinking override. */
  | { kind: "role"; role: ModelRoleKey; field?: "temperature" | "thinking" }
  | { kind: "mcp"; name: string; field?: EntryField<McpServerEntry> };

/** Where each field of a collection's entry saves, inside the entry's own table. */
const WEBHOOK_KEYS = {
  secret: "secret",
  routing: "routing",
  format: "format",
  content_fields: "content_fields",
} as const satisfies Record<EntryField<WebhookFormEntry>, string>;

const PROVIDER_KEYS = {
  type: "type",
  apiKey: "api_key",
  url: "url",
  keepAlive: "keep_alive",
} as const satisfies Record<EntryField<SettingsProviderEntry>, string>;

const MCP_KEYS = {
  transport: "type",
  command: "command",
  args: "args",
  env: "env",
  url: "url",
  headers: "headers",
} as const satisfies Record<EntryField<McpServerEntry>, string>;

/** The key of each role, from the model-role map the diff uses, and the embedding model it leaves out. */
const ROLE_PATHS: Readonly<Record<ModelRoleKey, readonly string[]>> = {
  ...(Object.fromEntries(MODEL_ROLE_MAP.map((role) => [role.formKey, role.path])) as Record<
    Exclude<ModelRoleKey, "embedding">,
    readonly string[]
  >),
  embedding: ["models", "embedding"],
};

const CONFIG_PATHS: ReadonlyMap<keyof ConfigFields, readonly string[]> = new Map([
  ...CONFIG_FIELD_MAP.map((spec) => [spec.key, spec.path] as const),
  ["webhooks", ["webhooks"]] as const,
]);

/** The key a field saves to, as path segments: `["models", "main"]` for `models.main`. */
export function keyPathOf(ref: FieldRef): readonly string[] {
  switch (ref.kind) {
    case "config":
      return CONFIG_PATHS.get(ref.field) ?? [];
    case "webhook":
      return ["webhooks", ref.name, ...(ref.field === undefined ? [] : [WEBHOOK_KEYS[ref.field]])];
    case "provider":
      return [
        "providers",
        ref.name,
        ...(ref.field === undefined ? [] : [PROVIDER_KEYS[ref.field]]),
      ];
    case "role":
      return [...ROLE_PATHS[ref.role], ...(ref.field === undefined ? [] : [ref.field])];
    case "mcp":
      return ["mcpServers", ref.name, ...(ref.field === undefined ? [] : [MCP_KEYS[ref.field]])];
  }
}

/** A string that is the same for the same field. */
export function fieldRefKey(ref: FieldRef): string {
  switch (ref.kind) {
    case "config":
      return `config:${ref.field}`;
    case "role":
      return `role:${ref.role}:${ref.field ?? ""}`;
    case "webhook":
    case "provider":
    case "mcp":
      return `${ref.kind}:${ref.name}:${ref.field ?? ""}`;
  }
}

// ── Placing a key path ───────────────────────────────────────────────

/** A key path in a diagnostic (`mcpServers.fs`, `skills.dirs[0]`) as segments. */
function segmentsOf(path: string): string[] {
  return path
    .replace(/\[(\w+)\]/g, ".$1")
    .split(".")
    .filter((segment) => segment !== "");
}

function startsWith(path: readonly string[], prefix: readonly string[]): boolean {
  return prefix.length <= path.length && prefix.every((segment, i) => segment === path[i]);
}

/** The field whose key `segments` is, or lies inside. A table that holds several fields has none. */
function locateConfigField(segments: readonly string[], owner: "hub" | "agent"): FieldRef | null {
  const specs = CONFIG_FIELD_MAP.filter((spec) => configFieldOwner(spec.path) === owner);
  const inside = specs
    .filter((spec) => startsWith(segments, spec.path))
    .sort((a, b) => b.path.length - a.path.length)[0];
  if (inside) return { kind: "config", field: inside.key };
  const holding = specs.filter((spec) => startsWith(spec.path, segments));
  const [only, ...others] = holding;
  return only && others.length === 0 ? { kind: "config", field: only.key } : null;
}

function entryField<K extends string>(
  keys: Readonly<Record<K, string>>,
  key: string | undefined,
): K | undefined {
  if (key === undefined) return undefined;
  return (Object.keys(keys) as K[]).find((field) => keys[field] === key);
}

/** The field a key path names in `file` of a scope of `kind`, or null when no form field holds that key. */
export function locateField(kind: ScopeKind, file: FieldFile, path: string): FieldRef | null {
  const segments = segmentsOf(path);
  const [head, name, key] = segments;
  if (head === undefined) return null;
  if (kind === "all") return file === "config" ? locateConfigField(segments, "hub") : null;
  switch (file) {
    case "mcp":
      return head === "mcpServers" && name !== undefined
        ? { kind: "mcp", name, field: entryField(MCP_KEYS, key === "transport" ? "type" : key) }
        : null;
    case "providers": {
      if (head === "providers" && name !== undefined) {
        return { kind: "provider", name, field: entryField(PROVIDER_KEYS, key) };
      }
      const role = Object.entries(ROLE_PATHS).find(([, rolePath]) =>
        startsWith(segments, rolePath),
      );
      if (!role) return null;
      const override = segments[role[1].length];
      return {
        kind: "role",
        role: role[0] as ModelRoleKey,
        field: override === "temperature" || override === "thinking" ? override : undefined,
      };
    }
    case "config":
      if (head !== "webhooks") return locateConfigField(segments, "agent");
      return name === undefined
        ? { kind: "config", field: "webhooks" }
        : { kind: "webhook", name, field: entryField(WEBHOOK_KEYS, key) };
  }
}

/** A diagnostic from a save, with the form field and section its key path leads to. */
export interface PlacedDiagnostic {
  diagnostic: Diagnostic;
  file: FieldFile;
  /** The field the key path names. Null for a diagnostic with no key path, or one no form field holds. */
  field: FieldRef | null;
  /** The section whose form edits the key. Null when the diagnostic has no key path, which leaves it for whichever section is open. */
  section: SectionId | null;
}

/** Place a diagnostic from a save of `file` in a scope of `kind`. */
export function placeDiagnostic(
  kind: ScopeKind,
  file: FieldFile,
  diagnostic: Diagnostic,
): PlacedDiagnostic {
  const path = diagnostic.location?.kind === "path" ? diagnostic.location.path : null;
  return {
    diagnostic,
    file,
    field: path === null ? null : locateField(kind, file, path),
    section: path === null ? null : sectionForConfigKey(kind, file, path),
  };
}
