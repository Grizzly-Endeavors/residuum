import {
  archiveUserInboxItem,
  fetchArchivedUserInbox,
  markUserInboxItemRead,
  restoreUserInboxItem,
} from "./api";
import { userErrorMessage } from "./errors";
import { agentPath, requireAgent } from "./paths";
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
  /** The agent whose inbox this is, `null` before one is bound. */
  private agent: string | null = null;

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
   * Forget the bound agent's items and take `agent` as the new one (`null` for
   * none). Polling carries on for it when it was running.
   */
  reset(agent: string | null): void {
    this.agent = agent;
    this.items = [];
    this.archivedItems = [];
    if (this.intervalId !== null) this.startPolling();
  }

  async refresh(): Promise<void> {
    const agent = this.agent;
    if (agent === null) return;
    try {
      const response = await fetch(agentPath(agent, "/inbox"));
      if (response.ok) {
        const data = (await response.json()) as UserInboxItem[];
        if (agent === this.agent) this.items = data;
      }
    } catch {
      // Silently ignore fetch failures — next poll cycle will retry
    }
  }

  async markRead(id: string): Promise<void> {
    try {
      const updatedItem = await markUserInboxItemRead(requireAgent(this.agent), id);
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
      await archiveUserInboxItem(requireAgent(this.agent), id);
      this.items = this.items.filter((item) => item.id !== id);
    } catch (err) {
      reportFailure(err, "Couldn't archive that item.");
    }
  }

  async refreshArchive(): Promise<void> {
    try {
      this.archivedItems = await fetchArchivedUserInbox(requireAgent(this.agent));
    } catch (err) {
      reportFailure(err, "Couldn't load archived items.");
    }
  }

  async restore(id: string): Promise<void> {
    try {
      await restoreUserInboxItem(requireAgent(this.agent), id);
      this.archivedItems = this.archivedItems.filter((item) => item.id !== id);
      await this.refresh();
    } catch (err) {
      reportFailure(err, "Couldn't restore that item.");
    }
  }
}

export const userInbox = new UserInboxState();
