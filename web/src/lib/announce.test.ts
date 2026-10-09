import { describe, expect, it } from "vitest";
import {
  failedAnnouncement,
  repliedAnnouncement,
  replyExcerpt,
  workingAnnouncement,
} from "./announce";

describe("replyExcerpt", () => {
  it("keeps a short reply whole", () => {
    expect(replyExcerpt("Done. The port is set once now.")).toBe("Done. The port is set once now.");
  });

  it("drops Markdown marks, so they aren't read out", () => {
    expect(
      replyExcerpt("## Key points\n\n1. **Config** lives in `config.toml`\n- _two_ more"),
    ).toBe("Key points Config lives in config.toml two more");
  });

  it("keeps the underscores inside a name", () => {
    expect(replyExcerpt("Ran _memory_search_ twice")).toBe("Ran memory_search twice");
  });

  it("reads a link by its text, and an image by its alt text", () => {
    expect(replyExcerpt("See [the wiki](https://example.com/x) and ![a chart](chart.png).")).toBe(
      "See the wiki and a chart.",
    );
  });

  it("says code where a block of it stood, even one that was never closed", () => {
    expect(replyExcerpt("Try this:\n```toml\nport = 1\n```\nThen restart.")).toBe(
      "Try this: code Then restart.",
    );
    expect(replyExcerpt("Run it:\n```sh\nls -la")).toBe("Run it: code");
  });

  it("cuts a long reply at a word, with an ellipsis", () => {
    const long = "word ".repeat(60);
    const excerpt = replyExcerpt(long);
    expect(excerpt.endsWith("word…")).toBe(true);
    expect(excerpt.length).toBeLessThanOrEqual(141);
  });

  it("cuts at the limit when there is no word to stop at", () => {
    expect(replyExcerpt("x".repeat(300))).toBe(`${"x".repeat(140)}…`);
  });

  it("is empty when nothing readable is left", () => {
    expect(replyExcerpt("")).toBe("");
    expect(replyExcerpt("  \n ** \n")).toBe("");
  });
});

describe("announcements", () => {
  it("say what the agent is doing, in the agent's name", () => {
    expect(workingAnnouncement("atlas")).toBe("atlas is working");
    expect(failedAnnouncement("atlas")).toBe("atlas couldn't finish");
  });

  it("say a reply is complete, with the start of it", () => {
    expect(repliedAnnouncement("atlas", "**Done.** Fixed the port.")).toBe(
      "atlas replied: Done. Fixed the port.",
    );
    expect(repliedAnnouncement("atlas", "```\nonly code\n```")).toBe("atlas replied: code");
    expect(repliedAnnouncement("atlas", "  ")).toBe("atlas replied");
  });
});
