import { describe, expect, it } from "vitest";
import { sourceLabel } from "./inbox-model";

describe("an item's source in words", () => {
  it("says nothing more when the agent sent it itself", () => {
    expect(sourceLabel("agent")).toBeNull();
  });

  it("keeps what an agent named as the source", () => {
    expect(sourceLabel("agent:digest")).toBe("digest");
  });

  it("names the kinds of sender people know", () => {
    expect(sourceLabel("hub")).toBe("Residuum");
    expect(sourceLabel("pulse:Inbox check")).toBe("Regular check: Inbox check");
    expect(sourceLabel("action:Send the report")).toBe("Scheduled action: Send the report");
  });

  it("shows a kind it doesn't know as it came", () => {
    expect(sourceLabel("discord")).toBe("discord");
    expect(sourceLabel("telegram:alerts")).toBe("telegram: alerts");
  });
});
