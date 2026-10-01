// What the Model settings section knows about model roles and providers: each
// role named by the job it does, the providers a role can name, and how a
// role's `provider/model` value splits into the two.

import { PROVIDER_NAMES, PROVIDER_KEYS } from "../components/setup/providers";
import { DEFAULT_EMBEDDING_MODELS, DEFAULT_MODELS, EMBEDDING_PROVIDERS } from "./models";
import type { ModelRoleKey, SettingsModelAssignments, SettingsProviderEntry } from "./types";

/** A role besides the main one, named by its job. */
export interface JobRole {
  key: Exclude<ModelRoleKey, "main">;
  label: string;
  /** What the job is, with `{agent}` for the agent's name. */
  description: string;
}

/**
 * The roles under "Use different models for specific jobs", in the order the
 * section lists them. `default` comes first, since the four after it fall back
 * to it.
 */
export const JOB_ROLES: readonly JobRole[] = [
  {
    key: "default",
    label: "Default for jobs",
    description:
      "The next four jobs use this model when they don't have their own. Background sessions and the search index don't.",
  },
  {
    key: "observer",
    label: "Summarizing older messages",
    description:
      "When a conversation gets long, this model turns older messages into notes so {agent} can keep going.",
  },
  {
    key: "reflector",
    label: "Condensing memories",
    description: "Merges and trims those notes once there are too many.",
  },
  {
    key: "pulse",
    label: "Regular checks",
    description: "Runs the scheduled checks, such as looking through the inbox.",
  },
  {
    key: "subconscious",
    label: "Reviewing replies",
    description:
      "Reads along and nudges {agent} when it drifts from its instructions. It runs only while Review replies is on in Memory.",
  },
  {
    key: "bgSmall",
    label: "Background sessions, small",
    description: "Quick work {agent} starts on its own, such as formatting and lookups.",
  },
  {
    key: "bgMedium",
    label: "Background sessions, medium",
    description: "Summaries and analysis it starts on its own.",
  },
  {
    key: "bgLarge",
    label: "Background sessions, large",
    description: "Research and other work that needs strong reasoning.",
  },
  {
    key: "embedding",
    label: "Search index",
    description:
      "Turns notes into numbers so {agent} can search its memory by meaning. Without one, it searches by words only.",
  },
];

/** What a role uses while it names no model of its own, as its first choice says. */
export function unsetChoiceLabel(role: ModelRoleKey, models: SettingsModelAssignments): string {
  switch (role) {
    case "main":
      return "Choose a provider";
    case "embedding":
      return "None: search by words only";
    case "bgSmall":
      return "Use the medium size";
    case "bgMedium":
      return "Use the large size";
    case "bgLarge":
    case "default":
      return "Use the main model";
    case "observer":
    case "reflector":
    case "pulse":
    case "subconscious":
      return models.default === "" ? "Use the main model" : "Use the default for jobs";
  }
}

/** A role's value split at the first `/`: Fireworks model ids carry slashes of their own. */
export function splitModel(value: string): { provider: string; model: string } {
  const slash = value.indexOf("/");
  return slash < 0
    ? { provider: value, model: "" }
    : { provider: value.slice(0, slash), model: value.slice(slash + 1) };
}

/** A provider a role can name: an entry in the providers list, or a type whose key comes from the environment. */
export interface ProviderOption {
  /** What goes before the `/` in a role's value. */
  name: string;
  /** The provider's type: `anthropic`, `openai`, `gemini`, `fireworks` or `ollama`. */
  type: string;
  label: string;
  /** The entry in the providers list, for its key and address. Null for a type named directly. */
  entry: SettingsProviderEntry | null;
}

/** The environment variable a provider type named directly reads its key from. */
export function envKeyOf(type: string): string {
  return `${type.toUpperCase()}_API_KEY`;
}

export function providerTypeLabel(type: string): string {
  return type in PROVIDER_NAMES ? PROVIDER_NAMES[type as keyof typeof PROVIDER_NAMES] : type;
}

/**
 * The providers a role can name: each entry in the providers list, then each
 * type no entry is named after, whose key comes from its environment variable.
 * The search index can only use the types that make embeddings.
 */
export function providerOptions(
  providers: readonly SettingsProviderEntry[],
  role: ModelRoleKey,
): ProviderOption[] {
  const usable = (type: string): boolean =>
    role !== "embedding" || EMBEDDING_PROVIDERS.includes(type);
  const named = providers
    .filter((entry) => entry.name.trim() !== "" && usable(entry.type))
    .map((entry) => ({
      name: entry.name,
      type: entry.type,
      label: `${entry.name} (${providerTypeLabel(entry.type)})`,
      entry,
    }));
  const direct = PROVIDER_KEYS.filter(
    (type) => usable(type) && !named.some((option) => option.name === type),
  ).map((type) => ({ name: type, type, label: providerTypeLabel(type), entry: null }));
  return [...named, ...direct];
}

/** Where a provider named by its type gets its key, which its label leaves out. */
export function directKeyNote(option: ProviderOption): string | undefined {
  if (option.entry !== null) return undefined;
  return option.type === "ollama"
    ? "Uses Ollama on this computer. Add it under Providers to use another address."
    : `Reads its key from ${envKeyOf(option.type)}. Add it under Providers to use another key.`;
}

/** The model a role starts on when its provider is chosen. */
export function startingModel(role: ModelRoleKey, type: string): string {
  return (role === "embedding" ? DEFAULT_EMBEDDING_MODELS[type] : DEFAULT_MODELS[type]) ?? "";
}

/** The thinking levels a model can be set to; empty leaves it to the default. */
export const THINKING_LEVELS: readonly { value: string; label: string }[] = [
  { value: "", label: "Default" },
  { value: "off", label: "Off" },
  { value: "low", label: "Low" },
  { value: "medium", label: "Medium" },
  { value: "high", label: "High" },
];

/** The thinking choices, with a value the file holds that isn't among them (such as `on`) kept as its own. */
export function thinkingChoices(current: string): { value: string; label: string }[] {
  return THINKING_LEVELS.some((level) => level.value === current)
    ? [...THINKING_LEVELS]
    : [...THINKING_LEVELS, { value: current, label: current }];
}
