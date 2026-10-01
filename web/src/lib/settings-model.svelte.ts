// ── The settings model (Svelte 5 runes) ──────────────────────────────
//
// What the settings modal edits (design §8), as logic with no UI. One scope
// holds the files one page of settings edits: an agent's `config.toml`,
// `providers.toml` and `mcp.json`, or the hub's `config.toml` for All agents.
// A scope never writes another scope's files.
//
// Each file has a baseline, its text as this model last loaded or saved it,
// and a form: a copy of the baseline parsed into the shapes the sections bind
// to. What the user has changed is the difference between the two, which
// `settings-toml.ts` turns into the PATCH diff the server takes. Those are the
// staged changes. They stay in the scope while the modal is closed or another
// scope is open, and Save and Discard act on the whole scope.
//
// Everything is written through the config write coordinator
// (`config-coordinator.ts`). Immediate actions (secrets, agent keys, Cloud,
// updates, raw saves, History) have their own endpoints and no state here.
//
// A section reads and edits a scope's forms in place:
//
//     const scope = settingsModel.agent("scout");   // in a script, not a template
//     await scope.load();
//     <input bind:value={scope.config.timeout_secs} />
//     scope.providers.push({ ... });                // staged; Discard takes it back
//
// Read the form through the scope each time: a reload, or an external change to
// a file with no staged changes, gives the scope new form objects.

import { parse as parseToml } from "smol-toml";
import { SvelteMap } from "svelte/reactivity";
import { storeSecret as storeSecretViaApi } from "./api";
import {
  agentConfigFile,
  checkpointLocationOf,
  configCoordinator,
  configFileName,
  HUB_CONFIG_FILE,
  type ConfigChooser,
  type ConfigCoordinator,
  type ConfigFile,
} from "./config-coordinator";
import { userErrorMessage, userErrorReason } from "./errors";
import {
  fieldRefKey,
  keyPathOf,
  placeDiagnostic,
  type FieldFile,
  type FieldRef,
  type PlacedDiagnostic,
} from "./settings-fields";
import { ALL_SCOPE, scopeKind, type ScopeKind, type SectionId } from "./settings-sections";
import { isStoredReference } from "./secrets";
import {
  configFieldOwner,
  diffConfigFields,
  diffMcpServers,
  diffProviders,
  jsonEqual,
  parseConfigToml,
  parseMcpJson,
  parseProvidersToml,
  splitConfigPatch,
  defaultConfigFields,
  type ConfigFields,
  type ProvidersFormState,
} from "./settings-toml";
import type {
  Diagnostic,
  McpServerEntry,
  RepoKind,
  SettingsModelAssignments,
  SettingsProviderEntry,
  ValidateResponse,
} from "./types";

// ── What the model asks of the outside ───────────────────────────────

/** The services a scope uses. Tests replace them. */
export interface SettingsDeps {
  coordinator: ConfigCoordinator;
  /** Store a typed secret and return the `secret:<name>` reference that goes in the file. */
  storeSecret: (name: string, value: string) => Promise<{ reference: string }>;
}

const APP_DEPS: SettingsDeps = {
  coordinator: configCoordinator,
  storeSecret: storeSecretViaApi,
};

// ── Forms ─────────────────────────────────────────────────────────────

/** How one file's text becomes a form and a form's changes become a patch. */
interface FormCodec<T> {
  parse: (raw: string) => T;
  /** What changed from `baseline` to `current`, as a PATCH diff. Empty when nothing did. */
  diff: (baseline: T, current: T) => Record<string, unknown>;
}

const AGENT_CONFIG_FORM: FormCodec<ConfigFields> = {
  parse: (raw) => parseConfigToml(raw),
  diff: (baseline, current) => splitConfigPatch(diffConfigFields(baseline, current)).agent,
};

const HUB_CONFIG_FORM: FormCodec<ConfigFields> = {
  parse: (raw) => parseConfigToml("", raw),
  diff: (baseline, current) => splitConfigPatch(diffConfigFields(baseline, current)).hub,
};

const PROVIDERS_FORM: FormCodec<ProvidersFormState> = {
  parse: parseProvidersToml,
  diff: (baseline, current) =>
    diffProviders(baseline.providers, current.providers, baseline.models, current.models),
};

const MCP_FORM: FormCodec<McpServerEntry[]> = { parse: parseMcpJson, diff: diffMcpServers };

