import {
  CACHE_KEY_HUB_CONFIG_RAW,
  cacheKeyConfigRaw,
  cacheKeyMcpRaw,
  cacheKeyProvidersRaw,
  fetchConfigRaw,
  fetchHubConfigRaw,
  fetchMcpRaw,
  fetchProvidersRaw,
  patchConfig,
  patchHubConfig,
  patchMcp,
  patchProviders,
  putConfigRaw,
  putHubConfigRaw,
  putMcpRaw,
  putProvidersRaw,
  restoreCheckpoint,
  undoCheckpoint,
} from "./api";
import { invalidate } from "./cache";
import type { RepoKind, RestoreOutcome, UndoOutcome, ValidateResponse } from "./types";

export type AgentConfigFileName = "config" | "providers" | "mcp";

export type ConfigFile =
  | { readonly kind: "hub" }
  | { readonly kind: "agent"; readonly agent: string; readonly name: AgentConfigFileName };

export const HUB_CONFIG_FILE: ConfigFile = { kind: "hub" };

export function agentConfigFile(agent: string, name: AgentConfigFileName): ConfigFile {
  return { kind: "agent", agent, name };
}

export function configFileKey(file: ConfigFile): string {
  return file.kind === "hub" ? "hub" : `agent:${file.agent}:${file.name}`;
}

export function configFileName(file: ConfigFile): string {
  if (file.kind === "hub") return "config.toml";
  return file.name === "mcp" ? "mcp.json" : `${file.name}.toml`;
}

export interface ConfigIo {
  read: (file: ConfigFile) => Promise<string>;
  patch: (file: ConfigFile, diff: Record<string, unknown>) => Promise<ValidateResponse>;
  put: (file: ConfigFile, text: string) => Promise<ValidateResponse>;
  restore: (
    agent: string | null,
    checkpointId: string,
    repo: RepoKind,
    path: string,
  ) => Promise<RestoreOutcome>;
  undo: (agent: string | null, checkpointId: string, repo: RepoKind) => Promise<UndoOutcome>;
}

interface FileRequests {
  cacheKey: string;
  fetch: () => Promise<string>;
  patch: (diff: Record<string, unknown>) => Promise<ValidateResponse>;
  put: (text: string) => Promise<ValidateResponse>;
}

function requestsFor(file: ConfigFile): FileRequests {
  if (file.kind === "hub") {
    return {
      cacheKey: CACHE_KEY_HUB_CONFIG_RAW,
      fetch: fetchHubConfigRaw,
      patch: patchHubConfig,
      put: putHubConfigRaw,
    };
  }
  const { agent } = file;
  switch (file.name) {
    case "config":
      return {
        cacheKey: cacheKeyConfigRaw(agent),
        fetch: () => fetchConfigRaw(agent),
        patch: (diff) => patchConfig(agent, diff),
        put: (text) => putConfigRaw(agent, text),
      };
    case "providers":
      return {
        cacheKey: cacheKeyProvidersRaw(agent),
        fetch: () => fetchProvidersRaw(agent),
        patch: (diff) => patchProviders(agent, diff),
        put: (text) => putProvidersRaw(agent, text),
      };
    case "mcp":
      return {
        cacheKey: cacheKeyMcpRaw(agent),
        fetch: () => fetchMcpRaw(agent),
        patch: (diff) => patchMcp(agent, diff),
        put: (text) => putMcpRaw(agent, text),
      };
  }
}

const apiConfigIo: ConfigIo = {
  read: (file) => {
    const requests = requestsFor(file);
    invalidate(requests.cacheKey);
    return requests.fetch();
  },
  patch: (file, diff) => requestsFor(file).patch(diff),
  put: (file, text) => requestsFor(file).put(text),
  restore: restoreCheckpoint,
  undo: undoCheckpoint,
};

type KeyPath = readonly string[];

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

type ConfigParser = (text: string) => unknown;

let tomlParser: Promise<ConfigParser> | null = null;

