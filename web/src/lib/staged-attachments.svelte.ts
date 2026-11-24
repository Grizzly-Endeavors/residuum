import { MAX_UPLOAD_BYTES, UPLOAD_LIMIT_LABEL, uploadFile } from "./api";
import { userErrorMessage } from "./errors";
import { joinNames } from "./words";

export type AttachmentStatus = "uploading" | "uploaded" | "failed";

let attachmentCount = 0;

export class StagedAttachment {
  readonly key = `attachment-${String(++attachmentCount)}`;
  readonly filename: string;
  readonly size: number;
  readonly contentType: string;
  status = $state<AttachmentStatus>("uploading");
  uploadId = $state<string | null>(null);
  error = $state<string | null>(null);
  done: Promise<void> = Promise.resolve();
  private removed = false;
  private controller: AbortController | null = null;

  constructor(readonly file: File) {
    this.filename = file.name;
    this.size = file.size;
    this.contentType = file.type;
  }

  upload(): Promise<void> {
    if (this.status === "uploaded") return this.done;
    return this.startUpload();
  }

  uploadAgain(): Promise<void> {
    if (this.removed) return this.done;
    this.controller?.abort();
    this.uploadId = null;
    return this.startUpload();
  }

  private startUpload(): Promise<void> {
    this.status = "uploading";
    this.error = null;
    const controller = new AbortController();
    this.controller = controller;
    const superseded = (): boolean => this.removed || this.controller !== controller;
    this.done = (async () => {
      try {
        const staged = await uploadFile(this.file, this.filename, { signal: controller.signal });
        if (superseded()) return;
        this.uploadId = staged.id;
        this.status = "uploaded";
      } catch (err) {
        if (superseded()) return;
        this.error = userErrorMessage(err, { action: `Couldn't upload ${this.filename}.` });
        this.status = "failed";
      }
    })();
    return this.done;
  }

  remove(): void {
    this.removed = true;
    this.controller?.abort();
  }
}

export class StagedAttachments {
  attachments = $state<StagedAttachment[]>([]);
  notice = $state<string | null>(null);

  get uploading(): boolean {
    return this.attachments.some((attachment) => attachment.status === "uploading");
  }

  get failed(): StagedAttachment[] {
    return this.attachments.filter((attachment) => attachment.status === "failed");
  }

  get problem(): string | null {
    const failed = this.failed;
    if (failed.length > 0) {
      const one = failed.length === 1;
      return `${joinNames(failed.map((attachment) => attachment.filename))} didn't upload. Try ${one ? "it" : "them"} again or remove ${one ? "it" : "them"}, then send.`;
    }
    const uploading = this.attachments.filter((attachment) => attachment.status === "uploading");
    if (uploading.length > 0) {
      const names = joinNames(uploading.map((attachment) => attachment.filename));
      return `Wait for ${names} to finish uploading, then send.`;
    }
    return null;
  }

  get uploadIds(): string[] {
    return this.attachments.map((attachment) => attachment.uploadId ?? "");
  }

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
        : `${joinNames(refused)} ${one ? "is" : "are"} larger than the ${UPLOAD_LIMIT_LABEL} upload limit, so ${one ? "it wasn't" : "they weren't"} attached.`;
    if (added.length > 0) this.attachments = [...this.attachments, ...added];
  }

  retry(key: string): Promise<void> {
    const attachment = this.attachments.find((held) => held.key === key);
    return attachment?.status === "failed" ? attachment.upload() : Promise.resolve();
  }

  remove(key: string): void {
    const attachment = this.attachments.find((held) => held.key === key);
    if (attachment === undefined) return;
    attachment.remove();
    this.attachments = this.attachments.filter((held) => held !== attachment);
  }

  dismissNotice(): void {
    this.notice = null;
  }

  async settled(): Promise<void> {
    while (this.uploading) {
      await Promise.all(this.attachments.map((attachment) => attachment.done));
    }
  }

  take(): StagedAttachment[] {
    const taken = this.attachments;
    this.attachments = [];
    this.notice = null;
    return taken;
  }

  restore(returned: readonly StagedAttachment[]): void {
    const back = returned.filter((attachment) => !this.attachments.includes(attachment));
    if (back.length > 0) this.attachments = [...back, ...this.attachments];
  }

  clear(): void {
    for (const attachment of this.attachments) attachment.remove();
    this.attachments = [];
    this.notice = null;
  }
}
