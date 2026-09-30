import {
  archiveUserInboxItem,
  fetchArchivedUserInbox,
  markUserInboxItemRead,
  restoreUserInboxItem,
} from "./api";
import { userErrorMessage } from "./errors";
import { agentPath, getCurrentAgent } from "./paths";
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

  startPolling(): void {
    this.stopPolling();
    void this.refresh();
    this.intervalId = window.setInterval(() => {
      void this.refresh();
    }, 30_000);
  }

  stopPolling(): void {
    if (this.intervalId !== null) {
      window.clearInterval(this.intervalId);
      this.intervalId = null;
    }
  }

  /**
   * Forget the current agent's items, for a switch to another agent. Polling
   * carries on for the new agent when it was running.
   */
  reset(): void {
    this.items = [];
    this.archivedItems = [];
    if (this.intervalId !== null) this.startPolling();
  }

  async refresh(): Promise<void> {
    const agent = getCurrentAgent();
    if (agent === null) return;
    try {
      const response = await fetch(agentPath("/inbox", agent));
      if (response.ok) {
        const data = (await response.json()) as UserInboxItem[];
        if (agent === getCurrentAgent()) this.items = data;
      }
    } catch {
      // Silently ignore fetch failures — next poll cycle will retry
    }
  }

  async markRead(id: string): Promise<void> {
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

  async archive(id: string): Promise<void> {
    try {
      await archiveUserInboxItem(id);
      this.items = this.items.filter((item) => item.id !== id);
    } catch (err) {
      reportFailure(err, "Couldn't archive that item.");
    }
  }

  async refreshArchive(): Promise<void> {
    try {
      this.archivedItems = await fetchArchivedUserInbox();
    } catch (err) {
      reportFailure(err, "Couldn't load archived items.");
    }
  }

  async restore(id: string): Promise<void> {
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
