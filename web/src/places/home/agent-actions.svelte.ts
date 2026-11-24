import { findSettingsFix } from "../../lib/agent-failure";
import type { AgentSummary } from "../../lib/hub-types";
import { hub } from "../../lib/hub.svelte";
import { notifications } from "../../lib/notifications.svelte";
import type * as SettingsModel from "../../lib/settings-model.svelte";
import type { SectionId } from "../../lib/settings-sections";
import { confirmations } from "../../lib/ui";

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
  private autostartWanted = $state<Record<string, boolean | undefined>>({});

  pendingOf(name: string): PendingAction | undefined {
    return this.pending[name];
  }

  autostartOf(agent: AgentSummary): boolean {
    return this.autostartWanted[agent.name] ?? agent.autostart;
  }

  start(name: string): Promise<void> {
    return this.run(name, "start", () => hub.startAgent(name));
  }

  stop(name: string): Promise<void> {
    return this.run(name, "stop", () => hub.stopAgent(name));
  }

  restart(name: string, force = false): Promise<void> {
    return this.run(name, "restart", () => hub.restartAgent(name, force));
  }

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
