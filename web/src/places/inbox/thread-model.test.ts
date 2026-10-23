import { describe, expect, it, vi } from "vitest";
import { MAX_UPLOAD_BYTES } from "../../lib/api";
import {
  deliveryHint,
  joinNames,
  opensInPanel,
  seenLine,
  sendOnModEnter,
  threadMeta,
  uploadLimitHint,
} from "./thread-model";

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

describe("threadMeta", () => {
  it("names you and the agents, with the message count", () => {
    expect(threadMeta(["atlas", "wren"], 3)).toBe("You and atlas, wren · 3 messages");
    expect(threadMeta(["atlas"], 1)).toBe("You and atlas · 1 message");
    expect(threadMeta([], 2)).toBe("You · 2 messages");
  });
});

describe("uploadLimitHint", () => {
  it("states the upload limit in megabytes", () => {
    expect(MAX_UPLOAD_BYTES).toBe(50 * 1024 * 1024);
    expect(uploadLimitHint()).toBe("Any file, up to 50 MB each.");
  });
});

describe("deliveryHint", () => {
  it("names a single recipient, and speaks of them all for several", () => {
    expect(deliveryHint(["atlas"])).toBe("atlas will see this next time it checks its inbox.");
    expect(deliveryHint(["atlas", "scout"])).toBe(
      "They’ll see this next time they check their inbox.",
    );
  });
});

describe("sendOnModEnter", () => {
  function press(init: { key: string; ctrlKey?: boolean; metaKey?: boolean; shiftKey?: boolean }): {
    sent: boolean;
    prevented: boolean;
  } {
    const send = vi.fn();
    const preventDefault = vi.fn();
    sendOnModEnter({ ctrlKey: false, metaKey: false, ...init, preventDefault }, send);
    return {
      sent: send.mock.calls.length === 1,
      prevented: preventDefault.mock.calls.length === 1,
    };
  }

  it("sends on Enter with Ctrl or Cmd, and keeps the Enter from adding a line", () => {
    expect(press({ key: "Enter", ctrlKey: true })).toEqual({ sent: true, prevented: true });
    expect(press({ key: "Enter", metaKey: true })).toEqual({ sent: true, prevented: true });
  });

  it("leaves a plain Enter and other keys alone", () => {
    expect(press({ key: "Enter" })).toEqual({ sent: false, prevented: false });
    expect(press({ key: "Enter", shiftKey: true })).toEqual({ sent: false, prevented: false });
    expect(press({ key: "a", ctrlKey: true })).toEqual({ sent: false, prevented: false });
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

  it("says where a single recipient's mail is waiting", () => {
    expect(seenLine([{ agent: "atlas", seen: false }])).toBe(
      "Not seen yet · waiting in atlas’s inbox",
    );
    expect(seenLine([{ agent: "atlas", seen: true }])).toBe("Seen by atlas");
  });
});
