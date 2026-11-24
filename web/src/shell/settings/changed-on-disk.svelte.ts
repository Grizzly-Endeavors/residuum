import type { ConfigChoice, ConfigChooser, ConfigConflict } from "../../lib/config-coordinator";
import { ConflictUnanswered } from "../../lib/settings-model.svelte";

class ConflictQuestion {
  current = $state.raw<ConfigConflict | null>(null);
  private settle: ((choice: ConfigChoice | null) => void) | null = null;

  ask(conflict: ConfigConflict): Promise<ConfigChoice> {
    this.answer(null);
    return new Promise((resolve, reject) => {
      this.current = conflict;
      this.settle = (choice) => {
        if (choice === null) reject(new ConflictUnanswered("the question was closed"));
        else resolve(choice);
      };
    });
  }

  answer(choice: ConfigChoice | null): void {
    const settle = this.settle;
    this.settle = null;
    this.current = null;
    settle?.(choice);
  }
}

export const conflictQuestion = new ConflictQuestion();

export const chooseOnConflict: ConfigChooser = (conflict) => conflictQuestion.ask(conflict);
