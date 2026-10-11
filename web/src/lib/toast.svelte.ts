import { SvelteMap } from "svelte/reactivity";

export type ToastKind = "info" | "success" | "error";

/** A one-click follow-up action offered from a toast, e.g. "Undo". */
export interface ToastAction {
  label: string;
  onClick: () => void;
}

export interface Toast {
  id: number;
  kind: ToastKind;
  message: string;
  action?: ToastAction;
}

const DISMISS_MS = 4000;
/** Toasts carrying an action stay up longer — there's something to click. */
const DISMISS_WITH_ACTION_MS = 10_000;

class ToastStore {
  toasts = new SvelteMap<number, Toast>();
  private nextId = 1;

  show(message: string, kind: ToastKind = "info", action?: ToastAction): number {
    const id = this.nextId++;
    this.toasts.set(id, { id, kind, message, action });
    if (kind !== "error") {
      setTimeout(
        () => {
          this.dismiss(id);
        },
        action ? DISMISS_WITH_ACTION_MS : DISMISS_MS,
      );
    }
    return id;
  }

  info(message: string): number {
    return this.show(message, "info");
  }

  success(message: string, action?: ToastAction): number {
    return this.show(message, "success", action);
  }

  error(message: string): number {
    return this.show(message, "error");
  }

  /** Run a toast action, then dismiss the toast regardless of outcome — a
   * second click while it's still settling would otherwise re-run it. */
  runAction(id: number): void {
    const t = this.toasts.get(id);
    this.dismiss(id);
    t?.action?.onClick();
  }

  dismiss(id: number): void {
    this.toasts.delete(id);
  }
}

export const toast = new ToastStore();
