<script lang="ts">
  import type { PinInfo } from "../../lib/generated/PinInfo";
  import type { SiblingInfo } from "../../lib/generated/SiblingInfo";
  import { instanceName, withValidSlugs } from "../../lib/instance-slug";
  import { removePin } from "../../lib/remote-access-api";
  import { toast } from "../../lib/toast.svelte";
  import { Button, confirmations } from "../../lib/ui";
  import type { RemoteAct } from "./remote-access-act";

  // The instances that have joined this one, and the certificate accounts of
  // instances that no longer exist in Residuum Cloud, which can be removed.
  // Names are text from the relay and are never drawn as markup.

  interface Props {
    siblings: readonly SiblingInfo[];
    pins: readonly PinInfo[];
    busy: string | null;
    act: RemoteAct;
  }

  let { siblings, pins, busy, act }: Props = $props();

  /** Siblings with a usable slug, one row each. */
  const joined = $derived([
    ...new Map(withValidSlugs(siblings).map((sibling) => [sibling.slug, sibling])).values(),
  ]);
  const removable = $derived(pins.filter((pin) => pin.removable));

  async function remove(pin: PinInfo): Promise<void> {
    const confirmed = await confirmations.ask({
      title: `Remove ${pin.slug}?`,
      message: `${pin.slug} no longer exists in Residuum Cloud. Removing it stops that certificate account from getting new certificates. Certificates it already holds stay valid until they expire.`,
      confirmLabel: "Remove account",
      tone: "danger",
    });
    if (!confirmed) return;
    await act(`remove:${pin.account_uri}`, `Couldn't remove ${pin.slug}.`, async () => {
      await removePin(pin.account_uri);
      toast.success(`Removed ${pin.slug}.`);
    });
  }
</script>

{#if joined.length > 0}
  <section class="ri" aria-label="Joined instances">
    <h4 class="ri-title">Joined instances</h4>
    <ul class="ri-list">
      {#each joined as sibling (sibling.slug)}
        <li class="ri-row">{instanceName(sibling)}</li>
      {/each}
    </ul>
  </section>
{/if}

{#if removable.length > 0}
  <section class="ri" aria-label="Certificate accounts to remove">
    <h4 class="ri-title">Instances that no longer exist</h4>
    <p class="ri-note">
      Residuum Cloud no longer lists these instances, but their certificate accounts can still get
      certificates for your addresses.
    </p>
    <ul class="ri-list">
      {#each removable as pin (pin.account_uri)}
        <li class="ri-row">
          <span class="ri-name">{pin.slug}</span>
          <Button
            size="sm"
            variant="quiet"
            aria-label="Remove {pin.slug}"
            loading={busy === `remove:${pin.account_uri}`}
            onclick={() => void remove(pin)}
          >
            Remove
          </Button>
        </li>
      {/each}
    </ul>
  </section>
{/if}

<style>
  .ri {
    display: flex;
    flex-direction: column;
    gap: var(--space-6);
  }

  .ri-title {
    font-size: var(--font-size-sm);
    font-weight: var(--font-weight-semibold);
  }

  .ri-note {
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
    line-height: var(--line-height-ui);
  }

  .ri-list {
    list-style: none;
  }

  .ri-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-8);
    min-height: 32px;
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
    overflow-wrap: anywhere;
  }

  .ri-name {
    min-width: 0;
    font-family: var(--font-code);
  }
</style>
