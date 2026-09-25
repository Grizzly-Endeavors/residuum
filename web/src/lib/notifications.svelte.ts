// System-notification orchestrator: shows an ephemeral toast AND records the
// message in a recall history list shown in the notification corner dropdown.
//
// Use `surface()` for system-originated events (server errors/notices, slash
// command output) where the user might want to recall the message later.
// For local UI feedback like "settings saved" or form validation errors,
// call `toast.show()` directly — those are single-action confirmations and
// don't belong in history.
//
// History is in-memory only and holds every notification for the session; it
// does not persist across a page reload.

import { toast } from "./toast.svelte";

export type NotificationKind = "error" | "notice" | "system";

export interface Notification {
  id: number;
  kind: NotificationKind;
  message: string;
  /** Full technical detail behind an expandable toggle in the recall list. */
  details?: string;
  timestamp: Date;
}

class NotificationStore {
  history = $state<Notification[]>([]);
  private nextId = 1;

  /**
   * Show as a transient toast AND record in history. `details`, when
   * given, is never shown in the toast itself — only behind the recall
   * list's expandable toggle.
   */
  surface(kind: NotificationKind, message: string, details?: string): void {
    if (kind === "error") {
      toast.error(message);
    } else {
      toast.info(message);
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

  clear(): void {
    this.history = [];
  }
}

export const notifications = new NotificationStore();
