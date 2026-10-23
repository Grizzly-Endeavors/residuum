/** What an attachment chip knows about a file from its name and content type. */

import type { IconName } from "../icons";

/** How a chip draws a file: a picture, a text or page document, or any other file. */
export type AttachmentKind = "image" | "document" | "file";

export const ATTACHMENT_ICONS: Readonly<Record<AttachmentKind, IconName>> = {
  image: "image",
  document: "document",
  file: "file",
};

const IMAGE_EXTENSIONS: ReadonlySet<string> = new Set([
  "png",
  "jpg",
  "jpeg",
  "gif",
  "webp",
  "avif",
  "bmp",
  "svg",
  "heic",
  "heif",
  "tif",
  "tiff",
  "ico",
]);

const DOCUMENT_EXTENSIONS: ReadonlySet<string> = new Set([
  "md",
  "markdown",
  "txt",
  "text",
  "pdf",
  "doc",
  "docx",
  "odt",
  "rtf",
]);

/** Document types that are not `text/*`. */
const DOCUMENT_CONTENT_TYPES: ReadonlySet<string> = new Set([
  "application/pdf",
  "application/rtf",
  "application/msword",
  "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
  "application/vnd.oasis.opendocument.text",
]);

/** The longest extension kept apart from the stem, including its dot (".markdown"). */
const MAX_EXTENSION_LENGTH = 9;

/** A filename split so a long stem can truncate while its extension stays in sight. */
export interface SplitFilename {
  readonly stem: string;
  /** The last extension with its dot (".pdf"), or "" when the name has none. */
  readonly extension: string;
}

export function splitFilename(filename: string): SplitFilename {
  const dot = filename.lastIndexOf(".");
  const extensionLength = filename.length - dot;
  if (dot <= 0 || extensionLength < 2 || extensionLength > MAX_EXTENSION_LENGTH) {
    return { stem: filename, extension: "" };
  }
  return { stem: filename.slice(0, dot), extension: filename.slice(dot) };
}

/** Picks the chip's icon from the content type when it says, then from the extension. */
export function attachmentKind(filename: string, contentType?: string): AttachmentKind {
  const type = contentType?.split(";")[0]?.trim().toLowerCase() ?? "";
  if (type.startsWith("image/")) return "image";
  if (type.startsWith("text/") || DOCUMENT_CONTENT_TYPES.has(type)) return "document";

  const extension = splitFilename(filename).extension.slice(1).toLowerCase();
  if (IMAGE_EXTENSIONS.has(extension)) return "image";
  if (DOCUMENT_EXTENSIONS.has(extension)) return "document";
  return "file";
}
