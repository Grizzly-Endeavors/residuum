// ── Config write coordinator ─────────────────────────────────────────
//
// Every write to a config file goes through here: an agent's `config.toml`,
// `providers.toml` and `mcp.json`, and the hub's `config.toml`. That covers
// settings saves, raw saves, the composer's model and thinking controls, and
// restoring a file from a checkpoint.
//
// - Writes to one file are serialized, so a read-modify-write can't be
//   interleaved with another write to the same file.
// - Before a save, the file is read again. If it changed since the caller's
//   view loaded it and the changed keys overlap what the caller is about to
//   write, the caller chooses between its own changes and what is on disk.
//   If they don't overlap the save goes ahead: a patch applies to the current
//   file, so the other keys' changes survive.
// - After every write, reload and restore, and whenever the file changes
//   under us, subscribers are told, so every view that shows a config value
//   can refresh.
//
// Where external changes come from is wired in `config-sync.ts`. Any agent's
// files can be written, but only the bound agent's `config/` folder and the
// hub's config have a change feed. Other agents' files have none, so for them
// the re-read before a save is the only protection.

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

// ── Files ─────────────────────────────────────────────────────────────

export type AgentConfigFileName = "config" | "providers" | "mcp";

/** One config file: the hub's `config.toml`, or one of an agent's three. */
export type ConfigFile =
  | { readonly kind: "hub" }
  | { readonly kind: "agent"; readonly agent: string; readonly name: AgentConfigFileName };

export const HUB_CONFIG_FILE: ConfigFile = { kind: "hub" };

export function agentConfigFile(agent: string, name: AgentConfigFileName): ConfigFile {
  return { kind: "agent", agent, name };
}

/** A string that is the same for the same file. */
export function configFileKey(file: ConfigFile): string {
  return file.kind === "hub" ? "hub" : `agent:${file.agent}:${file.name}`;
}

/** The file's name on disk, for messages. */
export function configFileName(file: ConfigFile): string {
  if (file.kind === "hub") return "config.toml";
  return file.name === "mcp" ? "mcp.json" : `${file.name}.toml`;
}

// ── What the coordinator asks of the server ──────────────────────────

/** The requests the coordinator makes. Tests replace them. */
export interface ConfigIo {
  /** The file's text now, read from the server and never from a cached copy. */
  read: (file: ConfigFile) => Promise<string>;
  /** Merge a diff into the file. Nothing is written unless the result is valid. */
  patch: (file: ConfigFile, diff: Record<string, unknown>) => Promise<ValidateResponse>;
  /** Replace the file's text. It is written whatever the result says about the text. */
  put: (file: ConfigFile, text: string) => Promise<ValidateResponse>;
  restore: (
    agent: string | null,
    checkpointId: string,
    repo: RepoKind,
    path: string,
  ) => Promise<RestoreOutcome>;
  undo: (agent: string | null, checkpointId: string, repo: RepoKind) => Promise<UndoOutcome>;
}

/** What the API client offers for one file. */
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
    // The fetch is cached, so drop the copy first; the fresh text is cached
    // for the views that read it next.
    invalidate(requests.cacheKey);
    return requests.fetch();
  },
  patch: (file, diff) => requestsFor(file).patch(diff),
  put: (file, text) => requestsFor(file).put(text),
  restore: restoreCheckpoint,
  undo: undoCheckpoint,
};

// ── Keys ──────────────────────────────────────────────────────────────

/** A key's place in the file: `["models", "main"]` is `models.main`. */
type KeyPath = readonly string[];

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

type ConfigParser = (text: string) => unknown;

/**
 * The TOML parser, loaded the first time a save finds its file changed on
 * disk, which is the only time keys are compared. A load that fails is tried
 * again by the next save.
 */
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

/** The file's content as nested objects, or `null` when it isn't valid TOML or JSON. */
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

/**
 * The keys whose value differs between two documents. A table is followed
 * into; an array, a scalar, or a value that changed type is one key.
 */
function changedKeyPaths(before: unknown, after: unknown, path: KeyPath = []): KeyPath[] {
  if (isRecord(before) && isRecord(after)) {
    const keys = new Set([...Object.keys(before), ...Object.keys(after)]);
    return [...keys].flatMap((key) => changedKeyPaths(before[key], after[key], [...path, key]));
  }
  return sameValue(before, after) ? [] : [path];
}

/**
 * The keys a PATCH diff sets or removes. A nested object recurses, and
 * `{$inline: {...}}` sets the key to an inline table, so it is one key.
 */
