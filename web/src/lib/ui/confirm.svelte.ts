import { tick } from "svelte";
import type { ConfirmLeave } from "../navigation-guard";
import type { ConfirmTone } from "./types";

export interface ConfirmRequest {
  title: string;
  message?: string;
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
  current = $state.raw<ConfirmRequest | null>(null);
  private shown: Pending | null = null;
  private readonly waiting: Pending[] = [];

  ask(request: ConfirmRequest): Promise<boolean> {
    return new Promise((resolve) => {
      this.waiting.push({ request, resolve });
      this.showNext();
    });
  }

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

export const confirmLeave: ConfirmLeave = (losses) =>
  confirmations.ask({
    title: "Discard unsaved changes?",
    message: "Leaving now loses:",
    items: losses,
    confirmLabel: "Discard and leave",
    cancelLabel: "Keep editing",
    tone: "danger",
  });
