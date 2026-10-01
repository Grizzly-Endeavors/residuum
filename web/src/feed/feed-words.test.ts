import { describe, expect, it } from "vitest";
import { cardSender } from "./feed-words";

describe("cardSender", () => {
  it("names a session by its kind and address, and lets it be opened", () => {
    expect(cardSender("spawned-research-3f9a", "spawned", "atlas")).toEqual({
      kind: "Background session",
      sender: "spawned-research-3f9a",
      icon: "layers",
      isSession: true,
    });
    expect(cardSender("scheduled-audit-1", "scheduled", "atlas").kind).toBe("Scheduled session");
    expect(cardSender("discord-builds", "external", "atlas").kind).toBe(
      "Conversation in another app",
    );
    expect(cardSender("artifact-tip-1", "artifact", "atlas").kind).toBe("Workbench page session");
  });

  it("calls a session of unknown kind a session", () => {
    expect(cardSender("spawned-x-1", null, "atlas")).toMatchObject({
      kind: "Session",
      isSession: true,
    });
  });

  it("names a teammate without offering a session to open", () => {
    expect(cardSender("agent:scout", "teammate", "atlas")).toEqual({
      kind: "Teammate",
      sender: "scout",
      icon: "users",
      isSession: false,
    });
    // A teammate's session is still the teammate's, not one of this agent's.
    expect(cardSender("agent:scout/research-2", null, "atlas")).toMatchObject({
      sender: "scout/research-2",
      isSession: false,
    });
  });

  it("names the main conversation by the feed's agent", () => {
    expect(cardSender("main", "main", "atlas")).toMatchObject({
      kind: "Main conversation",
      sender: "atlas",
      isSession: false,
    });
  });
});