function patchKeyPaths(diff: Record<string, unknown>, path: KeyPath = []): KeyPath[] {
  return Object.entries(diff).flatMap(([key, value]) => {
    const here = [...path, key];
    if (!isRecord(value) || "$inline" in value || Object.keys(value).length === 0) return [here];
    return patchKeyPaths(value, here);
  });
}

/** Whether one key is the other, or lies inside it. */
function keysOverlap(a: KeyPath, b: KeyPath): boolean {
  return a.every((segment, i) => i >= b.length || segment === b[i]);
}

/**
 * The keys that changed between `seen` and `disk` and that the write is about
 * to set. `null` means nothing is in the write's way. `[]` means the file
 * changed in a way that can't be put down to keys (the old text isn't valid),
 * or the change is to text alone and the write replaces all of it.
 *
 * `pending` is the keys of a patch, or `null` for a write of the whole text.
 */
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

// ── Checkpoint repositories ──────────────────────────────────────────

/** Where each config file lives in its checkpoint repository. */
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

/** Where `file` lives in its checkpoint repository, which an Undo of a save to it restores from. */
export function checkpointLocationOf(file: ConfigFile): { repo: RepoKind; path: string } {
  const agent = file.kind === "agent" ? file.agent : null;
  const key = configFileKey(file);
  const location = CHECKPOINT_LOCATIONS.find((candidate) => {
    const candidateFile = candidate.file(agent);
    return candidateFile !== null && configFileKey(candidateFile) === key;
  });
  // Every file `ConfigFile` can name has a location above.
  return { repo: location?.repo ?? "agent_config", path: location?.path ?? "config.toml" };
}

/** Whether restoring `requested` (a file, a folder, or `""` for the whole repo) reaches `location`. */
function reaches(requested: string, location: string): boolean {
  const base = requested.replace(/^\/+|\/+$/g, "");
  return base === "" || location === base || location.startsWith(`${base}/`);
}

/** The config files that restoring or reverting any of `paths` in `repo` writes. */
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

// ── Saving ────────────────────────────────────────────────────────────

/** What to write to a file. */
export type ConfigEdit =
  /** Merge a PATCH diff (see `settings-toml.ts`) into the file, leaving every other key alone. */
  | { patch: Record<string, unknown> }
  /** Replace the whole file, as a raw save does. */
  | { text: string };

/** A file that changed on disk under keys the caller is about to write. */
export interface ConfigConflict {
  file: ConfigFile;
  /**
   * The keys that changed on disk and overlap the write, like `models.main`,
   * sorted. Empty when the change can't be put down to keys.
   */
  keys: string[];
  /** The file's text on disk now. */
  disk: string;
}

export type ConfigChoice = "keep-mine" | "use-disk";

/**
 * Asked when a save finds a conflict. "keep-mine" goes on with the write;
 * "use-disk" drops it. No lock is held while the answer is awaited, so a
 * caller can ask the user.
 */
export type ConfigChooser = (conflict: ConfigConflict) => Promise<ConfigChoice>;

export interface ConfigSaveRequest {
  /** The file's text as the caller's view last loaded or saved it. */
  baseline: string;
  edit: ConfigEdit;
  choose: ConfigChooser;
  /** Names the caller in the notification, so a view can ignore the writes it made itself. */
  source?: symbol;
}

/** A save that reached the server. */
export interface ConfigSaved {
  /** The server's verdict. A patch is written only when it is valid; a raw save is always written. */
  result: ValidateResponse;
  /** The file's text changed. */
  written: boolean;
  /**
   * The file's text now: the new baseline for the caller's view. `null` when
   * the file was written but couldn't be read back.
   */
  raw: string | null;
}

export type ConfigSaveOutcome =
  | ({ kind: "saved" } & ConfigSaved)
  /** The caller chose what is on disk, so nothing was written. `raw` is that text. */
  | { kind: "used-disk"; raw: string };

/** What one pass of a save, under the file's lock, came to: it wrote the file, or stopped at a conflict. */
type SaveStep = { saved: ConfigSaved } | { conflict: ConfigConflict };

// ── Subscribers ───────────────────────────────────────────────────────

/** Why subscribers are told about a file. */
export type ConfigChangeCause =
  /** A save through the coordinator wrote it. */
  | "write"
  /** It was read again from disk on request, or the caller chose what was on disk. */
  | "reload"
  /** A checkpoint restore or undo wrote it. */
  | "restore"
  /** It changed outside this client's writes: the change feed or a hub config reload reported it. */
  | "external";

export interface ConfigChange {
  file: ConfigFile;
  cause: ConfigChangeCause;
  /** The `source` of the save that caused it, if it came from one. */
  source: symbol | null;
}

export type ConfigListener = (change: ConfigChange) => void;

