// One open file: its text on disk and in the editor, saving, the save
// conflict, and keeping up with changes made elsewhere. The file panel shows
// one, and the tree tells it when the file it shows is renamed or deleted.

import {
  ApiError,
  fetchWorkspaceFile,
  putWorkspaceFile,
  validateWorkspaceFile,
  workspaceConflictFromApiError,
  type WorkspaceFileRead,
} from "../../lib/api";
import {
  configCoordinator,
  configFileName,
  type ConfigChoice,
  type ConfigFile,
} from "../../lib/config-coordinator";
import { userErrorMessage } from "../../lib/errors";
import { toast } from "../../lib/toast.svelte";
import type { Diagnostic, ValidateResponse } from "../../lib/types";
import { configFileAt, fileName, sameSource, type FileSource } from "./file-source";

export type FileStatus = "loading" | "ready" | "missing" | "error";

/** What the user chose when a save found the file changed: theirs, the disk's, or neither for now. */
export type ConflictAnswer = ConfigChoice | "cancel";

function isNotFound(err: unknown): boolean {
  return err instanceof ApiError && err.status === 404;
}

/** A raw config save's verdict as diagnostics: the server's, or its one error. */
function verdictDiagnostics(result: ValidateResponse): Diagnostic[] {
  if (result.diagnostics !== undefined && result.diagnostics.length > 0) return result.diagnostics;
  return result.error === undefined ? [] : [{ severity: "error", message: result.error }];
}

export class FileBuffer {
  path = $state("");
  status = $state<FileStatus>("loading");
  /** The file's text as last read or saved. */
  saved = $state("");
  /** The text in the editor. */
  text = $state("");
  diagnostics = $state<Diagnostic[]>([]);
  /** Why the file couldn't be read (while `error`), or checked again (while `ready`), in plain words. */
  error = $state("");
  saving = $state(false);
  /** The file changed, or went away, on disk while it had unsaved edits. */
  changedOnDisk = $state<"changed" | "removed" | null>(null);
  /** A save found the file changed since it was read, and waits for the user's answer. */
  conflict = $state<((answer: ConflictAnswer) => void) | null>(null);

  readonly dirty = $derived(this.status === "ready" && this.text !== this.saved);
  readonly name = $derived(fileName(this.path));

  /** The version the file was read or saved at, sent back as `If-Match`. */
  private version: string | null = null;
  /** Counts reads, so one that a later read overtook is dropped. */
  private reads = 0;
  /** Marks the coordinator writes this buffer made, so it doesn't hear its own. */
  readonly writer = Symbol("file editor");

  /** It holds no file until `open`. */
  constructor(readonly source: FileSource) {}

  /** The config file this is, which saves through the config write coordinator, or null. */
  get configFile(): ConfigFile | null {
    return configFileAt(this.source, this.path);
  }

  /** Whether this buffer holds `path` in `source`. */
  shows(source: FileSource, path: string): boolean {
    return sameSource(source, this.source) && path === this.path;
  }

  /** Show `path`, read from disk, dropping the edits to the file shown. */
  async open(path: string): Promise<void> {
    this.path = path;
    this.status = "loading";
    this.text = "";
    this.saved = "";
    this.diagnostics = [];
    this.changedOnDisk = null;
    const read = ++this.reads;
    try {
      const file = await this.read();
      if (read === this.reads) this.adopt(file);
    } catch (err) {
      if (read !== this.reads) return;
      if (isNotFound(err)) {
        this.status = "missing";
        return;
      }
      this.status = "error";
      this.error = userErrorMessage(err, { action: `Couldn't open ${this.name}.` });
    }
  }

  /**
   * Catch up with the disk after a change made elsewhere: take the new text
   * when there are no edits, and otherwise keep the edits and say the file
   * changed under them. A change that leaves the text as saved (this
   * buffer's own write coming back) only moves the version on.
   */
  async refresh(): Promise<void> {
    if (this.status === "loading" || this.saving) return;
    const read = ++this.reads;
    try {
      const file = await this.read();
      if (read !== this.reads) return;
      this.error = "";
      if (this.status === "ready" && file.content === this.saved) {
        this.version = file.version;
      } else if (this.dirty) {
        this.changedOnDisk = "changed";
      } else {
        this.adopt(file);
      }
    } catch (err) {
      if (read !== this.reads) return;
      if (!isNotFound(err)) {
        // The file shown stays as it was, with a note that it may be out of date.
        this.error = userErrorMessage(err, {
          action: `Couldn't check ${this.name} for changes made elsewhere.`,
        });
        return;
      }
      if (this.dirty) this.changedOnDisk = "removed";
      else this.status = "missing";
    }
  }

