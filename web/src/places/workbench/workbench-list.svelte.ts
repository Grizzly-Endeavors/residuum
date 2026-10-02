// ── The Workbench's list (Svelte 5 runes) ────────────────────────────
//
// Every artifact, where they open, which one an agent is changing, and the
// commands on them. It follows the hub socket's artifact events, so it stays
// current with no agent running, and reads everything again when the hub
// connection comes back, since changes made in between arrive as no frames.

import { SvelteSet } from "svelte/reactivity";
import {
  deleteWorkbenchArtifact,
  fetchWorkbenchArtifacts,
  fetchWorkbenchInfo,
  stopSession,
} from "../../lib/api";
import { userErrorMessage } from "../../lib/errors";
import type { HubServerMessage, LiveSession } from "../../lib/hub-types";
import { notifications } from "../../lib/notifications.svelte";
import type { ArtifactSummary } from "../../lib/types";
import { notifyWithUndo } from "../../lib/undo";
import { artifactUrl, resolveArtifactsOrigin, type ArtifactsOrigin } from "../../lib/workbench";

/** How long an artifact reads "updating now" after an agent changes it. */
export const CHANGE_GLOW_MS = 2400;

/** What the list reads from outside: the hub socket's frames and the page's address. */
export interface WorkbenchSource {
  onFrame: (listener: (msg: HubServerMessage) => void) => () => void;
  page: () => Pick<Location, "origin" | "protocol" | "hostname">;
}

export class WorkbenchList {
  /** Every artifact, most recently edited first. */
  artifacts = $state<ArtifactSummary[]>([]);
  /** The list has arrived at least once. */
  loaded = $state(false);
  /** Why the last read failed, in plain words, or null. */
  loadError = $state<string | null>(null);
  /** Where artifacts open, or why they can't. Null until the first read lands. */
  origin = $state<ArtifactsOrigin | null>(null);
  /** Counts the reads that landed, so a view can act on a fresh list and not on its own edits. */
  generation = $state(0);
  /** Artifacts an agent changed in the last moment. */
  readonly changing = new SvelteSet<string>();
  readonly deleting = new SvelteSet<string>();
  /** Runs whose stop is in flight, as `agent:address`. */
  readonly stopping = new SvelteSet<string>();

  private request = 0;
  /** The timer that ends each artifact's "updating now", by name. */
  private glowTimers: Record<string, number> = {};

  constructor(private readonly source: WorkbenchSource) {}

  /** Read the list and follow the hub. Returns a function that stops following it. */
  start(): () => void {
    void this.load();
    const stop = this.source.onFrame((msg) => {
      this.handleFrame(msg);
    });
    return () => {
      stop();
      for (const timer of Object.values(this.glowTimers)) window.clearTimeout(timer);
      this.glowTimers = {};
      this.changing.clear();
    };
  }

  handleFrame(msg: HubServerMessage): void {
    if (msg.type === "artifact_updated") {
      this.markChanging(msg.name);
      void this.load();
    } else if (msg.type === "artifact_removed") {
      void this.load();
    } else if (msg.type === "hub_boot" && (this.loaded || this.loadError !== null)) {
      void this.load();
    }
  }

  /** An artifact's address on the artifacts origin, or null while artifacts can't open. */
  urlOf(name: string): string | null {
    return this.origin?.ok ? artifactUrl(this.origin.origin, name) : null;
  }

  /** Read the artifacts and where they open. A failure keeps the list as it was and lands in `loadError`. */
  async load(): Promise<void> {
    const request = ++this.request;
    try {
      const [artifacts, info] = await Promise.all([
        fetchWorkbenchArtifacts(),
        fetchWorkbenchInfo(),
      ]);
      if (request !== this.request) return;
      this.artifacts = artifacts;
      this.origin = resolveArtifactsOrigin(info, this.source.page());
      this.loadError = null;
      this.loaded = true;
      this.generation += 1;
    } catch (err) {
      if (request !== this.request) return;
      this.loadError = userErrorMessage(err, { action: "Couldn't load the workbench." });
    }
  }

  /** Delete an artifact and its saved data, with Undo on the toast. Resolves to whether it was deleted. */
  async remove(item: ArtifactSummary): Promise<boolean> {
    if (this.deleting.has(item.name)) return false;
    this.deleting.add(item.name);
    try {
      const { paths, checkpointId } = await deleteWorkbenchArtifact(item.name);
      this.artifacts = this.artifacts.filter((artifact) => artifact.name !== item.name);
      notifyWithUndo(null, `Deleted “${item.title}”.`, "team", paths, checkpointId, () =>
        this.load(),
      );
      return true;
    } catch (err) {
      notifications.surface(
        "error",
        userErrorMessage(err, {
          action: `Couldn't delete “${item.title}”.`,
          notFound: "It was already deleted.",
        }),
      );
      void this.load();
      return false;
    } finally {
      this.deleting.delete(item.name);
    }
  }

  /** Stop one of an artifact's sessions. The overview's next frame shows it ending. */
  async stopRun(agent: string, run: LiveSession): Promise<void> {
    const key = `${agent}:${run.address}`;
    if (this.stopping.has(key)) return;
    this.stopping.add(key);
    try {
      await stopSession(agent, run.address);
    } catch (err) {
      notifications.surface(
        "error",
        userErrorMessage(err, {
          action: `Couldn't stop “${run.purpose || run.address}” on ${agent}.`,
          notFound: "It had already finished.",
        }),
      );
    } finally {
      this.stopping.delete(key);
    }
  }

  private markChanging(name: string): void {
    this.changing.add(name);
    window.clearTimeout(this.glowTimers[name]);
    this.glowTimers[name] = window.setTimeout(() => {
      this.changing.delete(name);
    }, CHANGE_GLOW_MS);
  }
}
