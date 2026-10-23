// ── Staged attachments (Svelte 5 runes) ──────────────────────────────
//
// Files attached to a message being written, in the mail composers and the
// chat composer alike. Each file starts uploading to the hub
// (`POST /api/hub/uploads`) the moment it is attached, and the message is
// sent with the hub's ids for them, never the bytes. A file over the upload
// limit is refused before anything is uploaded; a failed upload can be tried
// again; removing a file that is still uploading cancels its upload.

import { MAX_UPLOAD_BYTES, UPLOAD_LIMIT_LABEL, uploadFile } from "./api";
import { userErrorMessage } from "./errors";

/** How far a staged attachment has got: uploading, staged on the hub, or failed. */
export type AttachmentStatus = "uploading" | "uploaded" | "failed";

/** `names` as a sentence lists them: "a", "a and b", "a, b and c". */
export function listing(names: readonly string[]): string {
  if (names.length <= 1) return names.join("");
  return `${names.slice(0, -1).join(", ")} and ${names[names.length - 1] ?? ""}`;
}

let attachmentCount = 0;

/**
 * A file picked for a message. Its upload starts as soon as it is picked; the
 * message is sent with the hub's id for it.
 */
export class StagedAttachment {
  /** Stable for keyed lists while a message holds it. */
  readonly key = `attachment-${String(++attachmentCount)}`;
  readonly filename: string;
  readonly size: number;
  /** The file's MIME type, or "" when the browser doesn't know it. */
  readonly contentType: string;
  status = $state<AttachmentStatus>("uploading");
  /** The hub's id for the staged file, once uploaded. */
  uploadId = $state<string | null>(null);
  /** Why the upload failed, in plain words, or null. */
  error = $state<string | null>(null);
  /** Settles when the current upload attempt does, whichever way it goes. */
  done: Promise<void> = Promise.resolve();
  /** Taken off the message: a late upload answer changes nothing. */
  private removed = false;
  /** Cancels the upload attempt in flight. */
  private controller: AbortController | null = null;

  constructor(readonly file: File) {
    this.filename = file.name;
    this.size = file.size;
    this.contentType = file.type;
  }

  /** Upload the file, or upload it again after a failure. */
  upload(): Promise<void> {
    if (this.status === "uploaded") return this.done;
    this.status = "uploading";
    this.error = null;
    const controller = new AbortController();
    this.controller = controller;
    this.done = (async () => {
      try {
        const staged = await uploadFile(this.file, this.filename, { signal: controller.signal });
        if (this.removed) return;
        this.uploadId = staged.id;
        this.status = "uploaded";
      } catch (err) {
        if (this.removed) return;
        this.error = userErrorMessage(err, { action: `Couldn't upload ${this.filename}.` });
        this.status = "failed";
      }
    })();
    return this.done;
  }

  /** Take it off the message: an upload still running is cancelled, and nothing it answers later counts. */
  remove(): void {
    this.removed = true;
    this.controller?.abort();
  }
}

/** The files attached to one message being written, in the order they were attached. */
export class StagedAttachments {
  attachments = $state<StagedAttachment[]>([]);
  /** Files that weren't attached because they are too large, said in plain words, or null. */
  notice = $state<string | null>(null);

  /** Some attachment is still uploading. */
  get uploading(): boolean {
    return this.attachments.some((attachment) => attachment.status === "uploading");
  }

  /** The attachments whose upload failed. */
  get failed(): StagedAttachment[] {
    return this.attachments.filter((attachment) => attachment.status === "failed");
  }

  /** Why the message can't go with its files as they stand, in plain words, or null when every file is uploaded. */
  get problem(): string | null {
    const failed = this.failed;
    if (failed.length > 0) {
      const one = failed.length === 1;
      return `${listing(failed.map((attachment) => attachment.filename))} didn't upload. Try ${one ? "it" : "them"} again or remove ${one ? "it" : "them"}, then send.`;
    }
    const uploading = this.attachments.filter((attachment) => attachment.status === "uploading");
    if (uploading.length > 0) {
      const names = listing(uploading.map((attachment) => attachment.filename));
      return `Wait for ${names} to finish uploading, then send.`;
    }
    return null;
  }

  /** The hub's id for every attachment, once each is uploaded. */
  get uploadIds(): string[] {
    return this.attachments.map((attachment) => attachment.uploadId ?? "");
  }

  /**
   * Attach `files`: each starts uploading at once. A file over the upload
   * limit is refused before any upload, and `notice` names it.
   */
  attach(files: Iterable<File>): void {
    const refused: string[] = [];
    const added: StagedAttachment[] = [];
    for (const file of files) {
      if (file.size > MAX_UPLOAD_BYTES) {
        refused.push(file.name);
        continue;
      }
      const attachment = new StagedAttachment(file);
      void attachment.upload();
      added.push(attachment);
    }
    const one = refused.length === 1;
    this.notice =
      refused.length === 0
        ? null
        : `${listing(refused)} ${one ? "is" : "are"} larger than the ${UPLOAD_LIMIT_LABEL} upload limit, so ${one ? "it wasn't" : "they weren't"} attached.`;
    if (added.length > 0) this.attachments = [...this.attachments, ...added];
  }

  /** Upload a failed attachment again. */
  retry(key: string): Promise<void> {
    const attachment = this.attachments.find((held) => held.key === key);
    return attachment?.status === "failed" ? attachment.upload() : Promise.resolve();
  }

  /** Take an attachment off the message, uploaded or not. */
  remove(key: string): void {
    const attachment = this.attachments.find((held) => held.key === key);
    if (attachment === undefined) return;
    attachment.remove();
    this.attachments = this.attachments.filter((held) => held !== attachment);
  }

  dismissNotice(): void {
    this.notice = null;
  }

  /** Wait for the uploads still running, including those of files attached while waiting. */
  async settled(): Promise<void> {
    while (this.uploading) {
      await Promise.all(this.attachments.map((attachment) => attachment.done));
    }
  }

  /**
   * Hand every attachment over to a message that is going out, and empty the
   * list. Nothing is cancelled: the message now holds them, and can give them
   * back with `restore` if it doesn't go.
   */
  take(): StagedAttachment[] {
    const taken = this.attachments;
    this.attachments = [];
    this.notice = null;
    return taken;
  }

  /** Put back attachments of a message that didn't go, ahead of any attached since. */
  restore(returned: readonly StagedAttachment[]): void {
    const back = returned.filter((attachment) => !this.attachments.includes(attachment));
    if (back.length > 0) this.attachments = [...back, ...this.attachments];
  }

  /** Throw every attachment away: uploads still running are cancelled. */
  clear(): void {
    for (const attachment of this.attachments) attachment.remove();
    this.attachments = [];
    this.notice = null;
  }
}
