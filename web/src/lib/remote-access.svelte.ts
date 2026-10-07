// ── Remote access status and the instance switcher ───────────────────
//
// The secure tunnel's status, read on a schedule so the shell's instance
// switcher doesn't depend on the Settings modal being open, and the switch
// itself. Residuum Cloud's list of the user's instances is untrusted: only
// entries with a valid slug are kept, and nothing here navigates anywhere
// the relay named.

import { userErrorMessage } from "./errors";
import type { InstanceInfo } from "./generated/InstanceInfo";
import type { RemoteAccessStatus } from "./generated/RemoteAccessStatus";
import { withValidSlugs } from "./instance-slug";
import { activateInstance, fetchRemoteAccess } from "./remote-access-api";
import { toast } from "./toast.svelte";

const REFRESH_MS = 30_000;
/** After a switch, how long the page waits for the relay to close its connections to the old instance. */
const RELOAD_DELAY_MS = 1500;

export class RemoteAccessStore {
  /** The last status read, or null before the first read or while none can be read. */
  status = $state.raw<RemoteAccessStatus | null>(null);
  /** The slug being switched to, from the press until the page reloads or the switch fails. */
  switching = $state<string | null>(null);

  /** The user's instances that have a usable slug. */
  readonly instances = $derived<InstanceInfo[]>(withValidSlugs(this.status?.instances ?? []));

  /** Take a status another reader just fetched, so the switcher shows it without another request. */
  accept(status: RemoteAccessStatus): void {
    this.status = status;
  }

  /** Read the status now. A failed read keeps what was known; the Remote access group in Settings reports why. */
  async refresh(): Promise<void> {
    try {
      this.status = await fetchRemoteAccess();
    } catch {
      // The last status stays.
    }
  }

  /** Read now and then every half minute, until the returned function is called. */
  follow(): () => void {
    void this.refresh();
    const timer = setInterval(() => void this.refresh(), REFRESH_MS);
    return () => {
      clearInterval(timer);
    };
  }

  /** Make `slug` the instance the user's address goes to, then reload to reach it. */
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
