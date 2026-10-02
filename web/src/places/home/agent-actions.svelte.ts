// What Home and an agent's state card do to an agent: start, stop, restart,
// Start automatically, finding where its settings can be fixed, delete after
// asking, and restore. One action runs per agent at a time, and `pendingOf`
// says which is in flight. The hub store surfaces failures, and its frames
// raise the created, deleted (with Undo) and restored toasts.

import { findSettingsFix } from "../../lib/agent-failure";
import type { AgentSummary } from "../../lib/hub-types";
import { hub } from "../../lib/hub.svelte";
import { notifications } from "../../lib/notifications.svelte";
import type * as SettingsModel from "../../lib/settings-model.svelte";
import type { SectionId } from "../../lib/settings-sections";
import { confirmations } from "../../lib/ui";

/** The settings model, which loads with Settings rather than with Home. */
function loadSettingsModel(): Promise<typeof SettingsModel> {
  return import("../../lib/settings-model.svelte");
}

export type PendingAction =
  | "start"
  | "stop"
  | "restart"
  | "autostart"
  | "fix"
  | "delete"
  | "restore";

class AgentActions {
  private pending = $state<Record<string, PendingAction | undefined>>({});
  /** The Start automatically value being saved, shown until the hub answers. */
  private autostartWanted = $state<Record<string, boolean | undefined>>({});

  /** The action in flight for an agent, if any. */
  pendingOf(name: string): PendingAction | undefined {
    return this.pending[name];
  }

  /** Start automatically as the menu shows it: the value being saved, else the hub's. */
  autostartOf(agent: AgentSummary): boolean {
    return this.autostartWanted[agent.name] ?? agent.autostart;
  }

  start(name: string): Promise<void> {
    return this.run(name, "start", () => hub.startAgent(name));
  }

  stop(name: string): Promise<void> {
    return this.run(name, "stop", () => hub.stopAgent(name));
  }

  restart(name: string): Promise<void> {
    return this.run(name, "restart", () => hub.restartAgent(name));
  }

  /** Flip Start automatically. When the hub refuses, the menu goes back to the hub's value. */
  async toggleAutostart(agent: AgentSummary): Promise<void> {
    if (this.pending[agent.name] !== undefined) return;
    const wanted = !this.autostartOf(agent);
    this.autostartWanted[agent.name] = wanted;
    try {
      await this.run(agent.name, "autostart", () => hub.setAutostart(agent.name, wanted));
    } finally {
      this.autostartWanted[agent.name] = undefined;
    }
  }

  /**
   * The Settings section where the agent's failing setting can be fixed
   * (`findSettingsFix`), or undefined while another action runs for it. The
   * problems are flagged on the agent's settings, and the field they name is
   * focused when the section shows.
   */
  async fixSection(name: string): Promise<SectionId | undefined> {
    let section: SectionId | undefined;
    await this.run(name, "fix", async () => {
      const fix = await findSettingsFix(name);
      if (fix.field !== null) {
        const { settingsModel } = await loadSettingsModel();
        const scope = settingsModel.agent(name);
        scope.flagProblems(fix.field.file, fix.field.problems);
        scope.requestFocus(fix.field.ref);
      }
      section = fix.section;
    });
    return section;
  }

  /** Ask, then delete. The hub's "deleted" toast carries Undo, and Recently deleted keeps Restore. */
  async delete(agent: AgentSummary): Promise<void> {
    const { name } = agent;
    const running = agent.state === "running" || agent.state === "starting";
    const confirmed = await confirmations.ask({
      title: `Delete ${name}?`,
      message:
        `${running ? `${name} stops, and its` : `${name}'s`} folder is removed: its notes, ` +
        "memory, settings and role page. A checkpoint is taken first, so Undo or Restore can " +
        "bring it back.",
      confirmLabel: `Delete ${name}`,
      tone: "danger",
    });
    if (!confirmed) return;
    await this.run(name, "delete", async () => {
      const outcome = await hub.deleteAgent(name);
      if (outcome === null) return;
      // The hub's agent_deleted frame drops it too, while the hub socket is up.
      const { settingsModel } = await loadSettingsModel();
      settingsModel.drop(name);
      if (outcome.checkpoint_id === null) {
        notifications.surface(
          "error",
          `${name} was deleted, but no checkpoint was taken first, so Undo and Restore can't ` +
            "bring back its latest files.",
        );
      }
    });
  }

  /** Restore a deleted agent from `checkpointId`, the one its deletion took. */
  restore(name: string, checkpointId: string): Promise<void> {
    return this.run(name, "restore", () => hub.restoreAgent(name, checkpointId));
  }

  private async run(
    name: string,
    action: PendingAction,
    call: () => Promise<unknown>,
  ): Promise<void> {
    if (this.pending[name] !== undefined) return;
    this.pending[name] = action;
    try {
      await call();
    } finally {
      this.pending[name] = undefined;
    }
  }
}

export const agentActions = new AgentActions();
