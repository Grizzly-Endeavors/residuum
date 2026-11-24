import type { AgentConfigFileName } from "./config-coordinator";
import { sectionForConfigKey, type ScopeKind, type SectionId } from "./settings-sections";
import {
  CONFIG_FIELD_MAP,
  MODEL_ROLE_MAP,
  PRICE_RATES,
  configFieldOwner,
  type ConfigFields,
  type PriceRate,
  type WebhookFormEntry,
} from "./settings-toml";
import type { Diagnostic, McpServerEntry, ModelRoleKey, SettingsProviderEntry } from "./types";

export type FieldFile = AgentConfigFileName;

type EntryField<T> = Exclude<keyof T, "name">;

export type FieldRef =
  | { kind: "config"; field: keyof ConfigFields }
  | { kind: "webhook"; name: string; field?: EntryField<WebhookFormEntry> }
  | { kind: "provider"; name: string; field?: EntryField<SettingsProviderEntry> }
  | { kind: "role"; role: ModelRoleKey; field?: "temperature" | "thinking" }
  | { kind: "mcp"; name: string; field?: EntryField<McpServerEntry> }
  | { kind: "price"; name: string; field?: PriceRate };

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
  models: "models",
} as const satisfies Record<EntryField<SettingsProviderEntry>, string>;

const MCP_KEYS = {
  transport: "type",
  command: "command",
  args: "args",
  env: "env",
  url: "url",
  headers: "headers",
} as const satisfies Record<EntryField<McpServerEntry>, string>;

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
  ["pricing", ["pricing"]] as const,
]);

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
    case "price":
      return ["pricing", ref.name, ...(ref.field === undefined ? [] : [ref.field])];
  }
}

export function fieldRefKey(ref: FieldRef): string {
  switch (ref.kind) {
    case "config":
      return `config:${ref.field}`;
    case "role":
      return `role:${ref.role}:${ref.field ?? ""}`;
    case "webhook":
    case "provider":
    case "mcp":
    case "price":
      return `${ref.kind}:${ref.name}:${ref.field ?? ""}`;
  }
}

function segmentsOf(path: string): string[] {
  return path
    .replace(/\[(\w+)\]/g, ".$1")
    .split(".")
    .filter((segment) => segment !== "");
}

function startsWith(path: readonly string[], prefix: readonly string[]): boolean {
  return prefix.length <= path.length && prefix.every((segment, i) => segment === path[i]);
}

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

export function locateField(kind: ScopeKind, file: FieldFile, path: string): FieldRef | null {
  const segments = segmentsOf(path);
  const [head, name, key] = segments;
  if (head === undefined) return null;
  if (kind === "all") {
    if (file !== "config") return null;
    if (head !== "pricing") return locateConfigField(segments, "hub");
    if (name === undefined) return { kind: "config", field: "pricing" };
    return { kind: "price", name, field: PRICE_RATES.find((rate) => rate === key) };
  }
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

export interface PlacedDiagnostic {
  diagnostic: Diagnostic;
  file: FieldFile;
  field: FieldRef | null;
  section: SectionId | null;
}

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