function parserFor(file: ConfigFile): Promise<ConfigParser> {
  if (file.kind === "agent" && file.name === "mcp") return Promise.resolve(JSON.parse);
  tomlParser ??= import("smol-toml").then(
    (toml) => toml.parse,
    (err: unknown) => {
      tomlParser = null;
      throw err;
    },
  );
  return tomlParser;
}

function parseConfigText(parse: ConfigParser, text: string): Record<string, unknown> | null {
  if (text.trim() === "") return {};
  try {
    const parsed = parse(text);
    return isRecord(parsed) ? parsed : null;
  } catch {
    return null;
  }
}

function sameValue(a: unknown, b: unknown): boolean {
  if (Array.isArray(a) || Array.isArray(b)) {
    return (
      Array.isArray(a) &&
      Array.isArray(b) &&
      a.length === b.length &&
      a.every((item, i) => sameValue(item, b[i]))
    );
  }
  if (a instanceof Date && b instanceof Date) return a.toISOString() === b.toISOString();
  if (isRecord(a) && isRecord(b)) return changedKeyPaths(a, b).length === 0;
  return Object.is(a, b);
}

function changedKeyPaths(before: unknown, after: unknown, path: KeyPath = []): KeyPath[] {
  if (isRecord(before) && isRecord(after)) {
    const keys = new Set([...Object.keys(before), ...Object.keys(after)]);
    return [...keys].flatMap((key) => changedKeyPaths(before[key], after[key], [...path, key]));
  }
  return sameValue(before, after) ? [] : [path];
}

function patchKeyPaths(diff: Record<string, unknown>, path: KeyPath = []): KeyPath[] {
  return Object.entries(diff).flatMap(([key, value]) => {
    const here = [...path, key];
    if (!isRecord(value) || "$inline" in value || Object.keys(value).length === 0) return [here];
    return patchKeyPaths(value, here);
  });
}

function keysOverlap(a: KeyPath, b: KeyPath): boolean {
  return a.every((segment, i) => i >= b.length || segment === b[i]);
}

async function conflictingKeys(
  file: ConfigFile,
  seen: string,
  disk: string,
  pending: readonly KeyPath[] | null,
): Promise<string[] | null> {
  if (disk === seen) return null;
  const parse = await parserFor(file);
  const after = parseConfigText(parse, disk);
  // The server reports a file it can't read when the write is refused.
  if (after === null) return null;
  const before = parseConfigText(parse, seen);
  if (before === null) return [];
  const changed = changedKeyPaths(before, after);
  if (pending === null) return changed.length === 0 ? [] : keyNames(changed);
  const hit = changed.filter((key) => pending.some((mine) => keysOverlap(key, mine)));
  return hit.length === 0 ? null : keyNames(hit);
}

function keyNames(paths: readonly KeyPath[]): string[] {
  return [...new Set(paths.map((path) => path.join(".")))].sort();
}

const CHECKPOINT_LOCATIONS: readonly {
  repo: RepoKind;
  path: string;
  file: (agent: string | null) => ConfigFile | null;
}[] = [
  { repo: "hub", path: "config.toml", file: () => HUB_CONFIG_FILE },
  { repo: "agent_config", path: "config.toml", file: (a) => agentFile(a, "config") },
  { repo: "agent_config", path: "providers.toml", file: (a) => agentFile(a, "providers") },
  { repo: "workspace", path: "config/mcp.json", file: (a) => agentFile(a, "mcp") },
];

function agentFile(agent: string | null, name: AgentConfigFileName): ConfigFile | null {
  return agent === null ? null : agentConfigFile(agent, name);
}

export function checkpointLocationOf(file: ConfigFile): { repo: RepoKind; path: string } {
  const agent = file.kind === "agent" ? file.agent : null;
  const key = configFileKey(file);
  const location = CHECKPOINT_LOCATIONS.find((candidate) => {
    const candidateFile = candidate.file(agent);
    return candidateFile !== null && configFileKey(candidateFile) === key;
  });
  return { repo: location?.repo ?? "agent_config", path: location?.path ?? "config.toml" };
}

