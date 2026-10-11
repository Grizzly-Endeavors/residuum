import { describe, expect, it } from "vitest";
import {
  fieldRefKey,
  keyPathOf,
  locateField,
  placeDiagnostic,
  type FieldFile,
  type FieldRef,
} from "./settings-fields";
import type { ScopeKind } from "./settings-sections";
import {
  configFieldOwner,
  defaultConfigFields,
  defaultModels,
  diffConfigFields,
  diffMcpServers,
  diffProviders,
  splitConfigPatch,
  type WebhookFormEntry,
} from "./settings-toml";
import type { McpServerEntry, ModelRoleKey, SettingsProviderEntry } from "./types";

/** Where a patch sets its one key: the path of nested single-key objects, down to a value, list or `$inline` table. */
function leafPath(patch: Record<string, unknown>): string[] {
  const path: string[] = [];
  let node: unknown = patch;
  while (
    typeof node === "object" &&
    node !== null &&
    !Array.isArray(node) &&
    !("$inline" in node)
  ) {
    const keys = Object.keys(node);
    expect(keys, `the patch sets one key: ${JSON.stringify(patch)}`).toHaveLength(1);
    const [key] = keys;
    path.push(key ?? "");
    node = (node as Record<string, unknown>)[key ?? ""];
  }
  return path;
}

/** A value that differs from the form's default, of the same kind. */
function changedFrom(value: unknown): unknown {
  if (typeof value === "boolean") return !value;
  if (Array.isArray(value)) return ["x"];
  return "7";
}

describe("every field of every form is in the key-path map", () => {
  const defaults = defaultConfigFields();
  const configFields = Object.keys(defaults) as (keyof typeof defaults)[];

  it("covers every field of ConfigFields, at the key its diff writes", () => {
    for (const field of configFields.filter((f) => f !== "webhooks")) {
      const path = keyPathOf({ kind: "config", field });
      expect(path.length, `${field} has a key path`).toBeGreaterThan(0);

      const diff = diffConfigFields(defaults, {
        ...defaults,
        [field]: changedFrom(defaults[field]),
      });
      expect(leafPath(diff), `${field} saves where the map says`).toEqual([...path]);
    }
    expect(keyPathOf({ kind: "config", field: "webhooks" })).toEqual(["webhooks"]);
  });

  it("covers every field of a webhook", () => {
    const hook: WebhookFormEntry = {
      name: "alerts",
      secret: "secret:a",
      routing: "inbox",
      format: "parsed",
      content_fields: "a, b",
    };
    const fields = Object.keys(hook).filter((f) => f !== "name") as (keyof WebhookFormEntry)[];
    expect(fields).toHaveLength(4);
    for (const field of fields) {
      const base = { ...defaults, webhooks: [hook] };
      const cur = {
        ...defaults,
        webhooks: [{ ...hook, [field]: field === "routing" ? "7" : "new" }],
      };
      const ref: FieldRef = {
        kind: "webhook",
        name: "alerts",
        field: field as Exclude<typeof field, "name">,
      };
      expect(leafPath(diffConfigFields(base, cur))).toEqual([...keyPathOf(ref)]);
    }
  });

  it("covers every field of a provider", () => {
    const provider: SettingsProviderEntry = {
      name: "acme",
      type: "openai",
      apiKey: "secret:k",
      url: "http://a",
      keepAlive: "5m",
    };
    const fields = Object.keys(provider).filter(
      (f) => f !== "name",
    ) as (keyof SettingsProviderEntry)[];
    expect(fields).toHaveLength(4);
    for (const field of fields) {
      const patch = diffProviders(
        [provider],
        [{ ...provider, [field]: "changed" }],
        defaultModels(),
        defaultModels(),
      );
      const ref: FieldRef = {
        kind: "provider",
        name: "acme",
        field: field as Exclude<typeof field, "name">,
      };
      expect(leafPath(patch)).toEqual([...keyPathOf(ref)]);
    }
  });

  it("covers every model role, and its temperature and thinking", () => {
    const roles = Object.keys(defaultModels()).filter(
      (key) => key !== "overrides" && key !== "fallbacks",
    ) as ModelRoleKey[];
    expect(roles).toHaveLength(10);
    for (const role of roles) {
      const patch = diffProviders([], [], defaultModels(), {
        ...defaultModels(),
        [role]: "acme/model",
      });
      expect(leafPath(patch), `${role} saves where the map says`).toEqual([
        ...keyPathOf({ kind: "role", role }),
      ]);
      if (role !== "embedding") {
        expect(keyPathOf({ kind: "role", role, field: "temperature" })).toEqual([
          ...keyPathOf({ kind: "role", role }),
          "temperature",
        ]);
      }
    }
  });

  it("covers every field of an MCP server", () => {
    const server: McpServerEntry = {
      name: "fs",
      transport: "stdio",
      command: "npx",
      args: ["a"],
      env: { A: "1" },
      url: "http://a",
      headers: { H: "1" },
    };
    const changes: Record<string, unknown> = {
      transport: "http",
      command: "other",
      args: ["b"],
      env: { A: "2" },
      url: "http://b",
      headers: { H: "2" },
    };
    const fields = Object.keys(server).filter((f) => f !== "name");
    expect(fields.sort()).toEqual(Object.keys(changes).sort());
    for (const field of fields) {
      const patch = diffMcpServers([server], [{ ...server, [field]: changes[field] }]);
      const ref: FieldRef = { kind: "mcp", name: "fs", field: field as "command" };
      // `env` and `headers` are written key by key, inside the field's table.
      expect(leafPath(patch).slice(0, keyPathOf(ref).length)).toEqual([...keyPathOf(ref)]);
    }
  });
});

