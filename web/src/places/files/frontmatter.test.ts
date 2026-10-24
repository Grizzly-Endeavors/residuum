import { describe, expect, it } from "vitest";
import { splitFrontmatter } from "./frontmatter";

describe("splitFrontmatter", () => {
  it("splits key lines from the body after the closing rule", () => {
    const split = splitFrontmatter("---\ntype: Agent\ntitle: Atlas\n---\n# Body\n");
    expect(split).toEqual({
      fields: [
        { key: "type", value: "Agent" },
        { key: "title", value: "Atlas" },
      ],
      body: "# Body\n",
    });
  });

  it("keeps the lines under a key with no inline value as its value", () => {
    const split = splitFrontmatter("---\ntags:\n  - one\n  - two\nname: x\n---\nbody");
    expect(split?.fields[0]).toEqual({ key: "tags", value: "- one\n- two" });
    expect(split?.fields[1]).toEqual({ key: "name", value: "x" });
  });

  it("drops comment lines", () => {
    const split = splitFrontmatter("---\n# a note\nname: x\n---\n");
    expect(split?.fields).toEqual([{ key: "name", value: "x" }]);
  });

  it("returns null when there is no frontmatter, or it holds no key", () => {
    expect(splitFrontmatter("# Plain\n")).toBeNull();
    expect(splitFrontmatter("---\n\n---\nbody")).toBeNull();
    expect(splitFrontmatter("---\nnot a key line\n---\nbody")).toBeNull();
  });

  it("needs its closing rule", () => {
    expect(splitFrontmatter("---\nname: x\nbody without a close")).toBeNull();
  });
});
