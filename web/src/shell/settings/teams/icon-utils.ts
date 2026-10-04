/**
 * Client-side validation of Teams app icon dimensions.
 * Color icon: 192x192 PNG. Outline icon: 32x32 PNG.
 */
export async function validateIcon(
  file: File,
  expectedWidth: number,
  expectedHeight: number,
): Promise<{ valid: boolean; base64: string | null; error: string | null }> {
  if (!file.type.includes("png")) {
    return {
      valid: false,
      base64: null,
      error: "Icon must be a PNG image file.",
    };
  }

  return new Promise((resolve) => {
    const reader = new FileReader();
    reader.onload = (e) => {
      const dataUrl = e.target?.result;
      if (typeof dataUrl !== "string") {
        resolve({ valid: false, base64: null, error: "Could not read image file." });
        return;
      }

      const img = new Image();
      img.onload = () => {
        if (img.naturalWidth !== expectedWidth || img.naturalHeight !== expectedHeight) {
          resolve({
            valid: false,
            base64: null,
            error: `Icon must be exactly ${expectedWidth}×${expectedHeight} pixels (found ${img.naturalWidth}×${img.naturalHeight}).`,
          });
        } else {
          // Extract base64 without data:image/png;base64, prefix for API
          const commaIndex = dataUrl.indexOf(",");
          const base64 = commaIndex !== -1 ? dataUrl.slice(commaIndex + 1) : dataUrl;
          resolve({ valid: true, base64, error: null });
        }
      };
      img.onerror = () => {
        resolve({ valid: false, base64: null, error: "Invalid image format." });
      };
      img.src = dataUrl;
    };
    reader.onerror = () => {
      resolve({ valid: false, base64: null, error: "Failed to read image file." });
    };
    reader.readAsDataURL(file);
  });
}