function reaches(requested: string, location: string): boolean {
  const base = requested.replace(/^\/+|\/+$/g, "");
  return base === "" || location === base || location.startsWith(`${base}/`);
}

function configFilesWithin(
  agent: string | null,
  repo: RepoKind,
  paths: readonly string[],
): ConfigFile[] {
  const files: ConfigFile[] = [];
  for (const location of CHECKPOINT_LOCATIONS) {
    if (location.repo !== repo || !paths.some((path) => reaches(path, location.path))) continue;
    const file = location.file(agent);
    if (file !== null) files.push(file);
  }
  return files;
}

export type ConfigEdit = { patch: Record<string, unknown> } | { text: string };

export interface ConfigConflict {
  file: ConfigFile;
  keys: string[];
  disk: string;
}

export type ConfigChoice = "keep-mine" | "use-disk";

export type ConfigChooser = (conflict: ConfigConflict) => Promise<ConfigChoice>;

export interface ConfigSaveRequest {
  baseline: string;
  edit: ConfigEdit;
  choose: ConfigChooser;
  source?: symbol;
}

export interface ConfigSaved {
  result: ValidateResponse;
  written: boolean;
  raw: string | null;
}

export type ConfigSaveOutcome =
  | ({ kind: "saved" } & ConfigSaved)
  | { kind: "used-disk"; raw: string };

type SaveStep = { saved: ConfigSaved } | { conflict: ConfigConflict };

export type ConfigChangeCause = "write" | "reload" | "restore" | "external";

export interface ConfigChange {
  file: ConfigFile;
  cause: ConfigChangeCause;
  source: symbol | null;
}

export type ConfigListener = (change: ConfigChange) => void;

export class ConfigCoordinator {
  private readonly tails = new Map<string, Promise<void>>();
  private readonly told = new Map<string, string>();
  private readonly listeners = new Map<string, Set<ConfigListener>>();

  constructor(private readonly io: ConfigIo = apiConfigIo) {}

  subscribe(file: ConfigFile, listener: ConfigListener): () => void {
    const key = configFileKey(file);
    const set = this.listeners.get(key) ?? new Set<ConfigListener>();
    set.add(listener);
    this.listeners.set(key, set);
    return () => {
      set.delete(listener);
      if (set.size === 0 && this.listeners.get(key) === set) this.listeners.delete(key);
    };
  }

  read(file: ConfigFile): Promise<string> {
    return this.locked(file, () => this.io.read(file));
  }

  async save(file: ConfigFile, request: ConfigSaveRequest): Promise<ConfigSaveOutcome> {
    const { edit } = request;
    if ("patch" in edit && Object.keys(edit.patch).length === 0) {
      return { kind: "saved", result: { valid: true }, written: false, raw: request.baseline };
    }
    const pending = "patch" in edit ? patchKeyPaths(edit.patch) : null;
    let seen = request.baseline;
    for (;;) {
      const step = await this.locked<SaveStep>(file, async () => {
        const disk = await this.io.read(file);
        const keys = await conflictingKeys(file, seen, disk, pending);
        if (keys === null) {
          return { saved: await this.write(file, edit, request.source ?? null, disk) };
        }
        return { conflict: { file, keys, disk } };
      });
      if ("saved" in step) return { kind: "saved", ...step.saved };

      const choice = await request.choose(step.conflict);
      if (choice === "use-disk") {
        this.announce(file, "reload", request.source ?? null, step.conflict.disk);
        return { kind: "used-disk", raw: step.conflict.disk };
      }
      seen = step.conflict.disk;
    }
  }

  edit(
    file: ConfigFile,
    build: (raw: string) => Record<string, unknown> | null,
    source?: symbol,
  ): Promise<ConfigSaved> {
    return this.locked(file, async () => {
      const raw = await this.io.read(file);
      const patch = build(raw);
      if (patch === null || Object.keys(patch).length === 0) {
        return { result: { valid: true }, written: false, raw };
      }
      return this.write(file, { patch }, source ?? null, raw);
    });
  }