describe("placing a key path on a field", () => {
  const place = (kind: ScopeKind, file: FieldFile, path: string): string | null => {
    const ref = locateField(kind, file, path);
    return ref === null ? null : fieldRefKey(ref);
  };

  it("finds every config field from its own key, in the scope that owns it", () => {
    const defaults = defaultConfigFields();
    for (const field of Object.keys(defaults) as (keyof typeof defaults)[]) {
      const ref: FieldRef = { kind: "config", field };
      const path = keyPathOf(ref);
      const kind: ScopeKind = configFieldOwner(path) === "hub" ? "all" : "agent";
      expect(locateField(kind, "config", path.join(".")), `${field} at ${path.join(".")}`).toEqual(
        ref,
      );
    }
  });

  it("finds each entry and its field in a collection", () => {
    expect(locateField("agent", "mcp", "mcpServers.fs")).toEqual({
      kind: "mcp",
      name: "fs",
      field: undefined,
    });
    expect(locateField("agent", "mcp", "mcpServers.fs.command")).toEqual({
      kind: "mcp",
      name: "fs",
      field: "command",
    });
    expect(locateField("agent", "mcp", "mcpServers.fs.transport")).toEqual({
      kind: "mcp",
      name: "fs",
      field: "transport",
    });
    expect(locateField("agent", "providers", "providers.acme.api_key")).toEqual({
      kind: "provider",
      name: "acme",
      field: "apiKey",
    });
    expect(locateField("agent", "config", "webhooks.alerts.content_fields")).toEqual({
      kind: "webhook",
      name: "alerts",
      field: "content_fields",
    });
    // A key the form doesn't model sits on its entry.
    expect(locateField("agent", "mcp", "mcpServers.fs.timeout")).toEqual({
      kind: "mcp",
      name: "fs",
      field: undefined,
    });
  });

  it("finds a model role, and an override inside it", () => {
    expect(place("agent", "providers", "models.main")).toBe("role:main:");
    expect(place("agent", "providers", "models.main.temperature")).toBe("role:main:temperature");
    expect(place("agent", "providers", "background.models.small")).toBe("role:bgSmall:");
    expect(place("agent", "providers", "models.embedding")).toBe("role:embedding:");
    expect(place("agent", "providers", "models")).toBeNull();
  });

  it("finds a list's field from one of its elements, and a field from a table that holds only it", () => {
    expect(place("agent", "config", "skills.dirs[1]")).toBe("config:skills_dirs");
    expect(place("agent", "config", "skills.dirs.1")).toBe("config:skills_dirs");
    expect(place("agent", "config", "skills")).toBe("config:skills_dirs");
  });

  it("gives no field for a table that holds several, or a key no form holds", () => {
    expect(place("agent", "config", "memory")).toBeNull();
    expect(place("agent", "config", "memory.search")).toBeNull();
    expect(place("agent", "config", "unknown.key")).toBeNull();
    expect(place("agent", "config", "")).toBeNull();
  });

  it("keeps the scopes' keys apart", () => {
    expect(place("agent", "config", "gateway.port")).toBeNull();
    expect(place("all", "config", "gateway.port")).toBe("config:gateway_port");
    expect(place("agent", "config", "a2a.port")).toBeNull();
    expect(place("all", "config", "a2a.port")).toBe("config:a2a_port");
    expect(place("agent", "config", "a2a.visibility")).toBe("config:a2a_visibility");
    expect(place("all", "config", "a2a.visibility")).toBeNull();
    expect(place("all", "providers", "models.main")).toBeNull();
    expect(place("all", "mcp", "mcpServers.fs")).toBeNull();
  });
});

describe("placing a diagnostic", () => {
  it("gives a key path its field and section", () => {
    const placed = placeDiagnostic("agent", "providers", {
      severity: "error",
      message: "no such provider",
      location: { kind: "path", path: "models.main" },
    });

    expect(placed.field).toEqual({ kind: "role", role: "main", field: undefined });
    expect(placed.section).toBe("model");
    expect(placed.file).toBe("providers");
  });

  it("gives a line position or no location neither", () => {
    for (const location of [{ kind: "line", line: 3 } as const, undefined]) {
      const placed = placeDiagnostic("agent", "config", {
        severity: "error",
        message: "bad",
        location,
      });
      expect(placed.field).toBeNull();
      expect(placed.section).toBeNull();
    }
  });

  it("gives a key path no field holds its section, from the key's table", () => {
    const placed = placeDiagnostic("agent", "config", {
      severity: "warning",
      message: "unknown",
      location: { kind: "path", path: "memory.bogus" },
    });

    expect(placed.field).toBeNull();
    expect(placed.section).toBe("memory");
  });

  it("splits ownership the way saving does", () => {
    const defaults = defaultConfigFields();
    for (const field of Object.keys(defaults) as (keyof typeof defaults)[]) {
      if (field === "webhooks") continue;
      const path = keyPathOf({ kind: "config", field });
      const diff = diffConfigFields(defaults, {
        ...defaults,
        [field]: changedFrom(defaults[field]),
      });
      const { hub, agent } = splitConfigPatch(diff);
      const owner = configFieldOwner(path);
      expect(
        Object.keys(owner === "hub" ? hub : agent),
        `${field} goes to the ${owner}`,
      ).toHaveLength(1);
      expect(Object.keys(owner === "hub" ? agent : hub)).toHaveLength(0);
    }
  });
});