/**
 * A deep copy of form state that shares nothing with it. `$state.snapshot`
 * unwraps the proxy; a runtime that doesn't proxy returns its argument, so the
 * copy is made here.
 */
function clone<T>(value: T): T {
  const plain: unknown = $state.snapshot(value);
  return structuredClone(plain) as T;
}

/** Whether a file's text parses. A form shows an empty file for one that doesn't. */
function parses(file: ConfigFile, raw: string): boolean {
  if (raw.trim() === "") return true;
  try {
    if (file.kind === "agent" && file.name === "mcp") JSON.parse(raw);
    else parseToml(raw);
    return true;
  } catch {
    return false;
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isNamedList(value: unknown): value is { name: string }[] {
  return (
    Array.isArray(value) &&
    value.every((entry) => isRecord(entry) && typeof entry.name === "string")
  );
}

/**
 * Three-way merge of form values. What the user changed between `base` and
 * `mine` stays; everything else takes `theirs`, the file as it is now.
 * Collections of named entries merge entry by entry.
 */
function rebase(base: unknown, mine: unknown, theirs: unknown): unknown {
  if (jsonEqual(base, mine)) return theirs;
  if (isNamedList(base) && isNamedList(mine) && isNamedList(theirs)) {
    const names = new Set([...theirs, ...mine, ...base].map((entry) => entry.name));
    return [...names].flatMap((name) => {
      const find = (list: { name: string }[]): unknown => list.find((entry) => entry.name === name);
      const merged = rebase(find(base), find(mine), find(theirs));
      return merged === undefined ? [] : [merged];
    });
  }
  if (isRecord(base) && isRecord(mine) && isRecord(theirs)) {
    const keys = new Set([...Object.keys(base), ...Object.keys(mine), ...Object.keys(theirs)]);
    return Object.fromEntries(
      [...keys].map((key) => [key, rebase(base[key], mine[key], theirs[key])]),
    );
  }
  return mine;
}

// ── One file ──────────────────────────────────────────────────────────

/** Which editor a file's staged state keeps read-only: the raw editor while the form has changes, the form while the raw editor does. */
export type FileLock = "form" | "raw";

/** What a scope uses of a file, whatever its form. */
export interface StagedFile {
  readonly name: FieldFile;
  readonly file: ConfigFile;
  /** The file's text as last loaded or saved here. Null until the first load. */
  readonly raw: string | null;
  /** The form holds changes the file doesn't. */
  readonly dirty: boolean;
  /** What a save would send: the form's changes as a PATCH diff. */
  readonly patch: Record<string, unknown>;
  /** The file changed on disk while the form held changes, so a save may meet a conflict. */
  readonly changedOnDisk: boolean;
  /** The file's text doesn't parse, so the form shows it empty. The raw editor is where to fix it. */
  readonly unreadable: boolean;
  /** Why the last read of the file failed, in plain language. */
  readonly loadError: string | null;
  /** What is in the raw editor when it differs from `raw`. */
  readonly rawDraft: string | null;
  /** The file's text when the raw draft began: what a save of the draft checks the file against. */
  readonly rawDraftBase: string | null;
  /** Which editor is read-only because of the other. */
  readonly lockedBy: FileLock | null;
  /** The raw editor holds `text`. Null, or the file's own text, clears the draft. */
  setRawDraft: (text: string | null) => void;
  /** Drop the form's changes. */
  discard: () => void;
  /** The file reads as `raw` now: the baseline and form become it. */
  refresh: (raw: string) => void;
  /** The file changed on disk while the form holds changes. */
  markChangedOnDisk: () => void;
  /** A read of the file failed. */
  fail: (message: string) => void;
  /**
   * A save is about to write the form as it is now. `done` takes the file's
   * text afterwards: it becomes the baseline, and anything edited since this
   * call stays staged.
   */
  beginSave: () => { patch: Record<string, unknown>; done: (raw: string) => void };
}

/** One config file: its baseline, its form, and what is staged between them. */
export class FileState<T extends object> implements StagedFile {
  raw = $state<string | null>(null);
  /** The file parsed as of `raw`. */
  baseline = $state.raw() as T;
  /** What the sections bind to. */
  form = $state() as T;
  rawDraft = $state<string | null>(null);
  rawDraftBase = $state<string | null>(null);
  changedOnDisk = $state(false);
  unreadable = $state(false);
  loadError = $state<string | null>(null);

  patch = $derived(this.currentPatch());
  dirty = $derived(Object.keys(this.patch).length > 0);
  lockedBy = $derived<FileLock | null>(this.lock());

  constructor(
    readonly name: FieldFile,
    readonly file: ConfigFile,
    private readonly codec: FormCodec<T>,
  ) {
    this.baseline = codec.parse("");
    this.form = clone(this.baseline);
  }

  private currentPatch(): Record<string, unknown> {
    return this.codec.diff(this.baseline, this.form);
  }

  private lock(): FileLock | null {
    if (this.dirty) return "form";
    return this.rawDraft !== null && this.rawDraft !== this.raw ? "raw" : null;
  }

  setRawDraft(text: string | null): void {
    const draft = text === this.raw ? null : text;
    // A file can change on disk while a draft is open; the draft's base stays
    // what the user started from, so the save's re-read finds that change.
    if (draft === null) this.rawDraftBase = null;
    else if (this.rawDraft === null) this.rawDraftBase = this.raw;
    this.rawDraft = draft;
  }

  discard(): void {
    this.form = clone(this.baseline);
    this.changedOnDisk = false;
  }

  refresh(raw: string): void {
    this.adopt(raw, this.codec.parse(raw), null);
  }

  markChangedOnDisk(): void {
    this.changedOnDisk = true;
  }

  fail(message: string): void {
    this.loadError = message;
  }

  beginSave(): { patch: Record<string, unknown>; done: (raw: string) => void } {
    const written = clone(this.form);
    return {
      patch: this.codec.diff(this.baseline, written),
      done: (raw) => {
        const now = this.codec.parse(raw);
        this.adopt(raw, now, rebase(written, clone(this.form), now) as T);
      },
    };
  }

  /** Take `raw` as the baseline, with `parsed` its parse, and `form` as the form (the baseline's copy when null). */
  private adopt(raw: string, parsed: T, form: T | null): void {
    this.raw = raw;
    this.baseline = parsed;
    this.form = clone(form ?? parsed);
    this.changedOnDisk = false;
    this.loadError = null;
    this.unreadable = !parses(this.file, raw);
    if (this.rawDraft === raw) this.setRawDraft(null);
  }
}

// ── Results ───────────────────────────────────────────────────────────

/** What a save did to one file. */
export type FileSaveStatus =
  | "saved"
  /** The form's changes came to nothing for this file: a typed secret that stored under the name already in the file. */
  | "unchanged"
  | "failed"
  /** Not tried, because a file it depends on didn't save. */
  | "skipped"
  /** The file changed on disk under the form's changes, and the user chose what was on disk. */
  | "used-disk";

export interface FileSaveResult {
  file: FieldFile;
  /** The file's name for a message: `providers.toml`. */
  label: string;
  status: FileSaveStatus;
  /** A plain-language sentence for `failed` and `skipped`. */
  message: string | null;
  /** The checkpoint taken just before the write, for Undo. */
  checkpointId: string | null;
}

/** The checkpoint a save took of a file just before writing it, which Undo restores the file from. */
export interface SavedCheckpoint {
  file: FieldFile;
  repo: RepoKind;
  /** The file's path in `repo`. */
  path: string;
  id: string;
  /** The file's text as the save left it. Undo skips a file that reads otherwise now. */
  text: string;
}

export interface SaveResult {
  /** `partial` when some files saved and others didn't. `nothing` when nothing was staged. */
  outcome: "saved" | "partial" | "failed" | "nothing";
  /** Each file the save tried, in the order it wrote them. */
  files: FileSaveResult[];
  /** The checkpoints the save took, in the order it took them. */
  checkpoints: SavedCheckpoint[];
  /** Names of the secrets stored from typed values. */
  storedSecrets: string[];
  /** What happened, in plain language, for the save bar. */
  message: string;
}

/** What Undo did to one checkpoint's file. */
export interface UndoFileResult {
  file: FieldFile;
  label: string;
  /** Paths put back as they were. */
  reverted: string[];
  /** Paths left alone because they changed again since. */
  skipped: string[];
  /** Why the file couldn't be restored, in plain language. Null when it was. */
  error: string | null;
}

export interface UndoResult {
  /** Each file, in the order restored: the reverse of the save. */
  files: UndoFileResult[];
  /** Files that couldn't be restored. Undo is still offered for them. */
  failed: FieldFile[];
  message: string;
}

/**
 * What a chooser throws when the user closes the question about a file that
 * changed on disk without answering it. That file isn't saved and keeps its
 * staged changes.
 */
export class ConflictUnanswered extends Error {}

// ── Messages ──────────────────────────────────────────────────────────

/** A file's name in a message. */
function fileLabel(file: ConfigFile): string {
  return file.kind === "hub" ? "the install-wide config.toml" : configFileName(file);
}

function joinNames(names: readonly string[]): string {
  if (names.length <= 2) return names.join(" and ");
  return `${names.slice(0, -1).join(", ")} and ${names[names.length - 1] ?? ""}`;
}

function sentence(text: string): string {
  return /[.!?]$/.test(text) ? text : `${text}.`;
}

function summarizeSave(
  files: readonly FileSaveResult[],
  storedSecrets: readonly string[],
): Pick<SaveResult, "outcome" | "message"> {
  const saved = files.filter((f) => f.status === "saved").map((f) => f.label);
  const onDisk = files.filter((f) => f.status === "used-disk").map((f) => f.label);
  const problems = files.filter((f) => f.status === "failed" || f.status === "skipped");
  const parts: string[] = [];
  if (saved.length > 0) parts.push(`Saved ${joinNames(saved)}.`);
  if (onDisk.length > 0) parts.push(`Kept ${joinNames(onDisk)} as on disk, without your changes.`);
  else if (saved.length === 0 && problems.length === 0)
    parts.push(storedSecrets.length > 0 ? "Saved the key." : "Saved.");
  for (const problem of problems) parts.push(problem.message ?? `${problem.label} wasn't saved.`);
  if (problems.length > 0) parts.push("Changes that weren't saved are still staged.");
  let outcome: SaveResult["outcome"] = "saved";
  if (problems.length > 0) outcome = saved.length > 0 ? "partial" : "failed";
  return { outcome, message: parts.join(" ") };
}

function summarizeUndo(files: readonly UndoFileResult[]): string {
  const parts: string[] = [];
  const reverted = files.filter((f) => f.error === null && f.reverted.length > 0);
  if (reverted.length > 0) parts.push(`Reverted ${joinNames(reverted.map((f) => f.label))}.`);
  for (const f of files) {
    if (f.skipped.length > 0) {
      parts.push(`Skipped ${f.skipped.join(", ")} in ${f.label}: changed again since.`);
    }
  }
  const failed = files.filter((f) => f.error !== null);
  if (failed.length > 0) parts.push(`Couldn't restore ${joinNames(failed.map((f) => f.label))}.`);
  return parts.length > 0 ? parts.join(" ") : "Nothing needed undoing.";
}

/** A save's or check's problems; a refusal that lists none is one problem, its error. */
export function diagnosticsOf(result: ValidateResponse): Diagnostic[] {
  const found = result.diagnostics ?? [];
  if (found.length > 0 || result.valid || result.error === undefined) return found;
  return [{ severity: "error", message: result.error }];
}

// ── Typed secrets ─────────────────────────────────────────────────────

/** A credential the user typed, which is stored as a secret and replaced by its reference before the file is written. */
interface TypedSecret {
  name: string;
  value: string;
  apply: (reference: string) => void;
}

/** Whether `value` is a literal the user entered, rather than empty, unchanged or already a reference. */
function isTyped(value: string, before: string | undefined): boolean {
  return value !== "" && value !== before && !isStoredReference(value);
}

/** The config fields that hold a credential, and the name each is stored under. */
const SECRET_FIELDS = [
  { field: "discord_token", name: "discord" },
  { field: "telegram_token", name: "telegram" },
  { field: "teams_app_password", name: "teams" },
  { field: "cloud_token", name: "cloud_token" },
  { field: "ws_brave_api_key", name: "ws_brave" },
  { field: "ws_tavily_api_key", name: "ws_tavily" },
  { field: "ws_ollama_api_key", name: "ws_ollama" },
] as const;

/** The typed credentials among the config fields of `owner`'s file. */
function typedConfigSecrets(file: FileState<ConfigFields>, owner: "hub" | "agent"): TypedSecret[] {
  const { form, baseline } = file;
  const typed: TypedSecret[] = [];
  for (const { field, name } of SECRET_FIELDS) {
    if (configFieldOwner(keyPathOf({ kind: "config", field })) !== owner) continue;
    if (isTyped(form[field], baseline[field])) {
      typed.push({
        name,
        value: form[field],
        apply: (reference) => {
          form[field] = reference;
        },
      });
    }
  }
  if (owner === "hub") return typed;
  for (const webhook of form.webhooks) {
    const entry = webhook.name.trim();
    const before = baseline.webhooks.find((candidate) => candidate.name.trim() === entry);
    if (entry !== "" && isTyped(webhook.secret, before?.secret)) {
      typed.push({
        name: `webhook_${entry}`,
        value: webhook.secret,
        apply: (reference) => {
          webhook.secret = reference;
        },
      });
    }
  }
  return typed;
}

// ── A scope ───────────────────────────────────────────────────────────

const NOTHING_STAGED: SaveResult = {
  outcome: "nothing",
  files: [],
  checkpoints: [],
  storedSecrets: [],
  message: "There is nothing to save.",
};

/** One page of settings: the files it edits, what is staged in them, and how they save. */
export abstract class ScopeModel {
  abstract readonly kind: ScopeKind;
  /** The agent whose files these are. Null for All agents. */
  abstract readonly agent: string | null;
  /** The files this scope edits, in the order a save writes them. */
  abstract readonly files: readonly StagedFile[];

  /** A save is writing. */
  saving = $state(false);
  loading = $state(false);
  /** What the last save did, for the save bar and its Undo. */
  lastResult = $state.raw<SaveResult | null>(null);
  /** Problems the last saves found, in the files they were found in, placed on fields where they name a key. */
  diagnostics = $state.raw<readonly PlacedDiagnostic[]>([]);

  /** Any file holds staged changes. */
  get dirty(): boolean {
    return this.files.some((state) => state.dirty);
  }

  /** Work a page reload would lose: staged changes, or a raw editor's unsaved edits. */
  get unsaved(): boolean {
    return this.dirty || this.files.some((state) => state.rawDraft !== null);
  }

  /** The last save took checkpoints that haven't been undone. */
  get undoable(): boolean {
    return (this.lastResult?.checkpoints.length ?? 0) > 0;
  }

  /** Why a file couldn't be read, if one couldn't. */
  get loadError(): string | null {
    return this.files.find((state) => state.loadError !== null)?.loadError ?? null;
  }

  /** Every file has been read, or failed to be. Until then the forms are empty, so nothing should show or edit them. */
  get loaded(): boolean {
    return this.files.every((state) => state.raw !== null || state.loadError !== null);
  }

  private running: Promise<SaveResult> | null = null;
  private stops: (() => void)[] = [];
  /** Names this scope's writes in the coordinator's notifications, so it skips the ones it made. */
  private readonly source = Symbol("settings-scope");

  protected constructor(
    readonly id: string,
    protected readonly deps: SettingsDeps,
  ) {}

  /** The credentials typed into the forms, to store as secrets before a save. */
  protected abstract typedSecrets(): TypedSecret[];

  /** Anything besides the files that loads with the scope. */
  protected loadOther(): Promise<void> {
    return Promise.resolve();
  }

  /** The files whose changes this scope follows, with what to do about each. */
  protected watches(): { file: ConfigFile; changed: () => Promise<void> }[] {
    return this.files.map((state) => ({ file: state.file, changed: () => this.read(state) }));
  }

  // ── Loading ─────────────────────────────────────────────────────────

  /**
   * Make the files current: read each from disk. A file with staged changes
   * keeps them, and its baseline, so the coordinator's re-read before a save
   * finds what changed. Also starts following changes made elsewhere.
   */
  async load(): Promise<void> {
    this.loading = true;
    try {
      await Promise.all([...this.files.map((state) => this.read(state)), this.loadOther()]);
      this.follow();
    } finally {
      this.loading = false;
    }
  }

  /** Drop the staged changes and the raw editors' edits, and read every file from disk again. */
  async reload(): Promise<void> {
    this.discard();
    for (const state of this.files) state.setRawDraft(null);
    await this.load();
  }

  /** Read a file. It becomes the baseline when nothing is staged in it; otherwise it is marked as changed on disk. */
  private async read(state: StagedFile): Promise<void> {
    try {
      const raw = await this.deps.coordinator.read(state.file);
      if (raw === state.raw && state.loadError === null) return;
      if (state.dirty) state.markChangedOnDisk();
      else state.refresh(raw);
    } catch (err) {
      state.fail(userErrorMessage(err, { action: `Couldn't read ${fileLabel(state.file)}.` }));
    }
  }

  private follow(): void {
    if (this.stops.length > 0) return;
    for (const { file, changed } of this.watches()) {
      this.stops.push(
        this.deps.coordinator.subscribe(file, (change) => {
          if (change.source !== this.source) void changed();
        }),
      );
    }
  }

  /** Stop following the files. */
  dispose(): void {
    for (const stop of this.stops) stop();
    this.stops = [];
  }

  // ── Staging ─────────────────────────────────────────────────────────

  /** Drop every staged change in the scope, and the problems the last save found. */
  discard(): void {
    for (const state of this.files) state.discard();
    this.diagnostics = [];
  }

  /** The file called `name`, if this scope edits one. */
  file(name: FieldFile): StagedFile | undefined {
    return this.files.find((state) => state.name === name);
  }

  // ── Diagnostics ─────────────────────────────────────────────────────

  /** The problems that name this field's key. An entry's own problems are on the ref with no `field`. */
  fieldDiagnostics(ref: FieldRef): Diagnostic[] {
    const key = fieldRefKey(ref);
    return this.diagnostics
      .filter((placed) => placed.field !== null && fieldRefKey(placed.field) === key)
      .map((placed) => placed.diagnostic);
  }

  /** The problems for the top of a section: those whose key it edits but no field holds, and those with no key path. */
  sectionDiagnostics(section: SectionId): Diagnostic[] {
    return this.diagnostics
      .filter(
        (placed) =>
          placed.field === null && (placed.section === null || placed.section === section),
      )
      .map((placed) => placed.diagnostic);
  }

  private place(state: StagedFile, found: readonly Diagnostic[]): void {
    this.diagnostics = [
      ...this.diagnostics.filter((placed) => placed.file !== state.name),
      ...found.map((diagnostic) => placeDiagnostic(this.kind, state.name, diagnostic)),
    ];
  }

  // ── Saving ──────────────────────────────────────────────────────────

  /**
   * Save every file with staged changes, as a diff through the coordinator:
   * providers first, then config, then MCP servers, because config validation
   * reads providers from disk. `choose` settles a file that changed on disk
   * under keys being saved. A file that fails keeps its staged changes, and
   * the rest still save, except that `config.toml` waits on `providers.toml`.
   * Typed credentials are stored as secrets first. A second call while one runs
   * gets that save's result.
   */
  save(choose: ConfigChooser): Promise<SaveResult> {
    this.running ??= this.runSave(choose).finally(() => {
      this.running = null;
    });
    return this.running;
  }

  private async runSave(choose: ConfigChooser): Promise<SaveResult> {
    const staged = this.files.filter((state) => state.dirty);
    if (staged.length === 0) return NOTHING_STAGED;
    this.saving = true;
    try {
      const storedSecrets: string[] = [];
      try {
        for (const secret of this.typedSecrets()) {
          const { reference } = await this.deps.storeSecret(secret.name, secret.value);
          secret.apply(reference);
          storedSecrets.push(secret.name);
        }
      } catch (err) {
        const reason = userErrorReason(err, { action: "Couldn't store a key you typed." });
        return this.record({
          outcome: "failed",
          files: [],
          checkpoints: [],
          storedSecrets,
          message: `Nothing was saved. Couldn't store a key you typed: ${reason}`,
        });
      }

      const files: FileSaveResult[] = [];
      const checkpoints: SavedCheckpoint[] = [];
      let providersFailed = false;
      for (const state of staged) {
        const result =
          state.name === "config" && providersFailed
            ? this.skipped(state)
            : await this.writeFile(state, choose);
        files.push(result);
        if (result.checkpointId !== null) {
          checkpoints.push({
            file: state.name,
            ...checkpointLocationOf(state.file),
            id: result.checkpointId,
            text: state.raw ?? "",
          });
        }
        if (state.name === "providers" && result.status === "failed") providersFailed = true;
      }
      return this.record({
        files,
        checkpoints,
        storedSecrets,
        ...summarizeSave(files, storedSecrets),
      });
    } finally {
      this.saving = false;
    }
  }

  private record(result: SaveResult): SaveResult {
    this.lastResult = result;
    return result;
  }

  private skipped(state: StagedFile): FileSaveResult {
    const label = fileLabel(state.file);
    return {
      file: state.name,
      label,
      status: "skipped",
      message: `${label} wasn't tried, because it is checked against providers.toml and that didn't save.`,
      checkpointId: null,
    };
  }

  /** Send one file's diff. */
  private async writeFile(state: StagedFile, choose: ConfigChooser): Promise<FileSaveResult> {
    const label = fileLabel(state.file);
    const outcome = (
      status: FileSaveStatus,
      message: string | null = null,
      checkpointId: string | null = null,
    ): FileSaveResult => ({ file: state.name, label, status, message, checkpointId });

    if (state.raw === null) {
      return outcome("failed", `Couldn't save ${label}: its settings haven't loaded yet.`);
    }
    const { patch, done } = state.beginSave();
    if (Object.keys(patch).length === 0) return outcome("unchanged");
    try {
      const saved = await this.deps.coordinator.save(state.file, {
        baseline: state.raw,
        edit: { patch },
        choose,
        source: this.source,
      });
      if (saved.kind === "used-disk") {
        state.refresh(saved.raw);
        this.place(state, []);
        return outcome("used-disk");
      }
      this.place(state, diagnosticsOf(saved.result));
      if (!saved.result.valid) {
        const reason = sentence(saved.result.error ?? "the server turned the change down");
        return outcome("failed", `Couldn't save ${label}: ${reason}`);
      }
      await this.adopt(state, done, saved.raw);
      return outcome("saved", null, saved.result.checkpoint_id ?? null);
    } catch (err) {
      if (err instanceof ConflictUnanswered) {
        return outcome(
          "failed",
          `${label} wasn't saved, because it changed on disk and you closed the question about it.`,
        );
      }
      return outcome("failed", userErrorMessage(err, { action: `Couldn't save ${label}.` }));
    }
  }

  /** Take the file's text after a write as its baseline, reading it back when the save couldn't. */
  private async adopt(
    state: StagedFile,
    done: (raw: string) => void,
    raw: string | null,
  ): Promise<void> {
    try {
      done(raw ?? (await this.deps.coordinator.read(state.file)));
    } catch (err) {
      state.fail(userErrorMessage(err, { action: `Couldn't read ${fileLabel(state.file)} back.` }));
    }
  }

  // ── Undo ────────────────────────────────────────────────────────────

  /**
   * Put each file the last save wrote back as it was, newest first, from the
   * checkpoint the save took just before writing it. A file that changed again
   * since is skipped, so that change isn't lost. A file that can't be restored
   * is named, and its checkpoint stays for another try.
   */
  async undo(): Promise<UndoResult> {
    const checkpoints = this.lastResult?.checkpoints ?? [];
    const files: UndoFileResult[] = [];
    const remaining: SavedCheckpoint[] = [];
    for (const checkpoint of [...checkpoints].reverse()) {
      const state = this.file(checkpoint.file);
      const label = state === undefined ? checkpoint.file : fileLabel(state.file);
      const { id, repo, path } = checkpoint;
      try {
        const now = state === undefined ? null : await this.deps.coordinator.read(state.file);
        const changed = now !== checkpoint.text;
        if (!changed) await this.deps.coordinator.restore(this.agent, id, repo, path);
        files.push({
          file: checkpoint.file,
          label,
          reverted: changed ? [] : [path],
          skipped: changed ? [path] : [],
          error: null,
        });
      } catch (err) {
        const error = userErrorMessage(err, { action: `Couldn't restore ${label}.` });
        files.push({ file: checkpoint.file, label, reverted: [], skipped: [], error });
        remaining.unshift(checkpoint);
      }
    }
    if (this.lastResult !== null) this.lastResult = { ...this.lastResult, checkpoints: remaining };
    return {
      files,
      failed: files.filter((f) => f.error !== null).map((f) => f.file),
      message: summarizeUndo(files),
    };
  }
}

/** An agent's settings: `providers.toml`, `config.toml` and `mcp.json`. */
export class AgentScopeModel extends ScopeModel {
  readonly kind = "agent";
  readonly files: readonly StagedFile[];
  readonly providersFile: FileState<ProvidersFormState>;
  readonly configFile: FileState<ConfigFields>;
  readonly mcpFile: FileState<McpServerEntry[]>;

  /**
   * The install-wide values that matter to an agent's page, read from the hub's
   * `config.toml` and never written from here. Show them read-only.
   */
  install = $state.raw<Readonly<ConfigFields>>(defaultConfigFields());
  /** Why the hub's file couldn't be read for `install`. */
  installError = $state<string | null>(null);

  constructor(
    readonly agent: string,
    deps: SettingsDeps,
  ) {
    super(agent, deps);
    this.providersFile = new FileState(
      "providers",
      agentConfigFile(agent, "providers"),
      PROVIDERS_FORM,
    );
    this.configFile = new FileState("config", agentConfigFile(agent, "config"), AGENT_CONFIG_FORM);
    this.mcpFile = new FileState("mcp", agentConfigFile(agent, "mcp"), MCP_FORM);
    this.files = [this.providersFile, this.configFile, this.mcpFile];
  }

  /** The agent's `config.toml` as a form. */
  get config(): ConfigFields {
    return this.configFile.form;
  }

  get providers(): SettingsProviderEntry[] {
    return this.providersFile.form.providers;
  }

  /** The model roles, with their failover lists. */
  get models(): SettingsModelAssignments {
    return this.providersFile.form.models;
  }

  get mcpServers(): McpServerEntry[] {
    return this.mcpFile.form;
  }

  protected override typedSecrets(): TypedSecret[] {
    const { providers: current } = this.providersFile.form;
    const { providers: before } = this.providersFile.baseline;
    const keys: TypedSecret[] = [];
    for (const provider of current) {
      const name = provider.name.trim();
      const was = before.find((candidate) => candidate.name.trim() === name);
      // An Ollama server's key isn't a credential the secret store holds.
      if (name !== "" && provider.type !== "ollama" && isTyped(provider.apiKey, was?.apiKey)) {
        keys.push({
          name,
          value: provider.apiKey,
          apply: (reference) => {
            provider.apiKey = reference;
          },
        });
      }
    }
    return [...keys, ...typedConfigSecrets(this.configFile, "agent")];
  }

  protected override async loadOther(): Promise<void> {
    await this.readInstall();
  }

  protected override watches(): { file: ConfigFile; changed: () => Promise<void> }[] {
    return [...super.watches(), { file: HUB_CONFIG_FILE, changed: () => this.readInstall() }];
  }

  override get loadError(): string | null {
    return super.loadError ?? this.installError;
  }

  private async readInstall(): Promise<void> {
    try {
      this.install = parseConfigToml("", await this.deps.coordinator.read(HUB_CONFIG_FILE));
      this.installError = null;
    } catch (err) {
      this.installError = userErrorMessage(err, {
        action: "Couldn't read the install-wide settings.",
      });
    }
  }
}

/** The install-wide settings: the hub's `config.toml`. */
export class AllScopeModel extends ScopeModel {
  readonly kind = "all";
  readonly agent = null;
  readonly files: readonly StagedFile[];
  readonly configFile: FileState<ConfigFields>;

  constructor(deps: SettingsDeps) {
    super(ALL_SCOPE, deps);
    this.configFile = new FileState("config", HUB_CONFIG_FILE, HUB_CONFIG_FORM);
    this.files = [this.configFile];
  }

  /** The hub's `config.toml` as a form. */
  get config(): ConfigFields {
    return this.configFile.form;
  }

  protected override typedSecrets(): TypedSecret[] {
    return typedConfigSecrets(this.configFile, "hub");
  }
}

// ── Every scope ───────────────────────────────────────────────────────

/**
 * The scopes the user has opened, kept so staged changes outlive the modal and
 * a switch of scope. Get a scope in a script, not in a template expression:
 * creating one changes this registry.
 */
export class SettingsModel {
  private readonly scopes = new SvelteMap<string, AgentScopeModel | AllScopeModel>();

  constructor(private readonly deps: SettingsDeps = APP_DEPS) {}

  /** The install-wide scope. */
  all(): AllScopeModel {
    const existing = this.scopes.get(ALL_SCOPE);
    if (existing instanceof AllScopeModel) return existing;
    const created = new AllScopeModel(this.deps);
    this.scopes.set(ALL_SCOPE, created);
    return created;
  }

  agent(name: string): AgentScopeModel {
    const existing = this.scopes.get(name);
    if (existing instanceof AgentScopeModel) return existing;
    const created = new AgentScopeModel(name, this.deps);
    this.scopes.set(name, created);
    return created;
  }

  /** The scope a URL token names: `_all`, or an agent. */
  scope(id: string): AgentScopeModel | AllScopeModel {
    return scopeKind(id) === "all" ? this.all() : this.agent(id);
  }

  /** The scopes that hold unsaved work: staged changes or raw edits. */
  get stagedScopes(): (AgentScopeModel | AllScopeModel)[] {
    return [...this.scopes.values()].filter((scope) => scope.unsaved);
  }

  /** Forget a scope, such as an agent that was deleted, with whatever it had staged. */
  drop(id: string): void {
    this.scopes.get(id)?.dispose();
    this.scopes.delete(id);
  }
}

export const settingsModel = new SettingsModel();
