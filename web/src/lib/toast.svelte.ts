import { SvelteMap } from "svelte/reactivity";

export type ToastKind = "info" | "success" | "error";

export interface Toast {
  id: number;
  kind: ToastKind;
  message: string;
}

const DISMISS_MS = 4000;

class ToastStore {
  toasts = new SvelteMap<number, Toast>();
  private nextId = 1;

  show(message: string, kind: ToastKind = "info"): number {
    const id = this.nextId++;
    this.toasts.set(id, { id, kind, message });
    if (kind !== "error") {
      window.setTimeout(() => {
        this.dismiss(id);
      }, DISMISS_MS);
    }
    return id;
  }

  info(message: string): number {
    return this.show(message, "info");
  }

  success(message: string): number {
    return this.show(message, "success");
  }

  error(message: string): number {
    return this.show(message, "error");
  }

  dismiss(id: number): void {
    this.toasts.delete(id);
  }
}

export const toast = new ToastStore();
