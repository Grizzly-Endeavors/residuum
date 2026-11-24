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
import { handoffUrl } from "../../lib/pairing";
import { createWorkbenchHandoff } from "../../lib/pairing-api";
import { artifactUrl, resolveArtifactsOrigin, type ArtifactsOrigin } from "../../lib/workbench";

export const CHANGE_GLOW_MS = 2400;

export interface WorkbenchSource {
  onFrame: (listener: (msg: HubServerMessage) => void) => () => void;
  page: () => Pick<Location, "origin" | "protocol" | "hostname">;
}

export class WorkbenchList {
  artifacts = $state<ArtifactSummary[]>([]);
  loaded = $state(false);
  loadError = $state<string | null>(null);
  origin = $state<ArtifactsOrigin | null>(null);
  /** This page is served through Residuum Cloud, where the workbench host wants its own credential. */
  viaRelay = $state(false);
  generation = $state(0);
  readonly changing = new SvelteSet<string>();
  readonly deleting = new SvelteSet<string>();
  readonly stopping = new SvelteSet<string>();

  private request = 0;
  private glowTimers: Record<string, number> = {};

  constructor(private readonly source: WorkbenchSource) {}

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

  urlOf(name: string): string | null {
    return this.origin?.ok ? artifactUrl(this.origin.origin, name) : null;
  }

  open(event: MouseEvent, name: string): void {
    const origin = this.origin;
    if (!this.viaRelay || origin?.ok !== true) return;
    event.preventDefault();
    void this.openThroughHandoff(origin.origin, name);
  }

  private async openThroughHandoff(origin: string, name: string): Promise<void> {
    const tab = window.open("", "_blank");
    if (tab !== null) tab.opener = null;
    try {
      const { token } = await createWorkbenchHandoff();
      const url = handoffUrl(origin, name, token);
      if (tab === null) window.location.assign(url);
      else tab.location.href = url;
    } catch (err) {
      tab?.close();
      notifications.surface("error", userErrorMessage(err, { action: "Couldn't open that page." }));
    }
  }

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
      this.viaRelay = info.relay !== null && this.source.page().origin === info.relay.ui_origin;
      this.loadError = null;
      this.loaded = true;
      this.generation += 1;
    } catch (err) {
      if (request !== this.request) return;
      this.loadError = userErrorMessage(err, { action: "Couldn't load the workbench." });
    }
  }

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
