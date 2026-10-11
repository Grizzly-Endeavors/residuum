import { describe, expect, it } from "vitest";
import {
  configFileAt,
  historyLocation,
  isIdentityFile,
  parentDir,
  sameSource,
  type FileSource,
} from "./file-source";

const ATLAS: FileSource = { agent: "atlas", scope: "agent" };
const TEAM: FileSource = { agent: null, scope: "team" };

describe("identity files", () => {
  it("tints the agent's own identity files at its root", () => {
    expect(isIdentityFile(ATLAS, "SOUL.md")).toBe(true);
    expect(isIdentityFile(ATLAS, "HEARTBEAT.yml")).toBe(true);
    expect(isIdentityFile(ATLAS, "notes/SOUL.md")).toBe(false);
  });

  it("no longer tints CHANNELS.yml, which agents don't read", () => {
    expect(isIdentityFile(ATLAS, "CHANNELS.yml")).toBe(false);
  });

  it("tints the team's rules and user facts, in an agent's team folder and in Shared files", () => {
    expect(isIdentityFile(ATLAS, "team/AGENTS.md")).toBe(true);
    expect(isIdentityFile(TEAM, "AGENTS.md")).toBe(true);
    expect(isIdentityFile(TEAM, "USER.md")).toBe(true);
    expect(isIdentityFile(ATLAS, "AGENTS.md")).toBe(false);
    expect(isIdentityFile(TEAM, "SOUL.md")).toBe(false);
    expect(isIdentityFile(TEAM, "wiki/AGENTS.md")).toBe(false);
  });
});

describe("config files", () => {
  it("names an agent's three config files, which save through the coordinator", () => {
    expect(configFileAt(ATLAS, "config/config.toml")).toEqual({
      kind: "agent",
      agent: "atlas",
      name: "config",
    });
    expect(configFileAt(ATLAS, "config/providers.toml")).toMatchObject({ name: "providers" });
    expect(configFileAt(ATLAS, "config/mcp.json")).toMatchObject({ name: "mcp" });
  });

  it("leaves every other file to the workspace routes", () => {
    expect(configFileAt(ATLAS, "config/channels.toml")).toBeNull();
    expect(configFileAt(ATLAS, "config.toml")).toBeNull();
    expect(configFileAt(ATLAS, "toString")).toBeNull();
    expect(configFileAt(TEAM, "config/config.toml")).toBeNull();
  });
});

describe("where a file's history is", () => {
  it("keeps an agent's own files in its workspace repository", () => {
    expect(historyLocation(ATLAS, "memory/notes.md")).toEqual({
      repo: "workspace",
      path: "memory/notes.md",
    });
    expect(historyLocation(ATLAS, "config/mcp.json")).toEqual({
      repo: "workspace",
      path: "config/mcp.json",
    });
  });

  it("keeps config.toml and providers.toml in the agent-config repository", () => {
    expect(historyLocation(ATLAS, "config/config.toml")).toEqual({
      repo: "agent_config",
      path: "config.toml",
    });
    expect(historyLocation(ATLAS, "config/providers.toml")).toEqual({
      repo: "agent_config",
      path: "providers.toml",
    });
  });

  it("keeps team files in the team repository, relative to the team folder", () => {
    expect(historyLocation(ATLAS, "team/wiki/x.md")).toEqual({ repo: "team", path: "wiki/x.md" });
    expect(historyLocation(TEAM, "wiki/x.md")).toEqual({ repo: "team", path: "wiki/x.md" });
  });
});

describe("paths and sources", () => {
  it("finds a path's folder", () => {
    expect(parentDir("team/wiki/x.md")).toBe("team/wiki");
    expect(parentDir("SOUL.md")).toBe("");
  });

  it("tells sources apart by agent and scope", () => {
    expect(sameSource(ATLAS, { agent: "atlas", scope: "agent" })).toBe(true);
    expect(sameSource(ATLAS, { agent: "scout", scope: "agent" })).toBe(false);
    expect(sameSource(TEAM, ATLAS)).toBe(false);
    expect(sameSource(null, TEAM)).toBe(false);
  });
});
