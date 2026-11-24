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
import { hub } from "./hub.svelte";
import { joinNames } from "./words";
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

export interface SettingsDeps {
  coordinator: ConfigCoordinator;
  storeSecret: (name: string, value: string) => Promise<{ reference: string }>;
}

const APP_DEPS: SettingsDeps = {
  coordinator: configCoordinator,
  storeSecret: storeSecretViaApi,
};

interface FormCodec<T> {
  parse: (raw: string) => T;
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

function clone<T>(value: T): T {
  const plain: unknown = $state.snapshot(value);
  return structuredClone(plain) as T;
}

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

export type FileLock = "form" | "raw";

export interface StagedFile {
  readonly name: FieldFile;
  readonly file: ConfigFile;
  readonly raw: string | null;
  readonly dirty: boolean;
  readonly patch: Record<string, unknown>;
  readonly changedOnDisk: boolean;
  readonly unreadable: boolean;
  readonly loadError: string | null;
  readonly rawDraft: string | null;
  readonly rawDraftBase: string | null;
  readonly lockedBy: FileLock | null;
  setRawDraft: (text: string | null) => void;
  discard: () => void;
  refresh: (raw: string) => void;
  markChangedOnDisk: () => void;
  fail: (message: string) => void;
  beginSave: () => { patch: Record<string, unknown>; done: (raw: string) => void };
}

export class FileState<T extends object> implements StagedFile {
  raw = $state<string | null>(null);
  baseline = $state.raw() as T;
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

export type FileSaveStatus = "saved" | "unchanged" | "failed" | "skipped" | "used-disk";

export interface FileSaveResult {
  file: FieldFile;
  label: string;
  status: FileSaveStatus;
  message: string | null;
  checkpointId: string | null;
}

export interface SavedCheckpoint {
  file: FieldFile;
  repo: RepoKind;
  path: string;
  id: string;
  text: string;
}

export interface SaveResult {
  outcome: "saved" | "partial" | "failed" | "nothing";
  files: FileSaveResult[];
  checkpoints: SavedCheckpoint[];
  storedSecrets: string[];
  message: string;
}

export interface UndoFileResult {
  file: FieldFile;
  label: string;
  reverted: string[];
  skipped: string[];
  error: string | null;
}

export interface UndoResult {
  files: UndoFileResult[];
  failed: FieldFile[];
  message: string;
}

export class ConflictUnanswered extends Error {}

function fileLabel(file: ConfigFile): string {
  return file.kind === "hub" ? "the install-wide config.toml" : configFileName(file);
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

export function diagnosticsOf(result: ValidateResponse): Diagnostic[] {
  const found = result.diagnostics ?? [];
  if (found.length > 0 || result.valid || result.error === undefined) return found;
  return [{ severity: "error", message: result.error }];
}

interface TypedSecret {
  name: string;
  value: string;
  apply: (reference: string) => void;
}

function isTyped(value: string, before: string | undefined): boolean {
  return value !== "" && value !== before && !isStoredReference(value);
}

const SECRET_FIELDS = [
  { field: "discord_token", name: "discord" },
  { field: "telegram_token", name: "telegram" },
  { field: "teams_app_password", name: "teams" },
  { field: "cloud_token", name: "cloud_token" },
  { field: "system_one_api_key", name: "system_one" },
  { field: "ws_brave_api_key", name: "ws_brave" },
  { field: "ws_tavily_api_key", name: "ws_tavily" },
  { field: "ws_ollama_api_key", name: "ws_ollama" },
] as const;

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

const NOTHING_STAGED: SaveResult = {
  outcome: "nothing",
  files: [],
  checkpoints: [],
  storedSecrets: [],
  message: "There is nothing to save.",
};

export abstract class ScopeModel {
  abstract readonly kind: ScopeKind;
  abstract readonly agent: string | null;
  abstract readonly files: readonly StagedFile[];

  saving = $state(false);
  loading = $state(false);
  lastResult = $state.raw<SaveResult | null>(null);
  diagnostics = $state.raw<readonly PlacedDiagnostic[]>([]);
  focusRequest = $state.raw<FieldRef | null>(null);

  get dirty(): boolean {
    return this.files.some((state) => state.dirty);
  }

  get unsaved(): boolean {
    return this.dirty || this.files.some((state) => state.rawDraft !== null);
  }

  get undoable(): boolean {
    return (this.lastResult?.checkpoints.length ?? 0) > 0;
  }

  get loadError(): string | null {
    return this.files.find((state) => state.loadError !== null)?.loadError ?? null;
  }

  get loaded(): boolean {
    return this.files.every((state) => state.raw !== null || state.loadError !== null);
  }

  private running: Promise<SaveResult> | null = null;
  private stops: (() => void)[] = [];
  private readonly source = Symbol("settings-scope");

  protected constructor(
    readonly id: string,
    protected readonly deps: SettingsDeps,
  ) {}

  protected abstract typedSecrets(): TypedSecret[];

  protected loadOther(): Promise<void> {
    return Promise.resolve();
  }

  protected watches(): { file: ConfigFile; changed: () => Promise<void> }[] {
    return this.files.map((state) => ({ file: state.file, changed: () => this.read(state) }));
  }

  async load(): Promise<void> {
    this.loading = true;
    try {
      await Promise.all([...this.files.map((state) => this.read(state)), this.loadOther()]);
      this.follow();
    } finally {
      this.loading = false;
    }
  }

  async reload(): Promise<void> {
    this.discard();
    for (const state of this.files) state.setRawDraft(null);
    await this.load();
  }

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

  dispose(): void {
    for (const stop of this.stops) stop();
    this.stops = [];
  }

  discard(): void {
    for (const state of this.files) state.discard();
    this.diagnostics = [];
  }

  file(name: FieldFile): StagedFile | undefined {
    return this.files.find((state) => state.name === name);
  }

  fieldDiagnostics(ref: FieldRef): Diagnostic[] {
    const key = fieldRefKey(ref);
    return this.diagnostics
      .filter((placed) => placed.field !== null && fieldRefKey(placed.field) === key)
      .map((placed) => placed.diagnostic);
  }

  sectionDiagnostics(section: SectionId): Diagnostic[] {
    return this.diagnostics
      .filter(
        (placed) =>
          placed.field === null && (placed.section === null || placed.section === section),
      )
      .map((placed) => placed.diagnostic);
  }

  flagProblems(file: FieldFile, found: readonly Diagnostic[]): void {
    const state = this.file(file);
    if (state !== undefined) this.place(state, found);
  }

  requestFocus(ref: FieldRef): void {
    this.focusRequest = ref;
  }

  takeFocus(): FieldRef | null {
    const ref = this.focusRequest;
    this.focusRequest = null;
    return ref;
  }

  private place(state: StagedFile, found: readonly Diagnostic[]): void {
    this.diagnostics = [
      ...this.diagnostics.filter((placed) => placed.file !== state.name),
      ...found.map((diagnostic) => placeDiagnostic(this.kind, state.name, diagnostic)),
    ];
  }

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

export class AgentScopeModel extends ScopeModel {
  readonly kind = "agent";
  readonly files: readonly StagedFile[];
  readonly providersFile: FileState<ProvidersFormState>;
  readonly configFile: FileState<ConfigFields>;
  readonly mcpFile: FileState<McpServerEntry[]>;

  install = $state.raw<Readonly<ConfigFields>>(defaultConfigFields());
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

  get config(): ConfigFields {
    return this.configFile.form;
  }

  get providers(): SettingsProviderEntry[] {
    return this.providersFile.form.providers;
  }

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

  get config(): ConfigFields {
    return this.configFile.form;
  }

  protected override typedSecrets(): TypedSecret[] {
    return typedConfigSecrets(this.configFile, "hub");
  }
}

export class SettingsModel {
  private readonly scopes = new SvelteMap<string, AgentScopeModel | AllScopeModel>();

  constructor(private readonly deps: SettingsDeps = APP_DEPS) {}

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

  scope(id: string): AgentScopeModel | AllScopeModel {
    return scopeKind(id) === "all" ? this.all() : this.agent(id);
  }

  get stagedScopes(): (AgentScopeModel | AllScopeModel)[] {
    return [...this.scopes.values()].filter((scope) => scope.unsaved);
  }

  drop(id: string): void {
    this.scopes.get(id)?.dispose();
    this.scopes.delete(id);
  }
}

export const settingsModel = new SettingsModel();

hub.onFrame((msg) => {
  if (msg.type === "agent_deleted") settingsModel.drop(msg.name);
});
