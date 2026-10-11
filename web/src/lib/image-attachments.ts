// Images attached to a message: which files the model APIs take, and reading
// them into the base64 form a message carries.

import type { ImageAttachment } from "./types";

/** The model APIs' limits, not Residuum's own: these types, up to 5 MB each, any number per message. */
export const IMAGE_TYPES = ["image/jpeg", "image/png", "image/gif", "image/webp"] as const;
export const MAX_IMAGE_BYTES = 5 * 1024 * 1024;

/** Why `file` can't be attached, in words for the user, or null when it can. */
export function imageProblem(file: Pick<File, "name" | "type" | "size">): string | null {
  if (!(IMAGE_TYPES as readonly string[]).includes(file.type)) {
    return `${file.name} can't be attached. Attach a JPEG, PNG, GIF or WebP image.`;
  }
  if (file.size > MAX_IMAGE_BYTES) {
    return `${file.name} is over 5 MB. Attach an image of 5 MB or less.`;
  }
  return null;
}

function readImage(file: File): Promise<ImageAttachment> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      // A data URL: "data:image/png;base64,<data>".
      const url = typeof reader.result === "string" ? reader.result : "";
      const data = url.split(",")[1] ?? "";
      resolve({ media_type: file.type, data });
    };
    reader.onerror = () => {
      reject(reader.error ?? new Error(`failed to read ${file.name}`));
    };
    reader.readAsDataURL(file);
  });
}

/** The files that can be attached, read, and a line naming any that can't. */
export async function readImages(
  files: Iterable<File>,
): Promise<{ images: ImageAttachment[]; problem: string | null }> {
  const images: ImageAttachment[] = [];
  const problems: string[] = [];
  for (const file of files) {
    const problem = imageProblem(file);
    if (problem !== null) {
      problems.push(problem);
      continue;
    }
    try {
      images.push(await readImage(file));
    } catch {
      problems.push(`Couldn't read ${file.name}. Try attaching it again.`);
    }
  }
  return { images, problem: problems.length === 0 ? null : problems.join(" ") };
}
