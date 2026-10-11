import { afterEach, describe, expect, it } from "vitest";
import { onViewedAgentChange, setViewedAgent } from "./viewed-agent";

afterEach(() => {
  setViewedAgent(null);
});

describe("viewed agent", () => {
  it("tells listeners when it changes, and not when it stays", () => {
    const seen: (string | null)[] = [];
    const stop = onViewedAgentChange((name) => {
      seen.push(name);
    });
    setViewedAgent("scout");
    setViewedAgent("scout");
    setViewedAgent("atlas");
    setViewedAgent(null);
    stop();
    setViewedAgent("scout");
    expect(seen).toEqual(["scout", "atlas", null]);
  });
});
