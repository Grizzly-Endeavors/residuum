import { describe, expect, it } from "vitest";
import {
  directKeyNote,
  JOB_ROLES,
  providerOptions,
  splitModel,
  startingModel,
  thinkingChoices,
  unsetChoiceLabel,
} from "./model-roles";
import { MODEL_ROLE_MAP, defaultModels } from "./settings-toml";
import type { SettingsProviderEntry } from "./types";

const entry = (name: string, type: string): SettingsProviderEntry => ({
  name,
  type,
  apiKey: "",
  url: "",
  keepAlive: "",
});

describe("model roles", () => {
  it("lists every role but the main one as a job, once", () => {
    const keys = JOB_ROLES.map((job) => job.key);
    expect(new Set(keys).size).toBe(keys.length);
    expect([...keys, "main"].sort()).toEqual(
      [...MODEL_ROLE_MAP.map((role) => role.formKey), "embedding"].sort(),
    );
  });

  it("splits a value at its first slash, so a model id keeps its own", () => {
    expect(splitModel("fireworks/accounts/fireworks/models/kimi-k3")).toEqual({
      provider: "fireworks",
      model: "accounts/fireworks/models/kimi-k3",
    });
    expect(splitModel("anthropic")).toEqual({ provider: "anthropic", model: "" });
  });

  it("says what a role uses while it names no model, by where it falls back", () => {
    const models = defaultModels();
    expect(unsetChoiceLabel("observer", models)).toBe("Use the main model");
    expect(unsetChoiceLabel("bgSmall", models)).toBe("Use the medium size");
    expect(unsetChoiceLabel("bgLarge", models)).toBe("Use the main model");
    expect(unsetChoiceLabel("embedding", models)).toBe("None: search by words only");
    models.default = "anthropic/claude-haiku-4-5";
    expect(unsetChoiceLabel("pulse", models)).toBe("Use the default for jobs");
    expect(unsetChoiceLabel("bgMedium", models)).toBe("Use the large size");
  });

  it("starts a role on its provider's usual model, or its embedding model for the index", () => {
    expect(startingModel("observer", "openai")).toBe("gpt-4o");
    expect(startingModel("embedding", "openai")).toBe("text-embedding-3-small");
    expect(startingModel("main", "elsewhere")).toBe("");
  });

  it("keeps a thinking value the levels don't have as its own choice", () => {
    expect(thinkingChoices("low").map((choice) => choice.value)).toEqual([
      "",
      "off",
      "low",
      "medium",
      "high",
    ]);
    expect(thinkingChoices("on").at(-1)).toEqual({ value: "on", label: "on" });
  });
});

describe("providerOptions", () => {
  it("offers the providers list first, then each type no entry is named after", () => {
    const options = providerOptions(
      [entry("work", "openai"), entry("anthropic", "anthropic")],
      "main",
    );
    expect(options.map((option) => [option.name, option.label])).toEqual([
      ["work", "work (OpenAI)"],
      ["anthropic", "anthropic (Anthropic)"],
      ["openai", "OpenAI"],
      ["gemini", "Google Gemini"],
      ["fireworks", "Fireworks AI"],
      ["ollama", "Ollama"],
    ]);
  });

  it("offers the search index only the types that make embeddings, and skips unnamed entries", () => {
    const options = providerOptions(
      [entry("", "openai"), entry("claude", "anthropic")],
      "embedding",
    );
    expect(options.map((option) => option.name)).toEqual([
      "openai",
      "gemini",
      "fireworks",
      "ollama",
    ]);
  });

  it("says where a type named directly gets its key", () => {
    const [named, ...direct] = providerOptions([entry("work", "openai")], "main");
    expect(named && directKeyNote(named)).toBeUndefined();
    expect(direct.map((option) => directKeyNote(option))).toEqual([
      "Reads its key from ANTHROPIC_API_KEY. Add it under Providers to use another key.",
      "Reads its key from OPENAI_API_KEY. Add it under Providers to use another key.",
      "Reads its key from GEMINI_API_KEY. Add it under Providers to use another key.",
      "Reads its key from FIREWORKS_API_KEY. Add it under Providers to use another key.",
      "Uses Ollama on this computer. Add it under Providers to use another address.",
    ]);
  });
});
