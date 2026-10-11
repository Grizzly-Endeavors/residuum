import { describe, expect, it } from "vitest";
import type { ProviderKey, SetupWizardState } from "../../lib/types";
import { setProviderSelected } from "./providers";

function wizard(selected: ProviderKey[], main: ProviderKey): SetupWizardState {
  return {
    selectedProviders: selected,
    mainProvider: main,
    roles: {
      observer: { provider: "openai", url: "", model: "gpt-x" },
      reflector: { provider: "", url: "", model: "claude-sonnet-4-6" },
      pulse: { provider: "anthropic", url: "", model: "claude-haiku" },
    },
    backgroundModels: {
      small: { provider: "openai", model: "gpt-mini" },
      medium: { provider: "", model: "claude-sonnet-4-6" },
      large: { provider: "anthropic", model: "claude-opus" },
    },
    embeddingModel: { provider: "openai", model: "text-embedding-3-small" },
  } as unknown as SetupWizardState;
}

describe("setProviderSelected", () => {
  it("adds a provider once", () => {
    const state = wizard(["anthropic"], "anthropic");
    setProviderSelected(state, "openai", true);
    setProviderSelected(state, "openai", true);
    expect(state.selectedProviders).toEqual(["anthropic", "openai"]);
  });

  it("keeps the last selected provider on", () => {
    const state = wizard(["anthropic"], "anthropic");
    setProviderSelected(state, "anthropic", false);
    expect(state.selectedProviders).toEqual(["anthropic"]);
  });

  it("clears every model choice that used a provider when it's turned off", () => {
    const state = wizard(["anthropic", "openai"], "anthropic");
    setProviderSelected(state, "openai", false);

    expect(state.selectedProviders).toEqual(["anthropic"]);
    expect(state.mainProvider).toBe("anthropic");
    expect(state.roles.observer).toEqual({ provider: "", url: "", model: "" });
    expect(state.backgroundModels.small).toEqual({ provider: "", model: "" });
    expect(state.embeddingModel).toEqual({ provider: "", model: "" });
    // Choices on the providers that stay are kept.
    expect(state.roles.reflector?.model).toBe("claude-sonnet-4-6");
    expect(state.roles.pulse?.model).toBe("claude-haiku");
    expect(state.backgroundModels.large?.model).toBe("claude-opus");
  });

  it("moves the main role on, and clears the roles that followed it, when the main provider goes", () => {
    const state = wizard(["anthropic", "openai"], "anthropic");
    setProviderSelected(state, "anthropic", false);

    expect(state.mainProvider).toBe("openai");
    // A role with no provider of its own used the main one.
    expect(state.roles.reflector).toEqual({ provider: "", url: "", model: "" });
    expect(state.backgroundModels.medium).toEqual({ provider: "", model: "" });
    expect(state.roles.pulse).toEqual({ provider: "", url: "", model: "" });
    expect(state.roles.observer?.model).toBe("gpt-x");
  });
});
