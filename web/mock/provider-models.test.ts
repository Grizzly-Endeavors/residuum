import { parse as parseToml } from "smol-toml";
import { describe, expect, it } from "vitest";
import { MOCK_BRITTLE_PROVIDERS } from "./constants";
import { modelProblems, offeredModelsOnly } from "./provider-models";

describe("models a provider doesn't offer", () => {
  it("are problems on the role's key path, for a provider named by type or by entry", () => {
    const text = [
      "[providers.work]",
      'type = "openai"',
      "[models]",
      'main = "openai/gpt-9"',
      'observer = "work/gpt-4o-mini"',
      'pulse = { model = "work/gpt-10", temperature = 0.2 }',
      "[background.models]",
      'small = ["anthropic/claude-haiku-4-5", "anthropic/claude-0"]',
    ].join("\n");

    expect(modelProblems(text)).toEqual([
      {
        severity: "error",
        message: "model 'gpt-9' is not offered by provider 'openai'",
        location: { kind: "path", path: "models.main" },
      },
      {
        severity: "error",
        message: "model 'gpt-10' is not offered by provider 'work'",
        location: { kind: "path", path: "models.pulse" },
      },
      {
        severity: "error",
        message: "model 'claude-0' is not offered by provider 'anthropic'",
        location: { kind: "path", path: "background.models.small[1]" },
      },
    ]);
  });

  it("leave out providers the mock has no list for, and text that doesn't parse", () => {
    expect(modelProblems('[models]\nmain = "elsewhere/anything"\n')).toEqual([]);
    expect(modelProblems("[models\n")).toEqual([]);
  });

  it("are replaced by the provider's first model, keeping the rest of the role", () => {
    const fixed = parseToml(
      offeredModelsOnly(
        '[models]\nmain = "openai/gpt-9"\npulse = { model = ["openai/o3", "openai/x"], thinking = "low" }\n',
      ),
    );
    expect(fixed).toEqual({
      models: {
        main: "openai/gpt-4o",
        pulse: { model: ["openai/o3", "openai/gpt-4o"], thinking: "low" },
      },
    });
    expect(modelProblems(offeredModelsOnly(MOCK_BRITTLE_PROVIDERS))).toEqual([]);
  });
});
