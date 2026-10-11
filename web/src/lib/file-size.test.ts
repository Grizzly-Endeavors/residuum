import { describe, expect, it } from "vitest";
import { fileSize } from "./file-size";

describe("a file's size", () => {
  it("reads in bytes, kilobytes or megabytes", () => {
    expect(fileSize(812)).toBe("812 B");
    expect(fileSize(4300)).toBe("4.2 KB");
    expect(fileSize(1_363_149)).toBe("1.3 MB");
  });
});
