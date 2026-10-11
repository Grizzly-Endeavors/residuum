import { parse as parseToml, stringify as stringifyToml } from "smol-toml";
import type { Diagnostic } from "../src/lib/types";

/** The models each provider type offers, as the mock's model listing answers. */
export const PROVIDER_MODELS: Readonly<Record<string, readonly { id: string; name: string }[]>> = {
  anthropic: [
    { id: "claude-opus-4-6", name: "Claude Opus 4.6" },
    { id: "claude-sonnet-4-6", name: "Claude Sonnet 4.6" },
    { id: "claude-haiku-4-5", name: "Claude Haiku 4.5" },
  ],
  openai: [
    { id: "gpt-4o", name: "GPT-4o" },
    { id: "gpt-4o-mini", name: "GPT-4o Mini" },
    { id: "o3", name: "o3" },
    { id: "o4-mini", name: "o4-mini" },
  ],
  gemini: [
    { id: "gemini-2.5-pro", name: "Gemini 2.5 Pro" },
    { id: "gemini-2.5-flash", name: "Gemini 2.5 Flash" },
    { id: "gemini-3.0-flash", name: "Gemini 3.0 Flash" },
  ],
  fireworks: [
    { id: "accounts/fireworks/models/glm-5p3", name: "accounts/fireworks/models/glm-5p3" },
    { id: "accounts/fireworks/models/kimi-k3", name: "accounts/fireworks/models/kimi-k3" },
    {
      id: "accounts/fireworks/routers/glm-flash-latest",
      name: "accounts/fireworks/routers/glm-flash-latest",
    },
  ],
  ollama: [
    { id: "llama3.3:latest", name: "Llama 3.3" },
    { id: "mistral:latest", name: "Mistral" },
    { id: "deepseek-r1:latest", name: "DeepSeek R1" },
    { id: "qwen3:latest", name: "Qwen 3" },
  ],
};

/** The model roles of `providers.toml` the mock checks. The search index model isn't in the lists. */
const ROLE_PATHS: readonly (readonly string[])[] = [
  ["models", "main"],
  ["models", "default"],
  ["models", "observer"],
  ["models", "reflector"],
  ["models", "pulse"],
  ["models", "subconscious"],
  ["background", "models", "small"],
  ["background", "models", "medium"],
  ["background", "models", "large"],
];

type Table = Record<string, unknown>;

function isTable(value: unknown): value is Table {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function tableAt(doc: Table, path: readonly string[]): Table | null {
  let at: unknown = doc;
  for (const key of path) at = isTable(at) ? at[key] : undefined;
  return isTable(at) ? at : null;
}

/** A model a role names that its provider doesn't offer, and how to put an offered one in its place. */
interface Unoffered {
  path: string;
  message: string;
  replace: () => void;
}

/**
 * Each model a role names, as `provider/model`, that the mock's list for its
 * provider's type leaves out. A provider is a `[providers]` entry or a type
 * named directly; one the mock has no list for is never checked.
 */
function unofferedModels(doc: Table): Unoffered[] {
  const providers = tableAt(doc, ["providers"]) ?? {};
  const found: Unoffered[] = [];
  for (const path of ROLE_PATHS) {
    const holder = tableAt(doc, path.slice(0, -1));
    const key = path[path.length - 1] ?? "";
    const value = holder?.[key];
    // A role is a model, a failover list, or an inline table whose `model` is either.
    const owner = isTable(value) ? value : holder;
    const ownKey = isTable(value) ? "model" : key;
    const named = owner?.[ownKey];
    const chain = Array.isArray(named) ? named : [named];
    chain.forEach((spec, index) => {
      if (typeof spec !== "string" || owner === null) return;
      const slash = spec.indexOf("/");
      if (slash < 0) return;
      const provider = spec.slice(0, slash);
      const model = spec.slice(slash + 1);
      const entry = providers[provider];
      const type = isTable(entry) && typeof entry.type === "string" ? entry.type : provider;
      const offered = PROVIDER_MODELS[type];
      const first = offered?.[0];
      if (first === undefined || offered?.some((candidate) => candidate.id === model)) return;
      found.push({
        path: `${path.join(".")}${Array.isArray(named) ? `[${String(index)}]` : ""}`,
        message: `model '${model}' is not offered by provider '${provider}'`,
        replace: () => {
          const fixed = `${provider}/${first.id}`;
          if (Array.isArray(named)) named[index] = fixed;
          else owner[ownKey] = fixed;
        },
      });
    });
  }
  return found;
}

function parsed(text: string): Table | null {
  try {
    return parseToml(text);
  } catch {
    return null;
  }
}

/**
 * The models a `providers.toml` assigns that their provider doesn't offer, as
 * problems on the role's key path (`models.main`). The mock's stand-in for a
 * check the backend makes when the agent starts; text that doesn't parse has
 * none, since its syntax error is the problem.
 */
export function modelProblems(providersToml: string): Diagnostic[] {
  const doc = parsed(providersToml);
  if (doc === null) return [];
  return unofferedModels(doc).map(({ path, message }) => ({
    severity: "error",
    message,
    location: { kind: "path", path },
  }));
}

/** `providersToml` with every model its provider doesn't offer replaced by the provider's first one. */
export function offeredModelsOnly(providersToml: string): string {
  const doc = parsed(providersToml);
  if (doc === null) return providersToml;
  const unoffered = unofferedModels(doc);
  if (unoffered.length === 0) return providersToml;
  for (const problem of unoffered) problem.replace();
  return stringifyToml(doc);
}
