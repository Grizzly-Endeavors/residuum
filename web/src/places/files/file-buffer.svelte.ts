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

export type ConflictAnswer = ConfigChoice | "cancel";

function isNotFound(err: unknown): boolean {
  return err instanceof ApiError && err.status === 404;
}

function verdictDiagnostics(result: ValidateResponse): Diagnostic[] {
  if (result.diagnostics !== undefined && result.diagnostics.length > 0) return result.diagnostics;
  return result.error === undefined ? [] : [{ severity: "error", message: result.error }];
}

export class FileBuffer {
  path = $state("");
  status = $state<FileStatus>("loading");
  saved = $state("");
  text = $state("");
  diagnostics = $state<Diagnostic[]>([]);
  error = $state("");
  saving = $state(false);
  changedOnDisk = $state<"changed" | "removed" | null>(null);
  conflict = $state<((answer: ConflictAnswer) => void) | null>(null);
  editing = $state(false);

  readonly dirty = $derived(this.status === "ready" && this.text !== this.saved);
  readonly name = $derived(fileName(this.path));

  private version: string | null = null;
  private reads = 0;
  readonly writer = Symbol("file editor");

  constructor(readonly source: FileSource) {}

  get configFile(): ConfigFile | null {
    return configFileAt(this.source, this.path);
  }

  shows(source: FileSource, path: string): boolean {
    return sameSource(source, this.source) && path === this.path;
  }

  async open(path: string): Promise<void> {
    if (path !== this.path) this.editing = false;
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
        this.error = userErrorMessage(err, {
          action: `Couldn't check ${this.name} for changes made elsewhere.`,
        });
        return;
      }
      if (this.dirty) this.changedOnDisk = "removed";
      else this.status = "missing";
    }
  }

  moved(path: string): void {
    this.path = path;
  }

  removed(): void {
    this.reads++;
    this.status = "missing";
    this.changedOnDisk = null;
  }

  discard(): void {
    this.text = this.saved;
    this.changedOnDisk = null;
  }

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
        ? `Saved ${this.name}, with problems noted in the panel.`
        : `Saved ${this.name}.`,
    );
  }
}

export const panelFile: { shown: FileBuffer | null } = { shown: null };
