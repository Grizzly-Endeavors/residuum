import { describe, expect, it } from "vitest";
import {
  attachmentDownloadPath,
  opensInPanel,
  withoutAttachmentLines,
} from "./message-attachments";

describe("opensInPanel", () => {
  it("opens Markdown and text files beside the thread, whatever their case", () => {
    expect(opensInPanel({ filename: "notes.md" })).toBe(true);
    expect(opensInPanel({ filename: "LOG.TXT" })).toBe(true);
    expect(opensInPanel({ filename: "report.pdf" })).toBe(false);
    expect(opensInPanel({ filename: "md" })).toBe(false);
  });
});

describe("attachmentDownloadPath", () => {
  it("names the agent's inbox item's first file", () => {
    expect(attachmentDownloadPath("atlas", "item-7")).toBe(
      "/api/agents/atlas/inbox/agent/item-7/attachments/0",
    );
  });
});

describe("withoutAttachmentLines", () => {
  const notes = { filename: "notes.md" };
  const pdf = { filename: "plan.pdf" };

  it("hides the line each attachment stands for and keeps the other text", () => {
    const content = [
      "Here is the plan.",
      "[Attachment: notes.md (120 bytes) → inbox/agent/attachments/a1/notes.md]",
      "[Attachment: plan.pdf (2048 bytes) → inbox/agent/attachments/a2/plan.pdf]",
      "Thanks.",
    ].join("\n");
    expect(withoutAttachmentLines(content, [notes, pdf])).toBe("Here is the plan.\nThanks.");
  });

  it("keeps a download-failed line, which has an em dash in place of the arrow", () => {
    const content = "Look\n[Attachment: plan.pdf (2048 bytes) — download failed: timeout]";
    expect(withoutAttachmentLines(content, [pdf])).toBe(content);
  });

  it("removes only one line per attachment, so a repeated name keeps its second line", () => {
    const content = [
      "[Attachment: notes.md (1 bytes) → inbox/agent/attachments/a1/notes.md]",
      "[Attachment: notes.md (2 bytes) → inbox/agent/attachments/a2/notes.md]",
    ].join("\n");
    expect(withoutAttachmentLines(content, [notes])).toBe(
      "[Attachment: notes.md (2 bytes) → inbox/agent/attachments/a2/notes.md]",
    );
  });

  it("returns the text unchanged when there are no attachments", () => {
    expect(withoutAttachmentLines("Hello\n\nthere", [])).toBe("Hello\n\nthere");
  });
});
