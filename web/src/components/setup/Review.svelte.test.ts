import { describe, expect, it } from "vitest";
import { fireEvent, jsonResponse, mockFetch, render, screen } from "../../test/component";
import { waitFor } from "../../test/wait";
import Review from "./Review.svelte";
import type { ProviderKey, SetupWizardState } from "../../lib/types";

// Reactive, as the wizard's own state is, so the step redraws after a change.
function wizard(overrides: Partial<SetupWizardState> = {}): SetupWizardState {
  const state = $state({
    userName: "",
    agentName: "assistant",
    timezone: "Europe/Oslo",
    selectedProviders: ["anthropic"] as ProviderKey[],
    providerConfigs: {
      anthropic: { apiKey: "sk-ant", model: "", url: "" },
      openai: { apiKey: "", model: "", url: "" },
      gemini: { apiKey: "", model: "", url: "" },
      fireworks: { apiKey: "", model: "", url: "" },
      ollama: { apiKey: "", model: "", url: "" },
    },
    mainProvider: "anthropic",
    roles: {
      observer: { provider: "", url: "", model: "" },
      reflector: { provider: "", url: "", model: "" },
      pulse: { provider: "", url: "", model: "" },
    },
    embeddingModel: { provider: "", model: "" },
    backgroundModels: {
      small: { provider: "", model: "" },
      medium: { provider: "", model: "" },
      large: { provider: "", model: "" },
    },
    mcpServers: [],
    integrations: {
      discordToken: "",
      telegramToken: "",
      teamsAppId: "",
      teamsTenantId: "",
      teamsAppPassword: "",
    },
    secretRefs: {},
    ...overrides,
  } as SetupWizardState);
  return state;
}

/** Answers the secret store and setup completion, recording what was sent. */
function recordSave(): { secrets: { name: string; value: string }[]; mcpJson: () => string } {
  const secrets: { name: string; value: string }[] = [];
  let completed: { mcp_json?: string } | null = null;
  mockFetch((url, init) => {
    const body = JSON.parse(typeof init?.body === "string" ? init.body : "null") as unknown;
    if (url.endsWith("/secrets")) {
      const secret = body as { name: string; value: string };
      secrets.push(secret);
      return jsonResponse({ reference: `secret:${secret.name}` });
    }
    if (url.endsWith("/config/complete-setup")) {
      completed = body as { mcp_json?: string };
      return jsonResponse({ valid: true, diagnostics: [] });
    }
    throw new Error(`unexpected fetch ${url}`);
  });
  return {
    secrets,
    mcpJson: () => {
      if (completed === null) throw new Error("setup not completed yet");
      return completed.mcp_json ?? "";
    },
  };
}

function renderStep(state: SetupWizardState): void {
  render(Review, { wizardState: state, onBack: () => {}, onComplete: () => {} });
}

describe("Setup save step", () => {
  it("stores a tool server's key as a secret and writes only its reference to mcp.json", async () => {
    const save = recordSave();
    renderStep(
      wizard({
        mcpServers: [
          {
            name: "tavily",
            command: "npx",
            args: ["tavily"],
            env: { TAVILY_API_KEY: "tvly-plain", REGION: "eu" },
            secretEnvKeys: ["TAVILY_API_KEY"],
          },
        ],
      }),
    );

    await fireEvent.click(screen.getByRole("button", { name: "Save and start" }));
    const mcpJson = await waitFor(() => save.mcpJson());

    expect(save.secrets).toContainEqual({ name: "mcp_tavily_tavily_api_key", value: "tvly-plain" });
    expect(mcpJson).not.toContain("tvly-plain");
    expect(JSON.parse(mcpJson)).toEqual({
      mcpServers: {
        tavily: {
          command: "npx",
          args: ["tavily"],
          env: { TAVILY_API_KEY: "secret:mcp_tavily_tavily_api_key", REGION: "eu" },
        },
      },
    });
  });

  it("writes nothing when a tool server key can't be stored", async () => {
    const requested: string[] = [];
    mockFetch((url, init) => {
      requested.push(url);
      const body = JSON.parse(typeof init?.body === "string" ? init.body : "null") as {
        name: string;
      } | null;
      if (url.endsWith("/secrets") && body?.name === "mcp_tavily_tavily_api_key") {
        return jsonResponse({ error: "the secret store is unreadable" }, 500);
      }
      if (url.endsWith("/secrets")) return jsonResponse({ reference: `secret:${body?.name}` });
      throw new Error(`unexpected fetch ${url}`);
    });
    renderStep(
      wizard({
        mcpServers: [
          {
            name: "tavily",
            command: "npx",
            args: ["tavily"],
            env: { TAVILY_API_KEY: "tvly-plain" },
            secretEnvKeys: ["TAVILY_API_KEY"],
          },
        ],
      }),
    );

    await fireEvent.click(screen.getByRole("button", { name: "Save and start" }));

    await waitFor(() =>
      expect(screen.getByRole("alert")).toHaveTextContent("Couldn't store your keys and tokens."),
    );
    expect(requested.some((url) => url.endsWith("/config/complete-setup"))).toBe(false);
    expect(screen.getByRole("button", { name: "Save and start" })).toBeEnabled();
  });

  it("says nothing about missing keys when every key is entered", () => {
    renderStep(wizard());

    expect(screen.queryByText(/saved without one/)).toBeNull();
  });

  it("lists a provider saved without a key, and why that can still work", () => {
    renderStep(
      wizard({
        providerConfigs: {
          anthropic: { apiKey: "", model: "", url: "" },
          openai: { apiKey: "", model: "", url: "" },
          gemini: { apiKey: "", model: "", url: "" },
          fireworks: { apiKey: "", model: "", url: "" },
          ollama: { apiKey: "", model: "", url: "" },
        },
      }),
    );

    expect(screen.getByRole("status")).toHaveTextContent(
      "Anthropic has no key and will be saved without one. A provider without a key uses its environment variable if one is set. Go back to enter it now.",
    );
  });
});
