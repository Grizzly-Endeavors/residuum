// Questions asked from code, answered in a confirm dialog. `ConfirmHost`,
// mounted once by the shell, shows them one at a time.

import { tick } from "svelte";
import type { ConfirmLeave } from "../navigation-guard";
import type { ConfirmTone } from "./types";

export interface ConfirmRequest {
  title: string;
  message?: string;
  /** What going ahead affects, one line each. */
  items?: readonly string[];
  confirmLabel: string;
  cancelLabel?: string;
  tone?: ConfirmTone;
}

interface Pending {
  request: ConfirmRequest;
  resolve: (confirmed: boolean) => void;
}

class Confirmations {
  /** The question on screen, or null. */
  current = $state.raw<ConfirmRequest | null>(null);
  private shown: Pending | null = null;
  private readonly waiting: Pending[] = [];

  /** Ask, and hear true if the user goes ahead. Questions asked while one is open wait their turn. */
  ask(request: ConfirmRequest): Promise<boolean> {
    return new Promise((resolve) => {
      this.waiting.push({ request, resolve });
      this.showNext();
    });
  }

  /**
   * The host's report of the user's answer. The dialog closes first, and the
   * answer is given once it is gone: the dialog's history entry is then
   * already leaving, so a navigation the answer allows starts from below it.
   */
  answer(confirmed: boolean): void {
    const shown = this.shown;
    if (shown === null) return;
    this.shown = null;
    this.current = null;
    void tick().then(() => {
      shown.resolve(confirmed);
      this.showNext();
    });
  }

  private showNext(): void {
    if (this.shown !== null) return;
    const next = this.waiting.shift();
    if (next === undefined) return;
    this.shown = next;
    this.current = next.request;
  }
}

export const confirmations = new Confirmations();

/** Asks before navigation loses unsaved work: what the shell gives `router.guard.setConfirm`. */
export const confirmLeave: ConfirmLeave = (losses) =>
  confirmations.ask({
    title: "Discard unsaved changes?",
    message: "Leaving now loses:",
    items: losses,
    confirmLabel: "Discard and leave",
    cancelLabel: "Keep editing",
    tone: "danger",
  });
