import { accountDashboardUrl } from "./cloud.svelte";
import { userErrorMessage } from "./errors";
import type { InstanceInfo } from "./generated/InstanceInfo";
import type { RemoteAccessStatus } from "./generated/RemoteAccessStatus";
import { withValidSlugs } from "./instance-slug";
import { activateInstance, fetchRemoteAccess } from "./remote-access-api";
import { toast } from "./toast.svelte";

const REFRESH_MS = 30_000;
const RELOAD_DELAY_MS = 1500;

export class RemoteAccessStore {
  status = $state.raw<RemoteAccessStatus | null>(null);
  switching = $state<string | null>(null);

  readonly instances = $derived<InstanceInfo[]>(withValidSlugs(this.status?.instances ?? []));

  readonly accountUrl = $derived(accountDashboardUrl(this.status?.relay_origin));

  accept(status: RemoteAccessStatus): void {
    this.status = status;
  }

  async refresh(): Promise<void> {
    try {
      this.status = await fetchRemoteAccess();
    } catch {}
  }

  follow(): () => void {
    void this.refresh();
    const timer = setInterval(() => void this.refresh(), REFRESH_MS);
    return () => {
      clearInterval(timer);
    };
  }

  async activate(slug: string): Promise<void> {
    if (this.switching !== null) return;
    const target = this.instances.find((instance) => instance.slug === slug);
    if (target === undefined || target.active) return;
    this.switching = target.slug;
    try {
      await activateInstance(target.slug);
    } catch (err) {
      this.switching = null;
      toast.error(userErrorMessage(err, { action: "Couldn't switch instances." }));
      return;
    }
    setTimeout(() => {
      window.location.reload();
    }, RELOAD_DELAY_MS);
  }
}

export const remoteAccess = new RemoteAccessStore();
