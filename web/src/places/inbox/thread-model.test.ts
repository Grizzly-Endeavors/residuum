import { describe, expect, it } from "vitest";
import { deliveryHint, joinNames, opensInPanel, seenLine } from "./thread-model";

describe("joinNames", () => {
  it("lists names as a sentence does", () => {
    expect(joinNames([])).toBe("");
    expect(joinNames(["atlas"])).toBe("atlas");
    expect(joinNames(["atlas", "scout"])).toBe("atlas and scout");
    expect(joinNames(["atlas", "scout", "wren"], "or")).toBe("atlas, scout or wren");
  });
});

describe("opensInPanel", () => {
  it("opens Markdown and text files beside the thread, whatever their case", () => {
    expect(opensInPanel({ filename: "notes.md" })).toBe(true);
    expect(opensInPanel({ filename: "LOG.TXT" })).toBe(true);
    expect(opensInPanel({ filename: "report.pdf" })).toBe(false);
    expect(opensInPanel({ filename: "md" })).toBe(false);
  });
});

describe("deliveryHint", () => {
  it("names a single recipient, and speaks of each for several", () => {
    expect(deliveryHint(["atlas"])).toBe("atlas sees it next time it checks its inbox.");
    expect(deliveryHint(["atlas", "scout"])).toBe("Each sees it next time it checks its inbox.");
  });
});

describe("seenLine", () => {
  it("says who has seen it and who hasn't yet", () => {
    expect(
      seenLine([
        { agent: "atlas", seen: true },
        { agent: "scout", seen: true },
      ]),
    ).toBe("Seen by atlas and scout");
    expect(
      seenLine([
        { agent: "atlas", seen: false },
        { agent: "scout", seen: false },
      ]),
    ).toBe("Not seen yet by atlas or scout");
    expect(
      seenLine([
        { agent: "atlas", seen: true },
        { agent: "scout", seen: false },
      ]),
    ).toBe("Seen by atlas, not yet by scout");
  });
});
