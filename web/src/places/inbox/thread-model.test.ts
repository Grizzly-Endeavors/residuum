import { describe, expect, it, vi } from "vitest";
import {
  deliveryHint,
  moveOnEnter,
  seenLine,
  sendOnModEnter,
  threadMeta,
  uploadLimitHint,
} from "./thread-model";

describe("threadMeta", () => {
  it("names you and the agents, with the message count", () => {
    expect(threadMeta(["atlas", "wren"], 3)).toBe("You and atlas, wren · 3 messages");
    expect(threadMeta(["atlas"], 1)).toBe("You and atlas · 1 message");
    expect(threadMeta([], 2)).toBe("You · 2 messages");
  });
});

describe("uploadLimitHint", () => {
  it("states the upload limit in megabytes", () => {
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

describe("moveOnEnter", () => {
  function press(init: {
    key: string;
    ctrlKey?: boolean;
    metaKey?: boolean;
    isComposing?: boolean;
  }): { moved: boolean; prevented: boolean } {
    const next = vi.fn();
    const preventDefault = vi.fn();
    moveOnEnter(
      { ctrlKey: false, metaKey: false, isComposing: false, ...init, preventDefault },
      next,
    );
    return {
      moved: next.mock.calls.length === 1,
      prevented: preventDefault.mock.calls.length === 1,
    };
  }

  it("moves on with a plain Enter, which would otherwise submit the form", () => {
    expect(press({ key: "Enter" })).toEqual({ moved: true, prevented: true });
  });

  it("leaves other keys, Ctrl or Cmd with Enter, and an Enter that confirms an input method alone", () => {
    expect(press({ key: "a" })).toEqual({ moved: false, prevented: false });
    expect(press({ key: "Enter", ctrlKey: true })).toEqual({ moved: false, prevented: false });
    expect(press({ key: "Enter", metaKey: true })).toEqual({ moved: false, prevented: false });
    expect(press({ key: "Enter", isComposing: true })).toEqual({ moved: false, prevented: false });
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