  /** The file was renamed or moved to `path`; the edits stay. */
  moved(path: string): void {
    this.path = path;
  }

  /** The file was deleted from the tree: there's nothing left to edit. */
  removed(): void {
    this.reads++;
    this.status = "missing";
    this.changedOnDisk = null;
  }

  discard(): void {
    this.text = this.saved;
    this.changedOnDisk = null;
  }

  /** Diagnostics for the text in the editor, as if it were saved. */
  async validate(): Promise<void> {
    const { path, text } = this;
    const found = await validateWorkspaceFile(this.source.agent, path, text, this.source.scope);
    if (path === this.path && text === this.text) this.diagnostics = found;
  }

  async save(): Promise<void> {
    if (!this.dirty || this.saving) return;
    this.saving = true;
    const text = this.text;
    try {
      const config = this.configFile;
      if (config === null) await this.saveFile(text, this.version);
      else await this.saveConfig(config, text);
    } catch (err) {
      toast.error(userErrorMessage(err, { action: `Couldn't save ${this.name}.` }));
    } finally {
      this.saving = false;
    }
  }

  /** The user's answer to the save conflict. */
  answer(answer: ConflictAnswer): void {
    this.conflict?.(answer);
  }

  private read(): Promise<WorkspaceFileRead> {
    return fetchWorkspaceFile(this.source.agent, this.path, this.source.scope);
  }

  private adopt(file: WorkspaceFileRead): void {
    this.saved = file.content;
    this.text = file.content;
    this.version = file.version;
    this.status = "ready";
    this.changedOnDisk = null;
    this.error = "";
  }

  private ask(): Promise<ConflictAnswer> {
    return new Promise((resolve) => {
      this.conflict = (answer) => {
        this.conflict = null;
        resolve(answer);
      };
    });
  }

  /** Write with `If-Match`, so a change made since the file was read is never overwritten unasked. */
  private async saveFile(text: string, version: string | null): Promise<void> {
    try {
      const { source } = this;
      const response = await putWorkspaceFile(source.agent, this.path, text, version, source.scope);
      this.wrote(text, response.version, response.diagnostics ?? []);
    } catch (err) {
      const conflict = workspaceConflictFromApiError(err);
      if (conflict === null) throw err;
      const answer = await this.ask();
      if (answer === "use-disk") await this.open(this.path);
      // Overwriting goes through `If-Match` again, so a further change asks again.
      else if (answer === "keep-mine") await this.saveFile(text, conflict.currentVersion);
    }
  }

  private async saveConfig(file: ConfigFile, text: string): Promise<void> {
    const asked: { answer: ConflictAnswer | null } = { answer: null };
    const outcome = await configCoordinator.save(file, {
      baseline: this.saved,
      edit: { text },
      source: this.writer,
      choose: async () => {
        asked.answer = await this.ask();
        return asked.answer === "keep-mine" ? "keep-mine" : "use-disk";
      },
    });
    if (outcome.kind === "used-disk") {
      if (asked.answer === "use-disk") this.adopt({ content: outcome.raw, version: "" });
      return;
    }
    if (!outcome.written && !outcome.result.valid) {
      this.diagnostics = verdictDiagnostics(outcome.result);
      toast.error(`${configFileName(file)} wasn't saved. Fix the problems noted below, then save.`);
      return;
    }
    this.wrote(outcome.raw ?? text, null, verdictDiagnostics(outcome.result));
  }

  private wrote(text: string, version: string | null, diagnostics: Diagnostic[]): void {
    this.saved = text;
    this.version = version;
    this.diagnostics = diagnostics;
    this.changedOnDisk = null;
    toast.success(
      diagnostics.length > 0
        ? `Saved ${this.name}, with problems noted below.`
        : `Saved ${this.name}.`,
    );
  }
}

/**
 * The file the context panel shows, if any. The tree tells it when that file
 * is renamed, moved, deleted or restored, so its edits follow the file.
 */
export const panelFile: { shown: FileBuffer | null } = { shown: null };
