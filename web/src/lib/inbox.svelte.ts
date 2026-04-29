import type { UserInboxItem } from "./types";

class UserInboxState {
  items = $state<UserInboxItem[]>([]);
  unreadCount = $derived(this.items.filter((item) => !item.read).length);

  private intervalId: number | null = null;

  startPolling() {
    this.stopPolling();
    void this.refresh();
    this.intervalId = window.setInterval(() => {
      void this.refresh();
    }, 30_000);
  }

  stopPolling() {
    if (this.intervalId !== null) {
      window.clearInterval(this.intervalId);
      this.intervalId = null;
    }
  }

  async refresh() {
    try {
      const response = await fetch("/api/inbox");
      if (response.ok) {
        const data = await response.json();
        this.items = data;
      }
    } catch {
      // Silently ignore fetch failures — next poll cycle will retry
    }
  }

  async markRead(id: string) {
    try {
      const response = await fetch(`/api/inbox/${encodeURIComponent(id)}/read`, {
        method: "PUT",
      });
      if (response.ok) {
        const updatedItem = await response.json();
        const index = this.items.findIndex((item) => item.id === id);
        if (index !== -1) {
          this.items[index] = updatedItem;
        }
      }
    } catch {
      // Silently ignore — item stays unread, next refresh will reconcile
    }
  }

  async archive(id: string) {
    try {
      const response = await fetch(`/api/inbox/${encodeURIComponent(id)}/archive`, {
        method: "POST",
      });
      if (response.ok) {
        this.items = this.items.filter((item) => item.id !== id);
      }
    } catch {
      // Silently ignore — item stays in list, next refresh will reconcile
    }
  }
}

export const userInbox = new UserInboxState();
