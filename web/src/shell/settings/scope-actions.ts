// What the Settings modal does to a scope: Save changes, Discard, Undo after a
// save, and Reload from disk. The save bar, the frame's buttons and the
// palette share these, so each reports the same way.

import { SvelteMap } from "svelte/reactivity";
import type { AppAction } from "../../lib/action-registry.svelte";
import type { SaveResult } from "../../lib/settings-model.svelte";
import { toast } from "../../lib/toast.svelte";
import { confirmations } from "../../lib/ui";
import { chooseOnConflict } from "./changed-on-disk.svelte";
import { scopeName, type SettingsScope } from "./sections";

/** Each scope's failed save whose changes were discarded since, which the save bar no longer explains. */
const discardedFailures = new SvelteMap<string, SaveResult>();

/** The last save, when it left files unsaved and their changes are still staged. */
export function unsavedFailure(scope: SettingsScope): SaveResult | null {
  const result = scope.lastResult;
  if (result?.outcome !== "partial" && result?.outcome !== "failed") return null;
  return discardedFailures.get(scope.id) === result ? null : result;
}

/** Save every staged change. A failure stays on the save bar; a save that wrote everything says so with Undo. */
export async function saveScope(scope: SettingsScope): Promise<void> {
  const result = await scope.save(chooseOnConflict);
  if (result.outcome !== "saved") return;
  const undo = { label: "Undo", onClick: () => void undoSave(scope) };
  toast.success(result.message, scope.undoable ? undo : undefined);
}

/** Restore what the last save wrote, and say what came back and what didn't. */
export async function undoSave(scope: SettingsScope): Promise<void> {
  const result = await scope.undo();
  if (result.failed.length > 0) toast.error(result.message);
  else toast.success(result.message);
}

/** The changes a failed save left staged are being dropped, so the save bar stops explaining it. */
function forgetFailure(scope: SettingsScope): void {
  const failure = unsavedFailure(scope);
  if (failure !== null) discardedFailures.set(scope.id, failure);
}

export function discardScope(scope: SettingsScope): void {
  forgetFailure(scope);
  scope.discard();
  toast.info("Discarded your changes.");
}

/** Read the scope's files again, asking first when that drops staged changes. */
export async function reloadScope(scope: SettingsScope): Promise<void> {
  if (scope.dirty) {
    const confirmed = await confirmations.ask({
      title: "Discard unsaved changes?",
      message: `Reloading reads ${scopeName(scope)} from disk again and drops the changes you haven't saved.`,
      confirmLabel: "Discard and reload",
      cancelLabel: "Keep editing",
      tone: "danger",
    });
    if (!confirmed) return;
    forgetFailure(scope);
  }
  await scope.reload();
  if (scope.loadError === null) toast.info(`Reloaded ${scopeName(scope)} from disk.`);
}

/** The palette's actions for the scope the open modal shows. */
export function modalActions(scope: SettingsScope): AppAction[] {
  const hint = scope.agent ?? "All agents";
  const nothingStaged = scope.dirty ? undefined : "There are no unsaved changes";
  const base = { group: "Settings", hint } as const;
  return [
    {
      ...base,
      id: "settings-modal:save",
      label: "Save changes",
      icon: "check",
      disabled: nothingStaged ?? (scope.saving ? "Saving now" : undefined),
      run: () => void saveScope(scope),
    },
    {
      ...base,
      id: "settings-modal:discard",
      label: "Discard changes",
      icon: "close",
      disabled: nothingStaged,
      run: () => {
        discardScope(scope);
      },
    },
    {
      ...base,
      id: "settings-modal:reload",
      label: "Reload settings from disk",
      icon: "reload",
      run: () => void reloadScope(scope),
    },
  ];
}
