import { describe, expect, it } from "vitest";
import { render, screen } from "../test/component";
import type { SessionUsageTotals } from "../lib/types";
import ChatFooter from "./ChatFooter.svelte";

function usage(overrides: Partial<SessionUsageTotals> = {}): SessionUsageTotals {
  return {
    input_tokens: 100,
    output_tokens: 20,
    context_tokens: null,
    tool_calls: 0,
    ...overrides,
  };
}

describe("ChatFooter", () => {
  it("shows the cumulative tool-call count next to tokens", () => {
    render(ChatFooter, { usage: usage({ tool_calls: 12 }), model: null });
    expect(screen.getByText("12 tool calls")).toBeTruthy();
  });

  it("omits the tool-call segment when the session has run none yet", () => {
    render(ChatFooter, { usage: usage({ tool_calls: 0 }), model: null });
    expect(screen.queryByText(/tool call/)).toBeNull();
  });

  it("uses the singular form for exactly one tool call", () => {
    render(ChatFooter, { usage: usage({ tool_calls: 1 }), model: null });
    expect(screen.getByText("1 tool call")).toBeTruthy();
  });
});
