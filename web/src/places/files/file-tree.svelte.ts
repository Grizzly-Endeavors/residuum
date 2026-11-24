import { SvelteSet } from "svelte/reactivity";
import { deleteWorkspaceFile, fetchWorkspaceFiles, moveWorkspaceFile } from "../../lib/api";
import { userErrorMessage } from "../../lib/errors";
import { hub } from "../../lib/hub.svelte";
import { toast } from "../../lib/toast.svelte";
import { treeUpdateFor, treeWatchPrefix } from "../../lib/tree-changes";
import type { WorkspaceChange, WorkspaceEntry } from "../../lib/types";
import { notifyWithWorkspaceUndo } from "../../lib/undo";
import type { WatchHandler, WatchOwner } from "../../lib/watch-registry";
import { ws } from "../../lib/ws.svelte";
import { panelFile } from "./file-buffer.svelte";
import { fileName, parentDir, type FileSource } from "./file-source";

export type TreeRow =
  | { kind: "folder" | "file"; path: string; depth: number; entry: WorkspaceEntry }
  | { kind: "empty"; path: string; depth: number }
  | { kind: "error"; path: string; depth: number; message: string };

function byKindThenName(a: WorkspaceEntry, b: WorkspaceEntry): number {
  if (a.entry_type !== b.entry_type) return a.entry_type === "directory" ? -1 : 1;
  return a.name.localeCompare(b.name);
}

function join(dir: string, name: string): string {
  return dir === "" ? name : `${dir}/${name}`;
}

export class FileTree {
  listings = $state<Record<string, WorkspaceEntry[]>>({});
  errors = $state<Record<string, string>>({});
  readonly expanded = new SvelteSet<string>();

  readonly rows = $derived.by(() => {
    const rows: TreeRow[] = [];
    const walk = (dir: string, depth: number): void => {
      const message = this.errors[dir];
      if (message !== undefined) rows.push({ kind: "error", path: dir, depth, message });
      const entries = this.listings[dir];
      if (entries === undefined) return;
      if (entries.length === 0 && dir !== "") rows.push({ kind: "empty", path: dir, depth });
      for (const entry of [...entries].sort(byKindThenName)) {
        const path = join(dir, entry.name);
        const folder = entry.entry_type === "directory";
        rows.push({ kind: folder ? "folder" : "file", path, depth, entry });
        if (folder && this.expanded.has(path)) walk(path, depth + 1);
      }
    };
    walk("", 0);
    return rows;
  });

  constructor(readonly source: FileSource) {}

  async list(dir: string): Promise<void> {
    const { agent, scope } = this.source;
    try {
      const entries = await fetchWorkspaceFiles(agent, dir === "" ? undefined : dir, scope);
      this.listings = { ...this.listings, [dir]: entries };
      if (dir in this.errors) this.errors = this.without(this.errors, [dir]);
    } catch (err) {
      const action = dir === "" ? "Couldn't list these files." : `Couldn't list ${fileName(dir)}.`;
      this.errors = { ...this.errors, [dir]: userErrorMessage(err, { action }) };
    }
  }

  async toggle(dir: string): Promise<void> {
    if (this.expanded.has(dir)) {
      this.expanded.delete(dir);
      return;
    }
    this.expanded.add(dir);
    if (!(dir in this.listings)) await this.list(dir);
  }

  async reveal(path: string): Promise<void> {
    const folders = path.split("/").slice(0, -1);
    for (const [index] of folders.entries()) {
      const dir = folders.slice(0, index + 1).join("/");
      this.expanded.add(dir);
      if (!(dir in this.listings)) await this.list(dir);
    }
  }

  refreshAll(): void {
    const dirs = ["", ...Object.keys(this.listings), ...Object.keys(this.errors)];
    for (const [index, dir] of dirs.entries()) {
      if (dirs.indexOf(dir) === index) void this.list(dir);
    }
  }

  applyChanges(changes: readonly WorkspaceChange[]): void {
    const update = treeUpdateFor(changes, this.source.scope, Object.keys(this.listings));
    if (update.forget.length > 0) {
      this.listings = this.without(this.listings, update.forget);
      for (const dir of update.forget) this.expanded.delete(dir);
    }
    for (const dir of update.reload) void this.list(dir);
  }

  watch(): () => void {
    const handler: WatchHandler = {
      changed: (changes) => {
        this.applyChanges(changes);
      },
      resync: () => {
        this.refreshAll();
      },
      reconnected: () => {
        this.refreshAll();
      },
    };
    const { agent, scope } = this.source;
    let owner: WatchOwner | null = null;
    if (scope === "team") owner = hub.teamWatches.register(handler);
    else if (agent !== null) owner = ws.watches.register(handler, { agent });
    owner?.set([treeWatchPrefix(scope)]);
    return () => owner?.release();
  }

  async rename(path: string, name: string): Promise<string | null> {
    const target = name.trim().replace(/^\/+|\/+$/g, "");
    if (target === "" || target === fileName(path)) return null;
    const to = join(parentDir(path), target);
    const { agent, scope } = this.source;
    try {
      await moveWorkspaceFile(agent, path, to, false, scope);
    } catch (err) {
      toast.error(userErrorMessage(err, { action: `Couldn't rename ${fileName(path)}.` }));
      return null;
    }
    if (panelFile.shown?.shows(this.source, path)) panelFile.shown.moved(to);
    const from = parentDir(path);
    await this.relist(from);
    if (parentDir(to) !== from) await this.relist(parentDir(to));
    toast.success(target.includes("/") ? `Moved to ${to}.` : `Renamed to ${target}.`);
    return to;
  }

  async remove(path: string): Promise<void> {
    const { agent, scope } = this.source;
    let checkpoints;
    try {
      checkpoints = await deleteWorkspaceFile(agent, path, scope);
    } catch (err) {
      toast.error(userErrorMessage(err, { action: `Couldn't delete ${fileName(path)}.` }));
      return;
    }
    if (panelFile.shown?.shows(this.source, path)) panelFile.shown.removed();
    await this.relist(parentDir(path));
    notifyWithWorkspaceUndo(
      agent,
      `Deleted ${fileName(path)}.`,
      scope === "team" ? `team/${path}` : path,
      checkpoints,
      async () => {
        await this.relist(parentDir(path));
        if (panelFile.shown?.shows(this.source, path)) await panelFile.shown.refresh();
      },
    );
  }

  async relist(dir: string): Promise<void> {
    if (dir in this.listings) await this.list(dir);
  }

  private without<T>(record: Record<string, T>, keys: readonly string[]): Record<string, T> {
    return Object.fromEntries(Object.entries(record).filter(([key]) => !keys.includes(key)));
  }
}
