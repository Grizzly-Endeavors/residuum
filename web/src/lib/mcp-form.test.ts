import { describe, expect, it } from "vitest";
import {
  argsText,
  pairsText,
  parseArgs,
  parsePairs,
  serverFromCatalog,
  serverNameProblem,
  skippedLinesProblem,
} from "./mcp-form";
import type { McpCatalogEntry, McpServerEntry } from "./types";

describe("a server's arguments", () => {
  it("round-trip one per line, keeping an argument that has a space in it", () => {
    const args = ["--root", "/home/bear/My Documents"];
    expect(parseArgs(argsText(args))).toEqual(args);
  });

  it("leave out blank lines and the spaces around each argument", () => {
    expect(parseArgs("  -y \n\n@org/server\n")).toEqual(["-y", "@org/server"]);
  });
});

describe("variables and headers", () => {
  it("round-trip as NAME=value lines, with an = inside a value kept", () => {
    const pairs = { TOKEN: "abc=def", Authorization: "Bearer ${API_TOKEN}" };
    expect(parsePairs(pairsText(pairs))).toEqual({ pairs, skipped: [] });
  });

  it("note the lines with no name, by number, and leave them out", () => {
    const read = parsePairs("A=1\njust text\n\n=2\nB=");
    expect(read.pairs).toEqual({ A: "1", B: "" });
    expect(read.skipped).toEqual([2, 4]);
    expect(skippedLinesProblem(read.skipped)).toBe(
      "Lines 2, 4 have no NAME= before the value, so they are left out.",
    );
    expect(skippedLinesProblem([3])).toBe(
      "Line 3 has no NAME= before the value, so it is left out.",
    );
    expect(skippedLinesProblem([])).toBeUndefined();
  });
});

describe("a catalog entry made into a server", () => {
  const github: McpCatalogEntry = {
    name: "github",
    description: "GitHub",
    command: "npx",
    args: ["-y", "@org/server-github"],
    env: { GITHUB_TOKEN: "", LOG: "1" },
    category: "dev",
    requires_input: [{ field: "env.GITHUB_TOKEN", label: "GitHub token" }],
    install_hint: "",
  };

  it("fills each input's variable with what was typed, trimmed, and keeps the rest", () => {
    expect(serverFromCatalog(github, { "env.GITHUB_TOKEN": "  ghp_1 " })).toEqual({
      name: "github",
      transport: "stdio",
      command: "npx",
      args: ["-y", "@org/server-github"],
      env: { GITHUB_TOKEN: "ghp_1", LOG: "1" },
    });
  });

  it("doesn't share its arguments or variables with the catalog", () => {
    const made = serverFromCatalog(github, {});
    made.args.push("--more");
    made.env.LOG = "2";
    expect(github.args).toEqual(["-y", "@org/server-github"]);
    expect(github.env.LOG).toBe("1");
  });
});

describe("a new server's name", () => {
  const servers = [{ name: "fetch" }] as McpServerEntry[];

  it("is needed, and can't be one the list already has", () => {
    expect(serverNameProblem("  ", servers)).toBe("Give the server a name.");
    expect(serverNameProblem(" fetch ", servers)).toBe("There's already a server named fetch.");
    expect(serverNameProblem("search", servers)).toBeNull();
  });
});
