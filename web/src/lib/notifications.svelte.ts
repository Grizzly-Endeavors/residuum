// System-notification orchestrator: shows an ephemeral toast AND records the
// message in a recall history list shown in the notification corner dropdown.
//
// Use `surface()` for system-originated events (server errors/notices, slash
// command output) where the user might want to recall the message later.
// For local UI feedback like "settings saved" or form validation errors,
// call `toast.show()` directly — those are single-action confirmations and
// don't belong in history.
//
// History is in-memory only. Add localStorage persistence when the broader
// browser-caching pass lands.

import { toast } from "./toast.svelte";

export type NotificationKind = "error" | "notice" | "system";

export interface Notification {
  id: number;
  kind: NotificationKind;
  message: string;
  timestamp: Date;
}

const HISTORY_CAP = 50;

class NotificationStore {
  history = $state<Notification[]>([]);
  private nextId = 1;

  /** Show as a transient toast AND record in history. */
  surface(kind: NotificationKind, message: string): void {
    if (kind === "error") {
      toast.error(message);
    } else {
      toast.info(message);
    }
    const entry: Notification = {
      id: this.nextId++,
      kind,
      message,
      timestamp: new Date(),
    };
    this.history.unshift(entry);
    if (this.history.length > HISTORY_CAP) {
      this.history.length = HISTORY_CAP;
    }
  }

  clear(): void {
    this.history = [];
  }
}

export const notifications = new NotificationStore();
