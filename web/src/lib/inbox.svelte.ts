import {
  archiveUserInboxItem,
  fetchArchivedUserInbox,
  markUserInboxItemRead,
  restoreUserInboxItem,
} from "./api";
import { userErrorMessage } from "./errors";
import { notifications } from "./notifications.svelte";
import type { UserInboxItem } from "./types";

function reportFailure(err: unknown, action: string): void {
  notifications.surface(
    "error",
    userErrorMessage(err, { action, notFound: "It's no longer there. It may have moved." }),
  );
}

class UserInboxState {
  items = $state<UserInboxItem[]>([]);
  archivedItems = $state<UserInboxItem[]>([]);
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
      const updatedItem = await markUserInboxItemRead(id);
      const index = this.items.findIndex((item) => item.id === id);
      if (index !== -1) {
        this.items[index] = updatedItem;
      }
    } catch (err) {
      reportFailure(err, "Couldn't mark that item read.");
    }
  }

  async archive(id: string) {
    try {
      await archiveUserInboxItem(id);
      this.items = this.items.filter((item) => item.id !== id);
    } catch (err) {
      reportFailure(err, "Couldn't archive that item.");
    }
  }

  async refreshArchive() {
    try {
      this.archivedItems = await fetchArchivedUserInbox();
    } catch (err) {
      reportFailure(err, "Couldn't load archived items.");
    }
  }

  async restore(id: string) {
    try {
      await restoreUserInboxItem(id);
      this.archivedItems = this.archivedItems.filter((item) => item.id !== id);
      await this.refresh();
    } catch (err) {
      reportFailure(err, "Couldn't restore that item.");
    }
  }
}

export const userInbox = new UserInboxState();
