import { describe, expect, it } from "vitest";
import { attachmentKind, splitFilename } from "./attachment-file";

describe("splitting a filename", () => {
  it("keeps the last extension, with its dot, apart from the stem", () => {
    expect(splitFilename("report.pdf")).toEqual({ stem: "report", extension: ".pdf" });
    expect(splitFilename("backup.tar.gz")).toEqual({ stem: "backup.tar", extension: ".gz" });
  });

  it("leaves a name without a usable extension whole", () => {
    expect(splitFilename("Makefile")).toEqual({ stem: "Makefile", extension: "" });
    expect(splitFilename(".gitignore")).toEqual({ stem: ".gitignore", extension: "" });
    expect(splitFilename("notes.")).toEqual({ stem: "notes.", extension: "" });
    expect(splitFilename("v1.2 final draft of the plan")).toEqual({
      stem: "v1.2 final draft of the plan",
      extension: "",
    });
  });
});

describe("an attachment's kind", () => {
  it("follows the content type when it says", () => {
    expect(attachmentKind("scan", "image/png")).toBe("image");
    expect(attachmentKind("notes", "text/markdown; charset=utf-8")).toBe("document");
    expect(attachmentKind("lease", "application/pdf")).toBe("document");
    expect(attachmentKind("archive", "application/zip")).toBe("file");
  });

  it("falls back to the extension, whatever its case, when the type doesn't say", () => {
    expect(attachmentKind("pharmacy-receipt.JPG")).toBe("image");
    expect(attachmentKind("grant-outline-v1.md", "application/octet-stream")).toBe("document");
    expect(attachmentKind("budget-2027.xlsx")).toBe("file");
    expect(attachmentKind("Makefile")).toBe("file");
  });
});
