import { describe, expect, it } from "vitest";
import { formatLocation, MAIN_CHAT, parseLocation, type AppLocation } from "./routes";

const IN_SESSION = { runId: "run-1790000000000-0a1b2c3d", workspace: true };

describe("parseLocation", () => {
  it("reads the root as the main chat", () => {
    expect(parseLocation("/", "", IN_SESSION)).toEqual({
      location: { chat: MAIN_CHAT, settings: null },
      corrected: false,
    });
  });

  it("reads a session path and the workspace flag", () => {
    expect(parseLocation("/sessions/run-1-ab", "?workspace", MAIN_CHAT)).toEqual({
      location: { chat: { runId: "run-1-ab", workspace: true }, settings: null },
      corrected: false,
    });
  });

  it("decodes an encoded run id", () => {
    const { location } = parseLocation("/sessions/run%20x", "", MAIN_CHAT);
    expect(location.chat.runId).toBe("run x");
  });

  it("keeps the chat side as it was when opening settings", () => {
    expect(parseLocation("/settings/memory", "", IN_SESSION)).toEqual({
      location: { chat: IN_SESSION, settings: "memory" },
      corrected: false,
    });
  });

  it("sends bare /settings to the first section", () => {
    expect(parseLocation("/settings", "", MAIN_CHAT)).toEqual({
      location: { chat: MAIN_CHAT, settings: "runtime" },
      corrected: true,
    });
  });

  it("corrects an unknown settings section to the first section", () => {
    expect(parseLocation("/settings/nope", "", MAIN_CHAT)).toEqual({
      location: { chat: MAIN_CHAT, settings: "runtime" },
      corrected: true,
    });
  });

  it.each(["/nope", "/sessions", "/sessions/a/b", "/sessions/%E0%A4%A", "/settings/runtime/x"])(
    "corrects %s to the main chat",
    (path) => {
      expect(parseLocation(path, "", IN_SESSION)).toEqual({
        location: { chat: MAIN_CHAT, settings: null },
        corrected: true,
      });
    },
  );

  it("tolerates a trailing slash", () => {
    expect(parseLocation("/sessions/run-1/", "", MAIN_CHAT).corrected).toBe(false);
  });
});

describe("formatLocation", () => {
  it.each<[AppLocation, string]>([
    [{ chat: MAIN_CHAT, settings: null }, "/"],
    [{ chat: { runId: null, workspace: true }, settings: null }, "/?workspace"],
    [{ chat: { runId: "run-1", workspace: false }, settings: null }, "/sessions/run-1"],
    [{ chat: { runId: "run-1", workspace: true }, settings: null }, "/sessions/run-1?workspace"],
    [{ chat: IN_SESSION, settings: "agent-keys" }, "/settings/agent-keys"],
  ])("formats %j as %s", (location, url) => {
    expect(formatLocation(location)).toBe(url);
  });

  it("round-trips through parseLocation", () => {
    const location: AppLocation = {
      chat: { runId: "run/odd id", workspace: true },
      settings: null,
    };
    const url = new URL(formatLocation(location), "http://localhost");
    expect(parseLocation(url.pathname, url.search, MAIN_CHAT)).toEqual({
      location,
      corrected: false,
    });
  });
});
