import { describe, expect, it } from "vitest";
import { validateIcon } from "./icon-utils";

describe("validateIcon", () => {
  it("rejects non-PNG files", async () => {
    const file = new File(["dummy"], "icon.jpg", { type: "image/jpeg" });
    const result = await validateIcon(file, 192, 192);
    expect(result.valid).toBe(false);
    expect(result.error).toBe("Icon must be a PNG image file.");
    expect(result.base64).toBeNull();
  });

  it("validates correct dimensions for PNG file", async () => {
    const originalFileReader = globalThis.FileReader;
    const originalImage = globalThis.Image;

    class MockFileReader {
      onload: ((e: { target: { result: string } }) => void) | null = null;
      readAsDataURL(): void {
        setTimeout(() => {
          this.onload?.({ target: { result: "data:image/png;base64,iVBORw0KGgoAAAANSUhEUg==" } });
        }, 0);
      }
    }

    class MockImage {
      naturalWidth = 192;
      naturalHeight = 192;
      onload: (() => void) | null = null;
      set src(_val: string) {
        setTimeout(() => {
          this.onload?.();
        }, 0);
      }
    }

    // @ts-expect-error Mock FileReader global in test environment
    globalThis.FileReader = MockFileReader;
    // @ts-expect-error Mock Image global in test environment
    globalThis.Image = MockImage;

    try {
      const file = new File(["dummy"], "icon.png", { type: "image/png" });
      const result = await validateIcon(file, 192, 192);
      expect(result.valid).toBe(true);
      expect(result.error).toBeNull();
      expect(result.base64).toBe("iVBORw0KGgoAAAANSUhEUg==");
    } finally {
      globalThis.FileReader = originalFileReader;
      globalThis.Image = originalImage;
    }
  });

  it("rejects incorrect dimensions for PNG file", async () => {
    const originalFileReader = globalThis.FileReader;
    const originalImage = globalThis.Image;

    class MockFileReader {
      onload: ((e: { target: { result: string } }) => void) | null = null;
      readAsDataURL(): void {
        setTimeout(() => {
          this.onload?.({ target: { result: "data:image/png;base64,dGVzdA==" } });
        }, 0);
      }
    }

    class MockImage {
      naturalWidth = 100;
      naturalHeight = 100;
      onload: (() => void) | null = null;
      set src(_val: string) {
        setTimeout(() => {
          this.onload?.();
        }, 0);
      }
    }

    // @ts-expect-error Mock FileReader global in test environment
    globalThis.FileReader = MockFileReader;
    // @ts-expect-error Mock Image global in test environment
    globalThis.Image = MockImage;

    try {
      const file = new File(["dummy"], "icon.png", { type: "image/png" });
      const result = await validateIcon(file, 192, 192);
      expect(result.valid).toBe(false);
      expect(result.error).toBe("Icon must be exactly 192×192 pixels (found 100×100).");
      expect(result.base64).toBeNull();
    } finally {
      globalThis.FileReader = originalFileReader;
      globalThis.Image = originalImage;
    }
  });
});
