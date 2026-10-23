import { describe, expect, it } from "vitest";
import { joinNames } from "./words";

describe("joinNames", () => {
  it("lists names as a sentence does", () => {
    expect(joinNames([])).toBe("");
    expect(joinNames(["atlas"])).toBe("atlas");
    expect(joinNames(["atlas", "scout"])).toBe("atlas and scout");
    expect(joinNames(["atlas", "scout", "wren"])).toBe("atlas, scout and wren");
  });

  it("joins the last two with the word given", () => {
    expect(joinNames(["atlas", "scout"], "or")).toBe("atlas or scout");
    expect(joinNames(["atlas", "scout", "wren"], "or")).toBe("atlas, scout or wren");
  });
});