// ── The coordinator ───────────────────────────────────────────────────

export class ConfigCoordinator {
  /** The last request of each file's queue; a file with no entry has nothing waiting. */
  private readonly tails = new Map<string, Promise<void>>();
  /**
   * The text each file read as when subscribers were last told about it. A
   * change report that reads the same was already told, which is how the echo
   * of a write is told from a change made elsewhere.
   */
  private readonly told = new Map<string, string>();
  private readonly listeners = new Map<string, Set<ConfigListener>>();

  constructor(private readonly io: ConfigIo = apiConfigIo) {}

  /** Hear about every change to `file`. Returns a function that stops listening. */
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

  /**
   * The file's text on disk now, read in its turn behind any write to it. Tells
   * no one: for a view that follows a change it was told about.
   */
  read(file: ConfigFile): Promise<string> {
    return this.locked(file, () => this.io.read(file));
  }

  /**
   * Write `request.edit` to `file`. The file is read again first; if it
   * changed since `request.baseline` under keys the edit sets, `request.choose`
   * decides. Throws what the API client throws when a request fails.
   */
  async save(file: ConfigFile, request: ConfigSaveRequest): Promise<ConfigSaveOutcome> {
    const { edit } = request;
    if ("patch" in edit && Object.keys(edit.patch).length === 0) {
      return { kind: "saved", result: { valid: true }, written: false, raw: request.baseline };
    }
    const pending = "patch" in edit ? patchKeyPaths(edit.patch) : null;
    // The text the caller has already seen and decided against; only a
    // change after it is a new conflict.
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

  /**
   * Read the file, build a patch from its text, and write it, all with no other
   * write to the file between. Nothing can have changed since the read, so
   * there is nothing to choose. `build` returns `null` to write nothing.
   */
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

  /** Read `file` from disk, bypassing any cached copy, and tell subscribers to do the same. */
  reload(file: ConfigFile, source?: symbol): Promise<string> {
    return this.locked(file, async () => {
      const raw = await this.io.read(file);
      this.announce(file, "reload", source ?? null, raw);
      return raw;
    });
  }

  /**
   * Restore `path` to its content at a checkpoint. Any config file it writes
   * is announced once the restore is done.
   */
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

  /** Undo everything a checkpoint changed. Any config file it writes is announced afterwards. */
  undo(agent: string | null, checkpointId: string, repo: RepoKind): Promise<UndoOutcome> {
    return this.lockedAll(configFilesWithin(agent, repo, [""]), async () => {
      const outcome = await this.io.undo(agent, checkpointId, repo);
      await this.announceRestored(agent, repo, outcome.reverted_paths);
      return outcome;
    });
  }

  /**
   * `file` may have changed outside this client's writes. Subscribers hear of
   * it unless the file turns out to read as it did the last time the
   * coordinator saw it, which is what the echo of its own write looks like.
   */
  externalChange(file: ConfigFile): Promise<void> {
    return this.locked(file, async () => {
      const after = await this.readQuietly(file);
      if (after !== null && after === this.told.get(configFileKey(file))) return;
      this.announce(file, "external", null, after);
    });
  }

  /** The change feed lost track of changes, so each of `files` may have changed. */
  async externalResync(files: readonly ConfigFile[]): Promise<void> {
    await Promise.all(
      files.map((file) =>
        this.locked(file, async () => {
          this.announce(file, "external", null, await this.readQuietly(file));
        }),
      ),
    );
  }

  // ── Internals ──────────────────────────────────────────────────────

  /** Send the edit. Runs with `file`'s queue held; `disk` is what the last read found. */
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

  /** Read the file from the server, or `null` when that fails: whoever listens reads it again and reports the failure. */
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

  /** Tell the file's subscribers. `text` is what the file reads as now, when it could be read. */
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
        // A failing subscriber must not fail the write it heard of, or keep
        // the others from hearing. It is raised as an uncaught error instead,
        // which the page reports.
        queueMicrotask(() => {
          throw err;
        });
      }
    }
  }

  /** Run `run` once everything queued for `file` before it is done. */
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

  /** `locked` for several files, taken in one order so two callers can't wait on each other. */
  private lockedAll<T>(files: readonly ConfigFile[], run: () => Promise<T>): Promise<T> {
    const ordered = [...files].sort((a, b) => configFileKey(a).localeCompare(configFileKey(b)));
    const next = (i: number): Promise<T> => {
      const file = ordered[i];
      return file === undefined ? run() : this.locked(file, () => next(i + 1));
    };
    return next(0);
  }
}

/** The app's coordinator. */
export const configCoordinator = new ConfigCoordinator();
