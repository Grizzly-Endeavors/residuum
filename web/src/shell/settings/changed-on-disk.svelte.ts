// The question a save asks when a file changed on disk under the keys it is
// about to write: keep the user's changes, or use what is on disk.
// `ChangedOnDiskDialog`, mounted with the Settings modal, shows it.

import type { ConfigChoice, ConfigChooser, ConfigConflict } from "../../lib/config-coordinator";
import { ConflictUnanswered } from "../../lib/settings-model.svelte";

class ConflictQuestion {
  /** The file being asked about, or null. */
  current = $state.raw<ConfigConflict | null>(null);
  private settle: ((choice: ConfigChoice | null) => void) | null = null;

  /** Ask about `conflict`. Closing the question without an answer rejects with `ConflictUnanswered`. */
  ask(conflict: ConfigConflict): Promise<ConfigChoice> {
    // A question still open from another scope's save is left unanswered.
    this.answer(null);
    return new Promise((resolve, reject) => {
      this.current = conflict;
      this.settle = (choice) => {
        if (choice === null) reject(new ConflictUnanswered("the question was closed"));
        else resolve(choice);
      };
    });
  }

  /** The user's answer; null when they closed the question. */
  answer(choice: ConfigChoice | null): void {
    const settle = this.settle;
    this.settle = null;
    this.current = null;
    settle?.(choice);
  }
}

export const conflictQuestion = new ConflictQuestion();

/** The chooser every settings save passes the coordinator. */
export const chooseOnConflict: ConfigChooser = (conflict) => conflictQuestion.ask(conflict);