  reload(file: ConfigFile, source?: symbol): Promise<string> {
    return this.locked(file, async () => {
      const raw = await this.io.read(file);
      this.announce(file, "reload", source ?? null, raw);
      return raw;
    });
  }

  restore(
    agent: string | null,
    checkpointId: string,
    repo: RepoKind,
    path: string,
  ): Promise<RestoreOutcome> {
    return this.lockedAll(configFilesWithin(agent, repo, [path]), async () => {
      const outcome = await this.io.restore(agent, checkpointId, repo, path);
      await this.announceRestored(agent, repo, outcome.restored_paths);
      return outcome;
    });
  }

  undo(agent: string | null, checkpointId: string, repo: RepoKind): Promise<UndoOutcome> {
    return this.lockedAll(configFilesWithin(agent, repo, [""]), async () => {
      const outcome = await this.io.undo(agent, checkpointId, repo);
      await this.announceRestored(agent, repo, outcome.reverted_paths);
      return outcome;
    });
  }

  externalChange(file: ConfigFile): Promise<void> {
    return this.locked(file, async () => {
      const after = await this.readQuietly(file);
      if (after !== null && after === this.told.get(configFileKey(file))) return;
      this.announce(file, "external", null, after);
    });
  }

  async externalResync(files: readonly ConfigFile[]): Promise<void> {
    await Promise.all(
      files.map((file) =>
        this.locked(file, async () => {
          this.announce(file, "external", null, await this.readQuietly(file));
        }),
      ),
    );
  }

  private async write(
    file: ConfigFile,
    edit: ConfigEdit,
    source: symbol | null,
    disk: string,
  ): Promise<ConfigSaved> {
    const result =
      "patch" in edit ? await this.io.patch(file, edit.patch) : await this.io.put(file, edit.text);
    const written = "text" in edit || result.valid;
    if (!written) return { result, written, raw: disk };
    const raw = await this.readQuietly(file);
    this.announce(file, "write", source, raw);
    return { result, written, raw };
  }

  private async readQuietly(file: ConfigFile): Promise<string | null> {
    try {
      return await this.io.read(file);
    } catch {
      return null;
    }
  }

  private async announceRestored(
    agent: string | null,
    repo: RepoKind,
    paths: readonly string[],
  ): Promise<void> {
    for (const file of configFilesWithin(agent, repo, paths)) {
      this.announce(file, "restore", null, await this.readQuietly(file));
    }
  }

  private announce(
    file: ConfigFile,
    cause: ConfigChangeCause,
    source: symbol | null,
    text: string | null,
  ): void {
    const key = configFileKey(file);
    if (text === null) this.told.delete(key);
    else this.told.set(key, text);
    const listeners = this.listeners.get(key);
    if (!listeners) return;
    const change: ConfigChange = { file, cause, source };
    for (const listener of [...listeners]) {
      try {
        listener(change);
      } catch (err) {
        queueMicrotask(() => {
          throw err;
        });
      }
    }
  }

  private locked<T>(file: ConfigFile, run: () => Promise<T>): Promise<T> {
    const key = configFileKey(file);
    const result = (this.tails.get(key) ?? Promise.resolve()).then(run);
    const tail = result.then(
      () => {},
      () => {},
    );
    this.tails.set(key, tail);
    void tail.then(() => {
      if (this.tails.get(key) === tail) this.tails.delete(key);
    });
    return result;
  }

  private lockedAll<T>(files: readonly ConfigFile[], run: () => Promise<T>): Promise<T> {
    const ordered = [...files].sort((a, b) => configFileKey(a).localeCompare(configFileKey(b)));
    const next = (i: number): Promise<T> => {
      const file = ordered[i];
      return file === undefined ? run() : this.locked(file, () => next(i + 1));
    };
    return next(0);
  }
}

export const configCoordinator = new ConfigCoordinator();
