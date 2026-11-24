import { toast, type ToastAction } from "./toast.svelte";

export type NotificationKind = "error" | "notice" | "system";

export interface Notification {
  id: number;
  kind: NotificationKind;
  message: string;
  details?: string;
  timestamp: Date;
}

class NotificationStore {
  history = $state<Notification[]>([]);
  private nextId = 1;

  surface(kind: NotificationKind, message: string, details?: string, action?: ToastAction): void {
    if (kind === "error") {
      toast.error(message);
    } else {
      toast.show(message, "info", action);
    }
    const entry: Notification = {
      id: this.nextId++,
      kind,
      message,
      details,
      timestamp: new Date(),
    };
    this.history.unshift(entry);
  }

  clear(): Notification[] {
    const cleared = this.history;
    this.history = [];
    return cleared;
  }

  restore(entries: Notification[]): void {
    this.history = [...this.history, ...entries];
  }
}

export const notifications = new NotificationStore();
