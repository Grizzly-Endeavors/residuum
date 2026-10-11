import { describe, expect, it } from "vitest";
import { imageProblem, MAX_IMAGE_BYTES } from "./image-attachments";

describe("which images can be attached", () => {
  it("takes JPEG, PNG, GIF and WebP up to 5 MB", () => {
    for (const type of ["image/jpeg", "image/png", "image/gif", "image/webp"]) {
      expect(imageProblem({ name: "a", type, size: MAX_IMAGE_BYTES })).toBeNull();
    }
  });

  it("names a file of another type, and what to attach instead", () => {
    expect(imageProblem({ name: "notes.pdf", type: "application/pdf", size: 10 })).toBe(
      "notes.pdf can't be attached. Attach a JPEG, PNG, GIF or WebP image.",
    );
  });

  it("names an image over 5 MB", () => {
    expect(imageProblem({ name: "big.png", type: "image/png", size: MAX_IMAGE_BYTES + 1 })).toBe(
      "big.png is over 5 MB. Attach an image of 5 MB or less.",
    );
  });
});
