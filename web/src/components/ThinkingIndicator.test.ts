import { describe, expect, it } from "vitest";
import { render, screen } from "../test/component";
import ThinkingIndicator from "./ThinkingIndicator.svelte";

describe("ThinkingIndicator", () => {
  it("shows the tool-call count next to elapsed time and tokens", () => {
    render(ThinkingIndicator, {
      since: null,
      outputTokens: 120,
      hasUsage: true,
      toolCalls: 4,
    });
    expect(screen.getByText(/4 tool calls/)).toBeTruthy();
  });

  it("omits the tool-call segment when no tools have run yet", () => {
    render(ThinkingIndicator, {
      since: null,
      outputTokens: 10,
      hasUsage: true,
      toolCalls: 0,
    });
    expect(screen.queryByText(/tool call/)).toBeNull();
  });

  it("uses the singular form for exactly one tool call", () => {
    render(ThinkingIndicator, {
      since: null,
      outputTokens: 10,
      hasUsage: true,
      toolCalls: 1,
    });
    expect(screen.getByText(/1 tool call(?!s)/)).toBeTruthy();
  });

  it("shows the tool-call count even when no token usage has been reported yet", () => {
    render(ThinkingIndicator, {
      since: null,
      outputTokens: 0,
      hasUsage: false,
      toolCalls: 2,
    });
    expect(screen.getByText(/2 tool calls/)).toBeTruthy();
    expect(screen.queryByText(/tokens/)).toBeNull();
  });
});
