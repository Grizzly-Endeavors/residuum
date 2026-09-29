import { describe, expect, it } from "vitest";
import { isPinnedFile } from "./pinned-files";

describe("isPinnedFile", () => {
  it("pins the agent's own identity files at the agent root", () => {
    expect(isPinnedFile("SOUL.md")).toBe(true);
    expect(isPinnedFile("HEARTBEAT.yml")).toBe(true);
    expect(isPinnedFile("CHANNELS.yml")).toBe(true);
  });

  it("pins the team's rules and user facts under team/", () => {
    expect(isPinnedFile("team/AGENTS.md")).toBe(true);
    expect(isPinnedFile("team/USER.md")).toBe(true);
  });

  it("does not pin team files at the agent root or look-alikes elsewhere", () => {
    expect(isPinnedFile("AGENTS.md")).toBe(false);
    expect(isPinnedFile("USER.md")).toBe(false);
    expect(isPinnedFile("notes/SOUL.md")).toBe(false);
    expect(isPinnedFile("team/SOUL.md")).toBe(false);
    expect(isPinnedFile("team/wiki/AGENTS.md")).toBe(false);
  });
});
